use std::path::{Component, Path, PathBuf};

use crate::context::{CompletionContext, mapping_key_supports_prefix_completion};
use crate::settings::{CompiledMappingEntry, CompiledSettings, SlashRoot};

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
        self.lsp_roots
            .iter()
            .filter(|root| document_path.starts_with(root.as_path()))
            .max_by_key(|root| root.components().count())
            .cloned()
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NonMappingBaseKind<'a> {
    ExactHome,
    HomeFragment(&'a str),
    SlashRooted(&'a str),
    WorkspaceRooted(&'a str),
    DocumentRelative(&'a str),
}

#[must_use]
pub fn resolve_bases(
    context: &CompletionContext,
    workspace_roots: &WorkspaceRoots,
    settings: &CompiledSettings,
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
    settings: &CompiledSettings,
) -> Vec<ResolvedBase> {
    match classify_non_mapping_base(context.raw_token.as_str(), settings) {
        NonMappingBaseKind::ExactHome => resolve_exact_home_base(),
        NonMappingBaseKind::HomeFragment(fragment) => {
            resolve_home_fragment_base(fragment, context.insert_prefix.as_str())
        }
        NonMappingBaseKind::SlashRooted(fragment) => {
            resolve_slash_root_base(fragment, context, workspace_roots, settings)
        }
        NonMappingBaseKind::WorkspaceRooted(fragment) => {
            resolve_workspace_root_base(fragment, context, workspace_roots)
        }
        NonMappingBaseKind::DocumentRelative(fragment) => {
            resolve_document_relative_base(fragment, context)
        }
    }
}

fn classify_non_mapping_base<'a>(
    raw_token: &'a str,
    settings: &CompiledSettings,
) -> NonMappingBaseKind<'a> {
    if raw_token == "~" {
        return NonMappingBaseKind::ExactHome;
    }

    if let Some(fragment) = raw_token.strip_prefix("~/") {
        return NonMappingBaseKind::HomeFragment(fragment);
    }

    if let Some(fragment) = raw_token.strip_prefix('/') {
        return NonMappingBaseKind::SlashRooted(fragment);
    }

    if should_resolve_from_workspace_root(raw_token, settings) {
        return NonMappingBaseKind::WorkspaceRooted(raw_token);
    }

    NonMappingBaseKind::DocumentRelative(raw_token)
}

fn should_resolve_from_workspace_root(raw_token: &str, settings: &CompiledSettings) -> bool {
    !raw_token.is_empty()
        && !has_explicit_path_root(raw_token)
        && !settings
            .normalized_path_mapping_keys()
            .iter()
            .any(|key| key == raw_token || mapping_key_supports_prefix_completion(key, raw_token))
}

fn has_explicit_path_root(raw_token: &str) -> bool {
    raw_token.starts_with('~') || raw_token.starts_with('.') || raw_token.starts_with('/')
}

fn resolve_exact_home_base() -> Vec<ResolvedBase> {
    expand_home(Path::new("~")).map_or_else(Vec::new, |home_root| {
        vec![ResolvedBase {
            target_dir: home_root.clone(),
            boundary_root: Some(home_root),
            prefix: String::new(),
            insert_prefix: "~/".to_string(),
        }]
    })
}

fn resolve_home_fragment_base(fragment: &str, insert_prefix: &str) -> Vec<ResolvedBase> {
    expand_home(Path::new("~")).map_or_else(Vec::new, |home_root| {
        vec![resolve_virtual_root(
            fragment,
            home_root.as_path(),
            Some(home_root.clone()),
            insert_prefix.to_string(),
        )]
    })
}

fn resolve_slash_root_base(
    fragment: &str,
    context: &CompletionContext,
    workspace_roots: &WorkspaceRoots,
    settings: &CompiledSettings,
) -> Vec<ResolvedBase> {
    match settings.slash_root() {
        SlashRoot::Workspace => workspace_roots
            .workspace_root_for_document(context.document_path.as_deref())
            .map_or_else(Vec::new, |workspace_root| {
                vec![resolve_virtual_root(
                    fragment,
                    workspace_root.as_path(),
                    Some(workspace_root.clone()),
                    context.insert_prefix.clone(),
                )]
            }),
        SlashRoot::Filesystem => {
            let filesystem_root = PathBuf::from("/");
            vec![resolve_virtual_root(
                fragment,
                filesystem_root.as_path(),
                Some(filesystem_root.clone()),
                context.insert_prefix.clone(),
            )]
        }
    }
}

fn resolve_workspace_root_base(
    fragment: &str,
    context: &CompletionContext,
    workspace_roots: &WorkspaceRoots,
) -> Vec<ResolvedBase> {
    workspace_roots
        .workspace_root_for_document(context.document_path.as_deref())
        .map_or_else(Vec::new, |workspace_root| {
            vec![resolve_virtual_root(
                fragment,
                workspace_root.as_path(),
                Some(workspace_root.clone()),
                context.insert_prefix.clone(),
            )]
        })
}

fn resolve_document_relative_base(
    fragment: &str,
    context: &CompletionContext,
) -> Vec<ResolvedBase> {
    let Some(document_dir) = context
        .document_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
    else {
        return Vec::new();
    };

    // `fragment` has already been classified as document-relative, so it does not carry an
    // explicit leading root marker here.
    let (dir_fragment, prefix) = split_relative_fragment(fragment);
    let target_dir = join_fragment_dir(&document_dir, dir_fragment);

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
    settings: &CompiledSettings,
) -> Vec<ResolvedBase> {
    let raw_token = context.raw_token.as_str();

    for mapping in settings.mapping_entries() {
        let Some(match_kind) = mapping_match(mapping.normalized_key(), raw_token) else {
            continue;
        };
        let targets =
            expand_mapping_targets(mapping, context.document_path.as_deref(), workspace_roots);
        if targets.is_empty() {
            continue;
        }

        return targets
            .into_iter()
            .map(|target| match match_kind {
                MappingMatch::Exact => ResolvedBase {
                    target_dir: target.clone(),
                    boundary_root: Some(target),
                    prefix: String::new(),
                    insert_prefix: exact_mapping_insert_prefix(mapping.normalized_key()),
                },
                MappingMatch::Nested(path_after_key) => resolve_virtual_root(
                    path_after_key,
                    target.as_path(),
                    Some(target.clone()),
                    String::new(),
                ),
            })
            .collect();
    }

    Vec::new()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MappingMatch<'a> {
    Exact,
    Nested(&'a str),
}

fn mapping_match<'a>(normalized_key: &'a str, raw_token: &'a str) -> Option<MappingMatch<'a>> {
    if raw_token == normalized_key {
        return Some(MappingMatch::Exact);
    }

    let remainder = raw_token.strip_prefix(normalized_key)?;
    if normalized_key == "/" {
        // `strip_prefix("/")` already removes the leading slash, so nested fragments under the
        // slash-root mapping must use the remainder as-is instead of stripping another `/`.
        return Some(MappingMatch::Nested(remainder));
    }

    remainder.strip_prefix('/').map(MappingMatch::Nested)
}

fn exact_mapping_insert_prefix(key: &str) -> String {
    if key == "/" {
        "/".to_string()
    } else {
        format!("{key}/")
    }
}

fn expand_mapping_targets(
    mapping: &CompiledMappingEntry,
    document_path: Option<&Path>,
    workspace_roots: &WorkspaceRoots,
) -> Vec<PathBuf> {
    let variables = PathVariables::new(document_path, workspace_roots);
    mapping
        .expand_targets(variables.relative_document_path.as_deref())
        .into_iter()
        .filter_map(|value| resolve_mapping_target(value, &variables))
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
    root: &Path,
    boundary_root: Option<PathBuf>,
    insert_prefix: String,
) -> ResolvedBase {
    // `fragment` already has any explicit root marker removed (`/`, `~/`, mapping key, etc.),
    // so `split_relative_fragment` never needs to handle a standalone `/` here.
    let (dir_fragment, prefix) = split_relative_fragment(fragment);
    let target_dir = join_fragment_dir(root, dir_fragment);

    ResolvedBase {
        target_dir,
        boundary_root,
        prefix: prefix.to_string(),
        insert_prefix,
    }
}

fn split_relative_fragment(fragment: &str) -> (&str, &str) {
    // Callers only pass path fragments after removing explicit roots, so `/` is not a supported
    // standalone input for this helper.
    match fragment.rfind('/') {
        Some(index) => (&fragment[..=index], &fragment[index + 1..]),
        None => ("", fragment),
    }
}

fn join_fragment_dir(root: &Path, dir_fragment: &str) -> PathBuf {
    let normalized_fragment = dir_fragment.trim_end_matches('/');
    if normalized_fragment.is_empty() {
        root.to_path_buf()
    } else {
        root.join(normalized_fragment)
    }
}

#[must_use]
pub fn document_is_ignored(
    document_path: Option<&Path>,
    workspace_roots: &WorkspaceRoots,
    settings: &CompiledSettings,
) -> bool {
    let Some(matcher) = settings.ignored_files_matcher() else {
        return false;
    };

    let Some(document_path) = document_path else {
        return false;
    };
    let candidate = workspace_roots
        .relative_document_path(document_path)
        .unwrap_or_else(|| path_to_forward_slashes(document_path));

    matcher.is_match(candidate.as_str())
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
    let home_dir = std::env::var_os("HOME").map(PathBuf::from)?;

    let mut components = fragment.components();
    match components.next()? {
        Component::Normal(segment) if segment == "~" => {}
        _ => return None,
    }

    let mut expanded = home_dir;
    for component in components {
        match component {
            Component::CurDir => expanded.push("."),
            Component::ParentDir => expanded.push(".."),
            Component::Normal(segment) => expanded.push(segment),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(expanded)
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
    use crate::settings::{CompiledSettings, PathSenseSettings};
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
        let settings = CompiledSettings::default();
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
        let settings = CompiledSettings::from(PathSenseSettings::from_json_value(json!({
            "slash_root": "workspace"
        })));
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
            &CompiledSettings::default(),
        );

        assert_eq!(resolved[0].prefix, "");
        assert_eq!(resolved[0].insert_prefix, "~/");
        assert!(resolved[0].boundary_root.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn expand_home_preserves_non_utf8_path_components() {
        use std::ffi::OsString;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let home_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME should be set for tests");
        let non_utf8_segment = OsString::from_vec(vec![0xFF, b'f', b'o', b'o']);
        let fragment = PathBuf::from(OsString::from_vec(vec![b'~', b'/', 0xFF, b'f', b'o', b'o']));

        let expanded = expand_home(fragment.as_path()).expect("expanded home path");
        let mut expected = home_dir;
        expected.push(non_utf8_segment);

        assert_eq!(
            expanded.as_os_str().as_bytes(),
            expected.as_os_str().as_bytes()
        );
    }

    #[test]
    fn plain_token_resolves_from_workspace_root() {
        let project = Path::new("/work/project");
        let document = project.join("src/app.rs");
        let resolved = resolve_bases(
            &context("rea", &document),
            &workspace_roots(project),
            &CompiledSettings::default(),
        );

        assert_eq!(resolved[0].target_dir, project);
        assert_eq!(resolved[0].prefix, "rea");
        assert_eq!(resolved[0].boundary_root, Some(project.to_path_buf()));
    }

    #[test]
    fn plain_token_without_workspace_root_returns_no_bases() {
        let document = Path::new("/work/project/src/app.rs");
        let resolved = resolve_bases(
            &context("rea", document),
            &WorkspaceRoots::default(),
            &CompiledSettings::default(),
        );

        assert!(resolved.is_empty());
    }

    #[test]
    fn slash_separated_plain_token_continues_from_workspace_root() {
        let project = Path::new("/work/project");
        let document = project.join("nested/config.yaml");
        let resolved = resolve_bases(
            &context("modules/home", &document),
            &workspace_roots(project),
            &CompiledSettings::default(),
        );

        assert_eq!(resolved[0].target_dir, project.join("modules"));
        assert_eq!(resolved[0].prefix, "home");
        assert_eq!(resolved[0].boundary_root, Some(project.to_path_buf()));
    }

    #[test]
    fn longest_mapping_prefix_wins() {
        let document = Path::new("/work/project/src/app.rs");
        let settings = CompiledSettings::from(PathSenseSettings::from_json_value(json!({
            "path_mappings": {
                "/": "/fallback",
                "/test": "/special"
            }
        })));

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
        let settings = CompiledSettings::from(PathSenseSettings::from_json_value(json!({
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
        })));

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
        let settings = CompiledSettings::from(PathSenseSettings::from_json_value(json!({
            "ignored_files_patterns": ["vendor/**"]
        })));

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
