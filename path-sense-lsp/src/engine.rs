use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs;
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::{
    Command, CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionResponse,
    CompletionTextEdit, InsertTextFormat, Position, TextEdit,
};

use crate::completion_documentation::{completion_item_data, fallback_completion_documentation};
use crate::context::{
    CompletionContext, OutsideStringsConfig, extract_completion_context,
    mapping_key_supports_prefix_completion,
};
use crate::resolver::{ResolvedBase, WorkspaceRoots, parent_candidate_dir, resolve_bases};
use crate::settings::CompiledSettings;
use crate::syntax::SyntaxSnapshot;

const RETRIGGER_COMPLETION_COMMAND: &str = "editor::ShowCompletions";

#[derive(Default, Debug, Clone, Copy)]
pub struct PathSenseEngine;

pub struct CompletionRequest<'a> {
    pub text: &'a str,
    pub position: Position,
    pub syntax: Option<&'a SyntaxSnapshot>,
    pub document_path: Option<&'a Path>,
    pub workspace_roots: &'a WorkspaceRoots,
    pub is_manual_trigger: bool,
    pub trigger_character: Option<char>,
    pub settings: &'a CompiledSettings,
}

impl PathSenseEngine {
    #[must_use]
    pub fn complete(&self, request: &CompletionRequest<'_>) -> Option<CompletionResponse> {
        let mapping_keys = request.settings.normalized_path_mapping_keys();
        let outside_strings =
            request
                .settings
                .trigger_outside_strings()
                .then_some(OutsideStringsConfig {
                    path_separators: request.settings.path_separators(),
                    mapping_keys,
                });
        let context = extract_completion_context(
            request.text,
            request.position,
            request.syntax,
            request.document_path,
            request.is_manual_trigger,
            outside_strings.as_ref(),
        )?;

        if request
            .settings
            .ignored_prefixes()
            .iter()
            .any(|prefix| context.line_prefix.ends_with(prefix))
        {
            return Some(CompletionResponse::Array(Vec::new()));
        }
        if should_suppress_for_min_auto_trigger(request, &context, mapping_keys) {
            return None;
        }

        let mapping_items = Self::mapping_prefix_items(&context, mapping_keys, request.settings);
        if !mapping_items.is_empty() {
            return Some(CompletionResponse::Array(mapping_items));
        }

        let bases = resolve_bases(&context, request.workspace_roots, request.settings);
        let (items, has_existing_file_base) =
            Self::completion_items_for_bases(&bases, context.replacement_range, request.settings);
        if items.is_empty() && has_existing_file_base {
            return None;
        }
        Some(CompletionResponse::Array(items))
    }

    #[must_use]
    pub(crate) fn items_for_bases(
        bases: &[ResolvedBase],
        replacement_range: tower_lsp::lsp_types::Range,
        settings: &CompiledSettings,
    ) -> Vec<CompletionItem> {
        Self::completion_items_for_bases(bases, replacement_range, settings).0
    }

    fn completion_items_for_bases(
        bases: &[ResolvedBase],
        replacement_range: tower_lsp::lsp_types::Range,
        settings: &CompiledSettings,
    ) -> (Vec<CompletionItem>, bool) {
        let mut deduped = BTreeMap::new();
        let mut has_existing_file_base = false;

        for base in bases {
            if base_points_to_existing_file(base) {
                has_existing_file_base = true;
                continue;
            }
            for candidate in read_directory_candidates(base, settings) {
                let key = (
                    candidate.name.clone(),
                    candidate.is_dir,
                    candidate.insert_prefix.clone(),
                );
                match deduped.entry(key) {
                    Entry::Vacant(slot) => {
                        slot.insert(candidate);
                    }
                    Entry::Occupied(mut slot) => {
                        if compare_candidates(&candidate, slot.get()) == Ordering::Less {
                            slot.insert(candidate);
                        }
                    }
                }
            }
        }

        let mut candidates = deduped.into_values().collect::<Vec<_>>();
        candidates.sort_by(compare_candidates);
        (
            candidates
                .into_iter()
                .take(200)
                .map(|candidate| candidate.into_completion_item(replacement_range, settings))
                .collect(),
            has_existing_file_base,
        )
    }

    #[must_use]
    pub fn items_for_context(
        context: &CompletionContext,
        workspace_roots: &WorkspaceRoots,
        settings: &CompiledSettings,
    ) -> Vec<CompletionItem> {
        let mapping_items =
            Self::mapping_prefix_items(context, settings.normalized_path_mapping_keys(), settings);
        if !mapping_items.is_empty() {
            return mapping_items;
        }

        let bases = resolve_bases(context, workspace_roots, settings);
        Self::items_for_bases(&bases, context.replacement_range, settings)
    }

    #[must_use]
    pub fn items_for_mapping_prefixes(
        context: &CompletionContext,
        settings: &CompiledSettings,
    ) -> Vec<CompletionItem> {
        Self::mapping_prefix_items(context, settings.normalized_path_mapping_keys(), settings)
    }

    fn mapping_prefix_items(
        context: &CompletionContext,
        mapping_keys: &[String],
        settings: &CompiledSettings,
    ) -> Vec<CompletionItem> {
        let mut candidates = mapping_keys
            .iter()
            .filter(|key| key.as_str() != context.raw_token.as_str())
            .filter(|key| {
                mapping_key_supports_prefix_completion(key.as_str(), context.raw_token.as_str())
            })
            .map(|key| Candidate::new(key.clone(), true, None, String::new(), MatchQuality::Prefix))
            .collect::<Vec<_>>();

        candidates.sort_by(compare_candidates);
        candidates
            .into_iter()
            .take(200)
            .map(|candidate| candidate.into_completion_item(context.replacement_range, settings))
            .collect()
    }
}

fn should_suppress_for_min_auto_trigger(
    request: &CompletionRequest<'_>,
    context: &CompletionContext,
    mapping_keys: &[String],
) -> bool {
    !request.is_manual_trigger
        && !bypasses_min_auto_trigger(request, context, mapping_keys)
        && context.prefix.chars().count() < request.settings.min_auto_trigger_word_length()
}

fn bypasses_min_auto_trigger(
    request: &CompletionRequest<'_>,
    context: &CompletionContext,
    mapping_keys: &[String],
) -> bool {
    is_path_separator_trigger(request.trigger_character)
        || is_explicit_path_root(context, mapping_keys)
        || is_alias_prefix_context(context, mapping_keys)
}

fn is_path_separator_trigger(trigger_character: Option<char>) -> bool {
    matches!(trigger_character, Some('/' | '.' | '~'))
}

fn is_explicit_path_root(context: &CompletionContext, mapping_keys: &[String]) -> bool {
    let raw_token = context.raw_token.as_str();
    raw_token == "~"
        || raw_token.starts_with("~/")
        || raw_token.starts_with('/')
        || raw_token.starts_with("./")
        || raw_token.starts_with("../")
        || mapping_keys.iter().any(|key| key == raw_token)
}

fn is_alias_prefix_context(context: &CompletionContext, mapping_keys: &[String]) -> bool {
    mapping_keys
        .iter()
        .any(|key| mapping_key_supports_prefix_completion(key.as_str(), context.raw_token.as_str()))
}

fn base_points_to_existing_file(base: &ResolvedBase) -> bool {
    fs::metadata(&base.target_dir).is_ok_and(|metadata| metadata.is_file())
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Candidate {
    name: String,
    sort_name: String,
    is_dir: bool,
    path: Option<PathBuf>,
    insert_prefix: String,
    match_quality: MatchQuality,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum MatchQuality {
    Prefix,
    Contains,
}

impl Candidate {
    fn new(
        name: String,
        is_dir: bool,
        path: Option<PathBuf>,
        insert_prefix: String,
        match_quality: MatchQuality,
    ) -> Self {
        let sort_name = name.to_lowercase();

        Self {
            name,
            sort_name,
            is_dir,
            path,
            insert_prefix,
            match_quality,
        }
    }

    fn sort_bucket(&self) -> u8 {
        match (self.name == "..", self.match_quality, self.is_dir) {
            (true, _, _) => 4,
            (false, MatchQuality::Prefix, true) => 0,
            (false, MatchQuality::Prefix, false) => 1,
            (false, MatchQuality::Contains, true) => 2,
            (false, MatchQuality::Contains, false) => 3,
        }
    }

    fn into_completion_item(
        self,
        range: tower_lsp::lsp_types::Range,
        settings: &CompiledSettings,
    ) -> CompletionItem {
        let Self {
            name,
            sort_name,
            is_dir,
            path,
            insert_prefix,
            match_quality,
        } = self;
        let annotation = if is_dir { "Directory" } else { "File" }.to_string();
        let sort_bucket = match (name == "..", match_quality, is_dir) {
            (true, _, _) => 4,
            (false, MatchQuality::Prefix, true) => 0,
            (false, MatchQuality::Prefix, false) => 1,
            (false, MatchQuality::Contains, true) => 2,
            (false, MatchQuality::Contains, false) => 3,
        };
        let insert_text = if is_dir {
            format!(
                "{0}{1}{2}",
                insert_prefix,
                name,
                settings.directory_suffix()
            )
        } else {
            format!("{insert_prefix}{name}")
        };
        let label = if is_dir {
            format!("{name}/")
        } else {
            name.clone()
        };

        CompletionItem {
            label,
            kind: Some(if is_dir {
                CompletionItemKind::FOLDER
            } else {
                CompletionItemKind::FILE
            }),
            sort_text: Some(format!("{sort_bucket}{sort_name}")),
            filter_text: Some(name.clone()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(annotation.clone()),
            }),
            detail: Some(annotation.clone()),
            documentation: Some(fallback_completion_documentation(
                annotation.as_str(),
                name.as_str(),
            )),
            data: path.as_deref().and_then(|path| {
                serde_json::to_value(completion_item_data(
                    path,
                    is_dir,
                    annotation.as_str(),
                    name.as_str(),
                ))
                .ok()
            }),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: insert_text,
            })),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            command: (is_dir && settings.should_retrigger_after_directory_completion()).then(
                || {
                    Command::new(
                        "Trigger path completions".to_string(),
                        RETRIGGER_COMPLETION_COMMAND.to_string(),
                        None,
                    )
                },
            ),
            ..CompletionItem::default()
        }
    }
}

fn compare_candidates(left: &Candidate, right: &Candidate) -> Ordering {
    left.sort_bucket()
        .cmp(&right.sort_bucket())
        .then_with(|| left.sort_name.cmp(&right.sort_name))
}

fn read_directory_candidates(base: &ResolvedBase, settings: &CompiledSettings) -> Vec<Candidate> {
    let show_hidden = base.prefix.starts_with('.');
    let mut candidates = Vec::new();

    if !settings.disable_up_one_folder()
        && synthetic_parent_matches_prefix(base.prefix.as_str())
        && parent_candidate_dir(base).is_some()
    {
        candidates.push(Candidate::new(
            "..".to_string(),
            true,
            parent_candidate_dir(base),
            base.insert_prefix.clone(),
            MatchQuality::Prefix,
        ));
    }

    let Ok(entries) = fs::read_dir(&base.target_dir) else {
        return candidates;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        let Some(match_quality) = match_quality(name.as_str(), base.prefix.as_str()) else {
            continue;
        };

        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        candidates.push(Candidate::new(
            name,
            is_dir,
            Some(entry.path()),
            base.insert_prefix.clone(),
            match_quality,
        ));
    }

    candidates
}

fn synthetic_parent_matches_prefix(prefix: &str) -> bool {
    prefix == "." || prefix == ".."
}

fn match_quality(name: &str, prefix: &str) -> Option<MatchQuality> {
    if name.starts_with(prefix) {
        Some(MatchQuality::Prefix)
    } else if prefix.starts_with('.') {
        None
    } else if name.contains(prefix) {
        Some(MatchQuality::Contains)
    } else {
        None
    }
}

#[must_use]
pub fn path_completion_response(
    text: &str,
    position: Position,
    syntax: Option<&SyntaxSnapshot>,
    document_path: Option<&Path>,
    workspace_roots: &WorkspaceRoots,
    allow_empty_token: bool,
    settings: &CompiledSettings,
) -> Option<CompletionResponse> {
    let request = CompletionRequest {
        text,
        position,
        syntax,
        document_path,
        workspace_roots,
        is_manual_trigger: allow_empty_token,
        trigger_character: None,
        settings,
    };
    PathSenseEngine.complete(&request)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;
    use tower_lsp::lsp_types::Position;

    use crate::context::extract_completion_context;
    use crate::resolver::ResolvedBase;
    use crate::settings::{CompiledSettings, PathSenseSettings};
    use crate::syntax::SyntaxState;

    use super::*;

    fn settings(directory_suffix: &str) -> CompiledSettings {
        PathSenseSettings {
            directory_suffix: directory_suffix.to_string(),
            ..PathSenseSettings::default()
        }
        .into()
    }

    fn extract(
        text: &str,
        position: Position,
        language_id: &str,
        document_path: Option<&Path>,
        allow_empty_token: bool,
    ) -> CompletionContext {
        let syntax = SyntaxState::new(language_id, text);
        let snapshot = syntax.as_ref().map(SyntaxState::snapshot);
        extract_completion_context(
            text,
            position,
            snapshot.as_ref(),
            document_path,
            allow_empty_token,
            None,
        )
        .expect("context")
    }

    fn base(target_dir: PathBuf, prefix: &str) -> ResolvedBase {
        ResolvedBase {
            target_dir,
            boundary_root: None,
            prefix: prefix.to_string(),
            insert_prefix: String::new(),
        }
    }

    #[test]
    fn candidate_sorting_prefers_directories_then_files() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("b_dir")).unwrap();
        std::fs::create_dir_all(tmp.path().join("a_dir")).unwrap();
        std::fs::write(tmp.path().join("b.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();

        let mut candidates =
            read_directory_candidates(&base(tmp.path().to_path_buf(), ""), &settings("/"));
        candidates.sort_by(compare_candidates);
        let names: Vec<_> = candidates
            .into_iter()
            .map(|candidate| (candidate.is_dir, candidate.name))
            .collect();

        assert_eq!(
            names,
            vec![
                (true, "a_dir".to_string()),
                (true, "b_dir".to_string()),
                (false, "a.txt".to_string()),
                (false, "b.txt".to_string()),
            ]
        );
    }

    #[test]
    fn candidate_sorting_prefers_prefix_matches_before_contains_matches() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("result_dir")).unwrap();
        std::fs::create_dir_all(tmp.path().join("feature_dir")).unwrap();
        std::fs::write(tmp.path().join("report.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("feature.txt"), "x").unwrap();

        let mut candidates =
            read_directory_candidates(&base(tmp.path().to_path_buf(), "re"), &settings("/"));
        candidates.sort_by(compare_candidates);
        let names: Vec<_> = candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();

        assert_eq!(
            names,
            vec![
                "result_dir".to_string(),
                "report.txt".to_string(),
                "feature_dir".to_string(),
                "feature.txt".to_string(),
            ]
        );
    }

    #[test]
    fn synthetic_parent_candidate_uses_lowest_sort_text() {
        let item = Candidate::new(
            "..".to_string(),
            true,
            None,
            String::new(),
            MatchQuality::Prefix,
        )
        .into_completion_item(tower_lsp::lsp_types::Range::default(), &settings("/"));

        assert_eq!(item.sort_text.as_deref(), Some("4.."));
    }

    #[test]
    fn hidden_entries_are_filtered_unless_prefix_starts_with_dot() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join(".hidden"), "x").unwrap();
        std::fs::write(tmp.path().join("main.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("visible"), "x").unwrap();

        let candidates =
            read_directory_candidates(&base(tmp.path().to_path_buf(), ""), &settings("/"));
        let mut names: Vec<_> = candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["main.rs".to_string(), "visible".to_string()]);

        let candidates =
            read_directory_candidates(&base(tmp.path().to_path_buf(), "."), &settings("/"));
        let mut names: Vec<_> = candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["..".to_string(), ".hidden".to_string()]);
    }

    #[test]
    fn synthetic_parent_candidate_only_matches_dot_or_double_dot_prefix() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("nested")).unwrap();

        let empty = read_directory_candidates(&base(tmp.path().to_path_buf(), ""), &settings("/"));
        assert!(empty.iter().all(|candidate| candidate.name != ".."));

        let single_dot =
            read_directory_candidates(&base(tmp.path().to_path_buf(), "."), &settings("/"));
        assert!(single_dot.iter().any(|candidate| candidate.name == ".."));

        let double_dot =
            read_directory_candidates(&base(tmp.path().to_path_buf(), ".."), &settings("/"));
        assert!(double_dot.iter().any(|candidate| candidate.name == ".."));
    }

    #[test]
    fn completion_items_attach_directory_suffix() {
        let candidate = Candidate::new(
            "src".to_string(),
            true,
            None,
            String::new(),
            MatchQuality::Prefix,
        );
        let item =
            candidate.into_completion_item(tower_lsp::lsp_types::Range::default(), &settings("/"));
        assert_eq!(item.label, "src/");
        assert!(matches!(item.kind, Some(CompletionItemKind::FOLDER)));
        assert_eq!(item.detail.as_deref(), Some("Directory"));
        assert_eq!(
            item.label_details
                .as_ref()
                .and_then(|details| details.description.as_deref()),
            Some("Directory")
        );
        assert_eq!(
            item.command
                .as_ref()
                .map(|command| command.command.as_str()),
            Some("editor::ShowCompletions")
        );
    }

    #[test]
    fn completion_items_include_resolve_data_for_real_directory_candidates() {
        let candidate_path = PathBuf::from("/tmp/src");
        let candidate = Candidate::new(
            "src".to_string(),
            true,
            Some(candidate_path.clone()),
            String::new(),
            MatchQuality::Prefix,
        );

        let item =
            candidate.into_completion_item(tower_lsp::lsp_types::Range::default(), &settings("/"));
        let data = item.data.expect("completion data");

        assert_eq!(
            data["path"].as_str(),
            Some(candidate_path.to_string_lossy().as_ref())
        );
        assert_eq!(data["kind"].as_str(), Some("directory"));
    }

    #[test]
    fn completion_items_include_resolve_data_for_real_file_candidates() {
        let candidate_path = PathBuf::from("/tmp/src/main.rs");
        let candidate = Candidate::new(
            "main.rs".to_string(),
            false,
            Some(candidate_path.clone()),
            String::new(),
            MatchQuality::Prefix,
        );

        let item =
            candidate.into_completion_item(tower_lsp::lsp_types::Range::default(), &settings("/"));
        let data = item.data.expect("completion data");

        assert_eq!(
            data["path"].as_str(),
            Some(candidate_path.to_string_lossy().as_ref())
        );
        assert_eq!(data["kind"].as_str(), Some("file"));
    }

    #[test]
    fn tilde_context_preserves_home_prefix_in_insert_text() {
        let candidate = Candidate::new(
            "Documents".to_string(),
            true,
            None,
            "~/".to_string(),
            MatchQuality::Prefix,
        );
        let item =
            candidate.into_completion_item(tower_lsp::lsp_types::Range::default(), &settings("/"));
        let tower_lsp::lsp_types::CompletionTextEdit::Edit(edit) =
            item.text_edit.expect("text edit")
        else {
            panic!("expected edit text edit");
        };
        assert_eq!(edit.new_text, "~/Documents/");
    }

    #[test]
    fn directory_suffix_can_be_disabled() {
        let candidate = Candidate::new(
            "src".to_string(),
            true,
            None,
            String::new(),
            MatchQuality::Prefix,
        );
        let item =
            candidate.into_completion_item(tower_lsp::lsp_types::Range::default(), &settings(""));
        assert_eq!(item.label, "src/");
        assert_eq!(item.detail.as_deref(), Some("Directory"));
        assert!(item.command.is_none());
    }

    #[test]
    fn mapping_prefix_candidates_prefer_virtual_aliases() {
        let settings =
            CompiledSettings::from(PathSenseSettings::from_json_value(serde_json::json!({
                "path_mappings": {
                    "@assets": "/tmp/assets",
                    "$lib": "/tmp/lib"
                }
            })));
        let context = CompletionContext {
            trigger: crate::context::CompletionTrigger::QuotedString,
            allow_empty_token: false,
            document_path: Some(PathBuf::from("/work/project/src/app.ts")),
            raw_token: "@a".to_string(),
            line_prefix: String::new(),
            insert_prefix: String::new(),
            replacement_range: tower_lsp::lsp_types::Range::default(),
            prefix: "@a".to_string(),
        };

        let items = PathSenseEngine::items_for_mapping_prefixes(&context, &settings);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "@assets/");
    }

    #[test]
    fn synthetic_parent_candidate_can_be_disabled() {
        let tmp = tempdir().expect("tempdir");
        let settings = PathSenseSettings {
            disable_up_one_folder: true,
            ..PathSenseSettings::default()
        }
        .into();
        let candidates = read_directory_candidates(&base(tmp.path().to_path_buf(), "."), &settings);
        assert!(candidates.is_empty());
    }

    #[test]
    fn file_path_descents_do_not_return_completion_items() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let src = project.join("src");
        std::fs::create_dir_all(&src).expect("mkdir src");
        std::fs::write(src.join("main.rs"), "fn main() {}").expect("write main");

        let text = "path: ./src/main.rs/";
        let document_path = project.join("config.yaml");
        std::fs::write(&document_path, text).expect("write config");

        let context = extract(
            text,
            Position::new(0, 20),
            "YAML",
            Some(document_path.as_path()),
            false,
        );
        let workspace_roots = WorkspaceRoots {
            internal_worktree_root: Some(project.clone()),
            lsp_roots: Vec::new(),
        };
        let settings = CompiledSettings::default();

        let items = PathSenseEngine::items_for_context(&context, &workspace_roots, &settings);
        assert!(items.is_empty());

        let syntax = SyntaxState::new("YAML", text);
        let snapshot = syntax.as_ref().map(SyntaxState::snapshot);
        let request = CompletionRequest {
            text,
            position: Position::new(0, 20),
            syntax: snapshot.as_ref(),
            document_path: Some(document_path.as_path()),
            workspace_roots: &workspace_roots,
            is_manual_trigger: true,
            trigger_character: None,
            settings: &settings,
        };
        assert!(PathSenseEngine.complete(&request).is_none());
    }

    #[test]
    fn auto_trigger_respects_min_word_length() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let src = project.join("src");
        std::fs::create_dir_all(&project).expect("mkdir project");
        std::fs::write(project.join("readme.md"), "x").expect("write readme");
        std::fs::write(project.join("release.toml"), "x").expect("write release");
        std::fs::create_dir_all(&src).expect("mkdir src");
        let document_path = src.join("config.toml");
        std::fs::write(&document_path, "path = \"re\"").expect("write config");

        let text = "path = \"re\"";
        let syntax = SyntaxState::new("TOML", text);
        let snapshot = syntax.as_ref().map(SyntaxState::snapshot);
        let workspace_roots = WorkspaceRoots {
            internal_worktree_root: Some(project),
            lsp_roots: Vec::new(),
        };
        let settings =
            CompiledSettings::from(PathSenseSettings::from_json_value(serde_json::json!({
                "min_auto_trigger_word_length": 3
            })));

        let auto_request = CompletionRequest {
            text,
            position: Position::new(0, 10),
            syntax: snapshot.as_ref(),
            document_path: Some(document_path.as_path()),
            workspace_roots: &workspace_roots,
            is_manual_trigger: false,
            trigger_character: None,
            settings: &settings,
        };
        assert!(PathSenseEngine.complete(&auto_request).is_none());

        let manual_request = CompletionRequest {
            is_manual_trigger: true,
            ..auto_request
        };
        let Some(CompletionResponse::Array(items)) = PathSenseEngine.complete(&manual_request)
        else {
            panic!("expected completion items");
        };
        let labels = items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"readme.md"));
        assert!(labels.contains(&"release.toml"));
    }

    #[test]
    fn explicit_path_roots_bypass_min_word_length() {
        let settings =
            CompiledSettings::from(PathSenseSettings::from_json_value(serde_json::json!({
                "path_mappings": {
                    "@assets": "${workspace}/assets"
                }
            })));

        for raw_token in ["~", "~/", "/", "./", "../", "@assets", "@a"] {
            let context = CompletionContext {
                trigger: crate::context::CompletionTrigger::QuotedString,
                allow_empty_token: false,
                document_path: Some(PathBuf::from("/work/project/src/app.ts")),
                raw_token: raw_token.to_string(),
                line_prefix: String::new(),
                insert_prefix: String::new(),
                replacement_range: tower_lsp::lsp_types::Range::default(),
                prefix: String::new(),
            };
            let request = CompletionRequest {
                text: "",
                position: Position::new(0, 0),
                syntax: None,
                document_path: context.document_path.as_deref(),
                workspace_roots: &WorkspaceRoots::default(),
                is_manual_trigger: false,
                trigger_character: None,
                settings: &settings,
            };

            assert!(!should_suppress_for_min_auto_trigger(
                &request,
                &context,
                settings.normalized_path_mapping_keys(),
            ));
        }
    }

    #[test]
    fn slash_trigger_bypasses_min_word_length_for_empty_next_segment() {
        let settings =
            CompiledSettings::from(PathSenseSettings::from_json_value(serde_json::json!({
                "min_auto_trigger_word_length": 3
            })));
        let context = CompletionContext {
            trigger: crate::context::CompletionTrigger::QuotedString,
            allow_empty_token: false,
            document_path: Some(PathBuf::from("/work/project/src/app.ts")),
            raw_token: "modules/".to_string(),
            line_prefix: String::new(),
            insert_prefix: String::new(),
            replacement_range: tower_lsp::lsp_types::Range::default(),
            prefix: String::new(),
        };
        let request = CompletionRequest {
            text: "",
            position: Position::new(0, 0),
            syntax: None,
            document_path: context.document_path.as_deref(),
            workspace_roots: &WorkspaceRoots::default(),
            is_manual_trigger: false,
            trigger_character: Some('/'),
            settings: &settings,
        };

        assert!(!should_suppress_for_min_auto_trigger(
            &request,
            &context,
            settings.normalized_path_mapping_keys(),
        ));
    }
}
