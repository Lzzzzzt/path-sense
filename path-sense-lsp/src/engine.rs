use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tower_lsp::lsp_types::{
    Command, CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionResponse,
    CompletionTextEdit, Documentation, InsertTextFormat, MarkupContent, MarkupKind, Position,
    TextEdit,
};

use crate::context::{CompletionContext, OutsideStringsConfig, extract_completion_context};
use crate::resolver::{ResolvedBase, WorkspaceRoots, parent_candidate_dir, resolve_bases};
use crate::settings::PathSenseSettings;

const RETRIGGER_COMPLETION_COMMAND: &str = "editor::ShowCompletions";

#[derive(Default, Debug, Clone, Copy)]
pub struct PathSenseEngine;

pub struct CompletionRequest<'a> {
    pub text: &'a str,
    pub position: Position,
    pub language_id: &'a str,
    pub document_path: Option<&'a Path>,
    pub workspace_roots: &'a WorkspaceRoots,
    pub allow_empty_token: bool,
    pub settings: &'a PathSenseSettings,
}

impl PathSenseEngine {
    #[must_use]
    pub fn complete(&self, request: &CompletionRequest<'_>) -> Option<CompletionResponse> {
        let mapping_keys = request.settings.normalized_path_mapping_keys();
        let outside_strings =
            request
                .settings
                .trigger_outside_strings
                .then_some(OutsideStringsConfig {
                    path_separators: &request.settings.path_separators,
                    mapping_keys: &mapping_keys,
                });
        let context = extract_completion_context(
            request.text,
            request.position,
            request.language_id,
            request.document_path,
            request.allow_empty_token,
            &mapping_keys,
            outside_strings.as_ref(),
        )?;

        if request
            .settings
            .ignored_prefixes
            .iter()
            .any(|prefix| context.line_prefix.ends_with(prefix))
        {
            return Some(CompletionResponse::Array(Vec::new()));
        }

        let bases = resolve_bases(&context, request.workspace_roots, request.settings);
        let items = self.items_for_bases(&bases, context.replacement_range, request.settings);
        Some(CompletionResponse::Array(items))
    }

    pub fn items_for_bases(
        &self,
        bases: &[ResolvedBase],
        replacement_range: tower_lsp::lsp_types::Range,
        settings: &PathSenseSettings,
    ) -> Vec<CompletionItem> {
        let mut deduped = BTreeMap::new();

        for base in bases {
            for candidate in read_directory_candidates(base, settings) {
                let key = (
                    candidate.name.clone(),
                    candidate.is_dir,
                    candidate.insert_prefix.clone(),
                );
                deduped.entry(key).or_insert(candidate);
            }
        }

        let mut candidates = deduped.into_values().collect::<Vec<_>>();
        candidates.sort_by(compare_candidates);
        candidates
            .into_iter()
            .take(200)
            .map(|candidate| candidate.into_completion_item(replacement_range, settings))
            .collect()
    }

    #[must_use]
    pub fn items_for_context(
        &self,
        context: &CompletionContext,
        workspace_roots: &WorkspaceRoots,
        settings: &PathSenseSettings,
    ) -> Vec<CompletionItem> {
        let bases = resolve_bases(context, workspace_roots, settings);
        self.items_for_bases(&bases, context.replacement_range, settings)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Candidate {
    name: String,
    is_dir: bool,
    insert_prefix: String,
}

impl Candidate {
    fn annotation(&self) -> &'static str {
        if self.is_dir { "Directory" } else { "File" }
    }

    fn into_completion_item(
        self,
        range: tower_lsp::lsp_types::Range,
        settings: &PathSenseSettings,
    ) -> CompletionItem {
        let annotation = self.annotation().to_string();
        let insert_text = if self.is_dir {
            format!(
                "{0}{1}{2}",
                self.insert_prefix, self.name, settings.directory_suffix
            )
        } else {
            format!("{}{}", self.insert_prefix, self.name)
        };
        let label = if self.is_dir {
            format!("{}{}", self.name, settings.directory_suffix)
        } else {
            self.name.clone()
        };

        CompletionItem {
            label,
            kind: Some(if self.is_dir {
                CompletionItemKind::FOLDER
            } else {
                CompletionItemKind::FILE
            }),
            sort_text: Some(format!(
                "{}{}",
                if self.is_dir { "0_" } else { "1_" },
                self.name.to_lowercase()
            )),
            filter_text: Some(self.name.clone()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(annotation.clone()),
            }),
            detail: Some(annotation.clone()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("{} path completion for `{}`.", annotation, self.name),
            })),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: insert_text,
            })),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            command: (self.is_dir && settings.should_retrigger_after_directory_completion()).then(
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
    match (left.is_dir, right.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
    }
}

fn read_directory_candidates(base: &ResolvedBase, settings: &PathSenseSettings) -> Vec<Candidate> {
    let show_hidden = base.prefix.starts_with('.');
    let mut candidates = Vec::new();

    if !settings.disable_up_one_folder
        && "..".starts_with(base.prefix.as_str())
        && parent_candidate_dir(base).is_some()
    {
        candidates.push(Candidate {
            name: "..".to_string(),
            is_dir: true,
            insert_prefix: base.insert_prefix.clone(),
        });
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
        if !name.starts_with(base.prefix.as_str()) {
            continue;
        }

        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        candidates.push(Candidate {
            name,
            is_dir,
            insert_prefix: base.insert_prefix.clone(),
        });
    }

    candidates
}

#[must_use]
pub fn path_completion_response(
    text: &str,
    position: Position,
    language_id: &str,
    document_path: Option<&Path>,
    workspace_roots: &WorkspaceRoots,
    allow_empty_token: bool,
    settings: &PathSenseSettings,
) -> Option<CompletionResponse> {
    let request = CompletionRequest {
        text,
        position,
        language_id,
        document_path,
        workspace_roots,
        allow_empty_token,
        settings,
    };
    PathSenseEngine.complete(&request)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use crate::resolver::ResolvedBase;
    use crate::settings::PathSenseSettings;

    use super::*;

    fn settings(directory_suffix: &str) -> PathSenseSettings {
        PathSenseSettings {
            directory_suffix: directory_suffix.to_string(),
            ..PathSenseSettings::default()
        }
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
                (true, "..".to_string()),
                (true, "a_dir".to_string()),
                (true, "b_dir".to_string()),
                (false, "a.txt".to_string()),
                (false, "b.txt".to_string()),
            ]
        );
    }

    #[test]
    fn hidden_entries_are_filtered_unless_prefix_starts_with_dot() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join(".hidden"), "x").unwrap();
        std::fs::write(tmp.path().join("visible"), "x").unwrap();

        let candidates =
            read_directory_candidates(&base(tmp.path().to_path_buf(), ""), &settings("/"));
        let names: Vec<_> = candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        assert_eq!(names, vec!["..".to_string(), "visible".to_string()]);

        let candidates =
            read_directory_candidates(&base(tmp.path().to_path_buf(), "."), &settings("/"));
        let names: Vec<_> = candidates
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        assert_eq!(names, vec!["..".to_string(), ".hidden".to_string()]);
    }

    #[test]
    fn completion_items_attach_directory_suffix() {
        let candidate = Candidate {
            name: "src".to_string(),
            is_dir: true,
            insert_prefix: String::new(),
        };
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
    fn tilde_context_preserves_home_prefix_in_insert_text() {
        let candidate = Candidate {
            name: "Documents".to_string(),
            is_dir: true,
            insert_prefix: "~/".to_string(),
        };
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
        let candidate = Candidate {
            name: "src".to_string(),
            is_dir: true,
            insert_prefix: String::new(),
        };
        let item =
            candidate.into_completion_item(tower_lsp::lsp_types::Range::default(), &settings(""));
        assert_eq!(item.label, "src");
        assert_eq!(item.detail.as_deref(), Some("Directory"));
        assert!(item.command.is_none());
    }

    #[test]
    fn synthetic_parent_candidate_can_be_disabled() {
        let tmp = tempdir().expect("tempdir");
        let settings = PathSenseSettings {
            disable_up_one_folder: true,
            ..PathSenseSettings::default()
        };
        let candidates = read_directory_candidates(&base(tmp.path().to_path_buf(), "."), &settings);
        assert!(candidates.is_empty());
    }
}
