use std::path::{Path, PathBuf};

use globset::{Glob, GlobSetBuilder};

use crate::context::CompletionContext;
use crate::settings::{
    ConditionalMapping, MappingTargets, PathMapping, PathSenseSettings, SlashRoot,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceRoots {
    pub internal_worktree_root: Option<PathBuf>,
    pub lsp_roots: Vec<PathBuf>,
}

impl WorkspaceRoots {
    #[must_use]
    pub fn workspace_root_for_document(&self, document_path: Option<&Path>) -> Option<PathBuf> {
        if let Some(root) = &self.internal_worktree_root {
            return Some(root.clone());
        }

        let document_path = document_path?;
        let mut matching_roots = self
            .lsp_roots
            .iter()
            .filter(|root| document_path.starts_with(root.as_path()))
            .collect::<Vec<_>>();
        matching_roots.sort_by_key(|root| root.components().count());
        matching_roots
            .last()
            .map(|root| (*root).clone())
            .or_else(|| self.lsp_roots.first().cloned())
    }

    #[must_use]
    pub fn relative_document_path(&self, document_path: &Path) -> Option<String> {
        let workspace_root = self.workspace_root_for_document(Some(document_path))?;
        let relative = document_path.strip_prefix(&workspace_root).ok()?;
        Some(path_to_forward_slashes(relative))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBase {
    pub target_dir: PathBuf,
    pub boundary_root: Option<PathBuf>,
    pub prefix: String,
    pub insert_prefix: String,
}

#[must_use]
pub fn resolve_bases(
    context: &CompletionContext,
    workspace_roots: &WorkspaceRoots,
    settings: &PathSenseSettings,
) -> Vec<ResolvedBase> {
    if document_is_ignored(context.document_path.as_deref(), workspace_roots, settings) {
        return Vec::new();
    }

    let mapping_bases = resolve_from_mappings(context, workspace_roots, settings);
    if !mapping_bases.is_empty() {
        return mapping_bases;
    }

    resolve_non_mapping_bases(context, workspace_roots, settings)
}

fn resolve_non_mapping_bases(
    context: &CompletionContext,
    workspace_roots: &WorkspaceRoots,
    settings: &PathSenseSettings,
) -> Vec<ResolvedBase> {
    let raw_token = context.raw_token.as_str();

    if raw_token == "~" {
        return expand_home(Path::new("~")).map_or_else(Vec::new, |home_root| {
            vec![ResolvedBase {
                target_dir: home_root.clone(),
                boundary_root: Some(home_root),
                prefix: String::new(),
                insert_prefix: "~/".to_string(),
            }]
        });
    }

    if let Some(home_fragment) = raw_token.strip_prefix("~/") {
        return expand_home(Path::new("~")).map_or_else(Vec::new, |home_root| {
            vec![resolve_virtual_root(
                home_fragment,
                home_root.clone(),
                Some(home_root),
                context.insert_prefix.clone(),
            )]
        });
    }

    if raw_token.starts_with('/') {
        return match settings.slash_root {
            SlashRoot::Workspace => workspace_roots
                .workspace_root_for_document(context.document_path.as_deref())
                .map_or_else(Vec::new, |workspace_root| {
                    vec![resolve_virtual_root(
                        raw_token.trim_start_matches('/'),
                        workspace_root.clone(),
                        Some(workspace_root),
                        context.insert_prefix.clone(),
                    )]
                }),
            SlashRoot::Filesystem => {
                let filesystem_root = PathBuf::from("/");
                vec![resolve_virtual_root(
                    raw_token.trim_start_matches('/'),
                    filesystem_root.clone(),
                    Some(filesystem_root),
                    context.insert_prefix.clone(),
                )]
            }
        };
    }

    let Some(document_dir) = context
        .document_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
    else {
        return Vec::new();
    };

    let (dir_fragment, prefix) = split_relative_fragment(raw_token);
    let target_dir = if dir_fragment.is_empty() {
        document_dir
    } else {
        document_dir.join(dir_fragment)
    };

    vec![ResolvedBase {
        target_dir,
        boundary_root: None,
        prefix: prefix.to_string(),
        insert_prefix: context.insert_prefix.clone(),
    }]
}

fn resolve_from_mappings(
    context: &CompletionContext,
    workspace_roots: &WorkspaceRoots,
    settings: &PathSenseSettings,
) -> Vec<ResolvedBase> {
    let raw_token = context.raw_token.as_str();
    let mapping_entries = normalized_mapping_entries(settings);

    for (normalized_key, mapping) in mapping_entries {
        if raw_token == normalized_key {
            let targets =
                expand_mapping_targets(mapping, context.document_path.as_deref(), workspace_roots);

            if !targets.is_empty() {
                return targets
                    .into_iter()
                    .map(|target| ResolvedBase {
                        target_dir: target.clone(),
                        boundary_root: Some(target),
                        prefix: String::new(),
                        insert_prefix: exact_mapping_insert_prefix(&normalized_key),
                    })
                    .collect();
            }
        }

        let Some(remainder) = raw_token.strip_prefix(normalized_key.as_str()) else {
            continue;
        };
        let path_after_key = if normalized_key == "/" {
            remainder
        } else if let Some(stripped) = remainder.strip_prefix('/') {
            stripped
        } else {
            continue;
        };

        let targets =
            expand_mapping_targets(mapping, context.document_path.as_deref(), workspace_roots);
        if targets.is_empty() {
            continue;
        }

        return targets
            .into_iter()
            .map(|target| {
                resolve_virtual_root(path_after_key, target.clone(), Some(target), String::new())
            })
            .collect();
    }

    Vec::new()
}

fn normalized_mapping_entries(settings: &PathSenseSettings) -> Vec<(String, &PathMapping)> {
    let mut entries = settings
        .path_mappings
        .iter()
        .map(|(key, value)| (normalize_mapping_key(key), value))
        .collect::<Vec<_>>();
    entries.sort_by(|(left, _), (right, _)| {
        right.len().cmp(&left.len()).then_with(|| left.cmp(right))
    });
    entries
}

fn normalize_mapping_key(key: &str) -> String {
    if key == "/" {
        "/".to_string()
    } else {
        key.trim_end_matches('/').to_string()
    }
}

fn exact_mapping_insert_prefix(key: &str) -> String {
    if key == "/" {
        "/".to_string()
    } else {
        format!("{key}/")
    }
}

fn expand_mapping_targets(
    mapping: &PathMapping,
    document_path: Option<&Path>,
    workspace_roots: &WorkspaceRoots,
) -> Vec<PathBuf> {
    let variables = PathVariables::new(document_path, workspace_roots);

    match mapping {
        PathMapping::Targets(targets) => expand_targets(targets, &variables),
        PathMapping::Conditional { conditions } => conditions
            .iter()
            .filter(|condition| condition_matches(condition, &variables))
            .flat_map(|condition| expand_targets(&condition.value, &variables))
            .collect(),
    }
}

fn condition_matches(condition: &ConditionalMapping, variables: &PathVariables) -> bool {
    let Some(document_path) = &variables.relative_document_path else {
        return false;
    };

    glob_matches(&condition.when, document_path)
}

fn expand_targets(targets: &MappingTargets, variables: &PathVariables) -> Vec<PathBuf> {
    targets
        .values()
        .into_iter()
        .filter_map(|value| resolve_mapping_target(value, variables))
        .collect()
}

fn resolve_mapping_target(value: &str, variables: &PathVariables) -> Option<PathBuf> {
    let expanded = variables.expand(value);
    let expanded_path = Path::new(expanded.as_str());

    if let Some(home) = expand_home(expanded_path) {
        return Some(home);
    }

    if expanded_path.is_absolute() {
        return Some(expanded_path.to_path_buf());
    }

    if let Some(workspace_root) = &variables.workspace_root {
        return Some(workspace_root.join(expanded_path));
    }

    variables
        .file_dirname
        .as_ref()
        .map(|file_dirname| file_dirname.join(expanded_path))
}

fn resolve_virtual_root(
    fragment: &str,
    root: PathBuf,
    boundary_root: Option<PathBuf>,
    insert_prefix: String,
) -> ResolvedBase {
    let (dir_fragment, prefix) = split_relative_fragment(fragment);
    let target_dir = if dir_fragment.is_empty() {
        root
    } else {
        root.join(dir_fragment)
    };

    ResolvedBase {
        target_dir,
        boundary_root,
        prefix: prefix.to_string(),
        insert_prefix,
    }
}

fn split_relative_fragment(fragment: &str) -> (&str, &str) {
    match fragment.rfind('/') {
        Some(index) => (&fragment[..=index], &fragment[index + 1..]),
        None => ("", fragment),
    }
}

#[must_use]
pub fn document_is_ignored(
    document_path: Option<&Path>,
    workspace_roots: &WorkspaceRoots,
    settings: &PathSenseSettings,
) -> bool {
    if settings.ignored_files_patterns.is_empty() {
        return false;
    }

    let Some(document_path) = document_path else {
        return false;
    };
    let candidate = workspace_roots
        .relative_document_path(document_path)
        .unwrap_or_else(|| path_to_forward_slashes(document_path));

    settings
        .ignored_files_patterns
        .iter()
        .any(|pattern| glob_matches(pattern, candidate.as_str()))
}

#[must_use]
pub fn parent_candidate_dir(base: &ResolvedBase) -> Option<PathBuf> {
    let parent = base.target_dir.parent()?.to_path_buf();
    let Some(boundary_root) = &base.boundary_root else {
        return Some(parent);
    };

    if base.target_dir == *boundary_root || !parent.starts_with(boundary_root) {
        return None;
    }

    Some(parent)
}

pub fn expand_home(fragment: &Path) -> Option<PathBuf> {
    let fragment = fragment.to_str()?;
    let remainder = if fragment == "~" {
        ""
    } else {
        fragment.strip_prefix("~/")?
    };
    let home_dir = std::env::var_os("HOME").map(PathBuf::from)?;
    if remainder.is_empty() {
        Some(home_dir)
    } else {
        Some(home_dir.join(remainder))
    }
}

fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let mut builder = GlobSetBuilder::new();
    let Ok(glob) = Glob::new(pattern) else {
        return false;
    };
    builder.add(glob);
    let Ok(globset) = builder.build() else {
        return false;
    };
    globset.is_match(candidate)
}

fn path_to_forward_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Clone, Debug)]
struct PathVariables {
    home: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    folder_root: Option<PathBuf>,
    file_dirname: Option<PathBuf>,
    relative_file_dirname: Option<String>,
    relative_document_path: Option<String>,
}

impl PathVariables {
    fn new(document_path: Option<&Path>, workspace_roots: &WorkspaceRoots) -> Self {
        let file_dirname = document_path.and_then(Path::parent).map(Path::to_path_buf);
        let workspace_root = workspace_roots.workspace_root_for_document(document_path);
        let folder_root = workspace_root.clone();
        let relative_file_dirname = match (file_dirname.as_deref(), workspace_root.as_deref()) {
            (Some(file_dirname), Some(workspace_root)) => file_dirname
                .strip_prefix(workspace_root)
                .ok()
                .map(path_to_forward_slashes),
            _ => None,
        };
        let relative_document_path = document_path
            .and_then(|document_path| workspace_roots.relative_document_path(document_path));

        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            workspace_root,
            folder_root,
            file_dirname,
            relative_file_dirname,
            relative_document_path,
        }
    }

    fn expand(&self, value: &str) -> String {
        let mut expanded = value.to_string();
        expanded = replace_path_variable(&expanded, "${home}", self.home.as_deref());
        expanded = replace_path_variable(&expanded, "${workspace}", self.workspace_root.as_deref());
        expanded = replace_path_variable(&expanded, "${folder}", self.folder_root.as_deref());
        expanded = replace_path_variable(&expanded, "${fileDirname}", self.file_dirname.as_deref());
        expanded = replace_string_variable(
            &expanded,
            "${relativeFileDirname}",
            self.relative_file_dirname.as_deref(),
        );
        expanded
    }
}

fn replace_path_variable(input: &str, variable: &str, value: Option<&Path>) -> String {
    let replacement = value.map_or_else(String::new, |value| {
        value.to_string_lossy().trim_end_matches('/').to_string()
    });
    input.replace(variable, replacement.as_str())
}

fn replace_string_variable(input: &str, variable: &str, value: Option<&str>) -> String {
    input.replace(variable, value.unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::context::CompletionTrigger;
    use crate::settings::PathSenseSettings;
    use tower_lsp::lsp_types::Range;

    fn context(raw_token: &str, document_path: &Path) -> CompletionContext {
        CompletionContext {
            trigger: CompletionTrigger::QuotedString,
            allow_empty_token: false,
            document_path: Some(document_path.to_path_buf()),
            raw_token: raw_token.to_string(),
            line_prefix: String::new(),
            insert_prefix: String::new(),
            replacement_range: Range::default(),
            prefix: raw_token.to_string(),
        }
    }

    fn workspace_roots(root: &Path) -> WorkspaceRoots {
        WorkspaceRoots {
            internal_worktree_root: Some(root.to_path_buf()),
            lsp_roots: Vec::new(),
        }
    }

    #[test]
    fn slash_root_defaults_to_filesystem_root() {
        let document = Path::new("/work/project/src/app.rs");
        let settings = PathSenseSettings::default();
        let resolved = resolve_bases(
            &context("/tmp/fi", document),
            &WorkspaceRoots::default(),
            &settings,
        );

        assert_eq!(resolved[0].target_dir, PathBuf::from("/tmp"));
        assert_eq!(resolved[0].prefix, "fi");
        assert_eq!(resolved[0].boundary_root, Some(PathBuf::from("/")));
    }

    #[test]
    fn slash_root_can_use_workspace_root() {
        let project = Path::new("/work/project");
        let document = project.join("src/app.rs");
        let settings = PathSenseSettings::from_json_value(json!({
            "slash_root": "workspace"
        }));
        let resolved = resolve_bases(
            &context("/src/ma", &document),
            &workspace_roots(project),
            &settings,
        );

        assert_eq!(resolved[0].target_dir, project.join("src"));
        assert_eq!(resolved[0].prefix, "ma");
        assert_eq!(resolved[0].boundary_root, Some(project.to_path_buf()));
    }

    #[test]
    fn exact_tilde_token_keeps_home_insert_prefix() {
        let document = Path::new("/work/project/src/app.rs");
        let resolved = resolve_bases(
            &context("~", document),
            &WorkspaceRoots::default(),
            &PathSenseSettings::default(),
        );

        assert_eq!(resolved[0].prefix, "");
        assert_eq!(resolved[0].insert_prefix, "~/");
        assert!(resolved[0].boundary_root.is_some());
    }

    #[test]
    fn longest_mapping_prefix_wins() {
        let document = Path::new("/work/project/src/app.rs");
        let settings = PathSenseSettings::from_json_value(json!({
            "path_mappings": {
                "/": "/fallback",
                "/test": "/special"
            }
        }));

        let resolved = resolve_bases(
            &context("/test/fi", document),
            &workspace_roots(Path::new("/work/project")),
            &settings,
        );
        assert_eq!(resolved[0].target_dir, PathBuf::from("/special"));
        assert_eq!(resolved[0].prefix, "fi");
    }

    #[test]
    fn conditional_mapping_expands_variables() {
        let project = Path::new("/work/project");
        let document = project.join("src/app.rs");
        let settings = PathSenseSettings::from_json_value(json!({
            "path_mappings": {
                "@assets": {
                    "conditions": [
                        {
                            "when": "src/**",
                            "value": "${workspace}/assets/${relativeFileDirname}"
                        }
                    ]
                }
            }
        }));

        let resolved = resolve_bases(
            &context("@assets", &document),
            &workspace_roots(project),
            &settings,
        );
        assert_eq!(resolved[0].target_dir, project.join("assets/src"));
        assert_eq!(resolved[0].insert_prefix, "@assets/");
    }

    #[test]
    fn ignored_files_patterns_match_relative_document_path() {
        let project = Path::new("/work/project");
        let document = project.join("vendor/lib.rs");
        let settings = PathSenseSettings::from_json_value(json!({
            "ignored_files_patterns": ["vendor/**"]
        }));

        assert!(document_is_ignored(
            Some(document.as_path()),
            &workspace_roots(project),
            &settings
        ));
    }

    #[test]
    fn parent_candidate_is_clamped_to_boundary_root() {
        let base = ResolvedBase {
            target_dir: PathBuf::from("/work/project/src"),
            boundary_root: Some(PathBuf::from("/work/project")),
            prefix: String::new(),
            insert_prefix: String::new(),
        };
        let parent = parent_candidate_dir(&base).expect("parent");
        assert_eq!(parent, PathBuf::from("/work/project"));

        let root_base = ResolvedBase {
            target_dir: PathBuf::from("/work/project"),
            boundary_root: Some(PathBuf::from("/work/project")),
            prefix: String::new(),
            insert_prefix: String::new(),
        };
        assert!(parent_candidate_dir(&root_base).is_none());
    }
}
