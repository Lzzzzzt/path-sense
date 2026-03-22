use std::collections::BTreeMap;

use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SlashRoot {
    #[default]
    Filesystem,
    Workspace,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum MappingTargets {
    One(String),
    Many(Vec<String>),
}

impl MappingTargets {
    #[must_use]
    pub fn values(&self) -> Vec<&str> {
        match self {
            Self::One(value) => vec![value.as_str()],
            Self::Many(values) => values.iter().map(String::as_str).collect(),
        }
    }

    fn into_strings(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ConditionalMapping {
    pub when: String,
    pub value: MappingTargets,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum PathMapping {
    Targets(MappingTargets),
    Conditional { conditions: Vec<ConditionalMapping> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct PathSenseSettings {
    pub slash_root: SlashRoot,
    pub path_mappings: BTreeMap<String, PathMapping>,
    pub trigger_outside_strings: bool,
    pub path_separators: String,
    pub disable_up_one_folder: bool,
    pub ignored_files_patterns: Vec<String>,
    pub ignored_prefixes: Vec<String>,
    pub directory_suffix: String,
}

#[derive(Debug)]
pub struct CompiledSettings {
    raw: PathSenseSettings,
    normalized_path_mapping_keys: Vec<String>,
    mapping_entries: Vec<CompiledMappingEntry>,
    ignored_files_matcher: Option<GlobSet>,
}

#[derive(Debug)]
pub struct CompiledMappingEntry {
    normalized_key: String,
    mapping: CompiledPathMapping,
}

#[derive(Debug)]
enum CompiledPathMapping {
    Targets(Vec<String>),
    Conditional {
        conditions: Vec<CompiledConditionalMapping>,
    },
}

#[derive(Debug)]
pub struct CompiledConditionalMapping {
    matcher: Option<GlobMatcher>,
    values: Vec<String>,
}

impl Default for PathSenseSettings {
    fn default() -> Self {
        Self {
            slash_root: SlashRoot::Filesystem,
            path_mappings: BTreeMap::new(),
            trigger_outside_strings: false,
            path_separators: " \t({[".to_string(),
            disable_up_one_folder: false,
            ignored_files_patterns: Vec::new(),
            ignored_prefixes: Vec::new(),
            directory_suffix: "/".to_string(),
        }
    }
}

impl PathSenseSettings {
    #[must_use]
    pub fn from_json_value(value: Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }

    #[must_use]
    pub fn should_retrigger_after_directory_completion(&self) -> bool {
        self.directory_suffix.contains('/')
    }
}

impl Default for CompiledSettings {
    fn default() -> Self {
        PathSenseSettings::default().into()
    }
}

impl CompiledSettings {
    #[must_use]
    pub fn from_json_value(value: Value) -> Self {
        PathSenseSettings::from_json_value(value).into()
    }

    #[must_use]
    pub fn raw(&self) -> &PathSenseSettings {
        &self.raw
    }

    #[must_use]
    pub fn slash_root(&self) -> SlashRoot {
        self.raw.slash_root
    }

    #[must_use]
    pub fn trigger_outside_strings(&self) -> bool {
        self.raw.trigger_outside_strings
    }

    #[must_use]
    pub fn path_separators(&self) -> &str {
        self.raw.path_separators.as_str()
    }

    #[must_use]
    pub fn disable_up_one_folder(&self) -> bool {
        self.raw.disable_up_one_folder
    }

    #[must_use]
    pub fn ignored_prefixes(&self) -> &[String] {
        &self.raw.ignored_prefixes
    }

    #[must_use]
    pub fn directory_suffix(&self) -> &str {
        self.raw.directory_suffix.as_str()
    }

    #[must_use]
    pub fn should_retrigger_after_directory_completion(&self) -> bool {
        self.raw.should_retrigger_after_directory_completion()
    }

    #[must_use]
    pub fn normalized_path_mapping_keys(&self) -> &[String] {
        &self.normalized_path_mapping_keys
    }

    #[must_use]
    pub fn mapping_entries(&self) -> &[CompiledMappingEntry] {
        &self.mapping_entries
    }

    #[must_use]
    pub fn ignored_files_matcher(&self) -> Option<&GlobSet> {
        self.ignored_files_matcher.as_ref()
    }
}

impl CompiledMappingEntry {
    #[must_use]
    pub fn normalized_key(&self) -> &str {
        self.normalized_key.as_str()
    }

    #[must_use]
    pub fn expand_targets(&self, relative_document_path: Option<&str>) -> Vec<&str> {
        match &self.mapping {
            CompiledPathMapping::Targets(values) => values.iter().map(String::as_str).collect(),
            CompiledPathMapping::Conditional { conditions } => conditions
                .iter()
                .filter(|condition| condition.matches(relative_document_path))
                .flat_map(CompiledConditionalMapping::values)
                .collect(),
        }
    }
}

impl CompiledConditionalMapping {
    #[must_use]
    pub fn matches(&self, relative_document_path: Option<&str>) -> bool {
        relative_document_path
            .zip(self.matcher.as_ref())
            .is_some_and(|(path, matcher)| matcher.is_match(path))
    }

    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.values.iter().map(String::as_str)
    }
}

impl From<PathSenseSettings> for CompiledSettings {
    fn from(raw: PathSenseSettings) -> Self {
        let normalized_path_mapping_keys = raw
            .path_mappings
            .keys()
            .map(|key| normalize_mapping_key(key))
            .collect::<Vec<_>>();
        let mut mapping_entries = raw
            .path_mappings
            .clone()
            .into_iter()
            .map(|(key, mapping)| CompiledMappingEntry {
                normalized_key: normalize_mapping_key(key.as_str()),
                mapping: compile_mapping(mapping),
            })
            .collect::<Vec<_>>();
        mapping_entries.sort_by(|left, right| {
            right
                .normalized_key
                .len()
                .cmp(&left.normalized_key.len())
                .then_with(|| left.normalized_key.cmp(&right.normalized_key))
        });

        Self {
            ignored_files_matcher: build_globset(&raw.ignored_files_patterns),
            raw,
            normalized_path_mapping_keys,
            mapping_entries,
        }
    }
}

fn compile_mapping(mapping: PathMapping) -> CompiledPathMapping {
    match mapping {
        PathMapping::Targets(targets) => CompiledPathMapping::Targets(targets.into_strings()),
        PathMapping::Conditional { conditions } => CompiledPathMapping::Conditional {
            conditions: conditions
                .into_iter()
                .map(|condition| CompiledConditionalMapping {
                    matcher: Glob::new(condition.when.as_str())
                        .ok()
                        .map(|glob| glob.compile_matcher()),
                    values: condition.value.into_strings(),
                })
                .collect(),
        },
    }
}

fn build_globset(patterns: &[String]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut has_pattern = false;

    for pattern in patterns {
        let Ok(glob) = Glob::new(pattern) else {
            continue;
        };
        builder.add(glob);
        has_pattern = true;
    }

    if !has_pattern {
        return None;
    }

    builder.build().ok()
}

fn normalize_mapping_key(key: &str) -> String {
    if key == "/" {
        "/".to_string()
    } else {
        key.trim_end_matches('/').to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_match_linux_macos_plan() {
        let settings = PathSenseSettings::default();
        assert_eq!(settings.slash_root, SlashRoot::Filesystem);
        assert_eq!(settings.path_separators, " \t({[");
        assert_eq!(settings.directory_suffix, "/");
        assert!(!settings.trigger_outside_strings);
        assert!(!settings.disable_up_one_folder);
        assert!(settings.path_mappings.is_empty());
        assert!(settings.ignored_files_patterns.is_empty());
        assert!(settings.ignored_prefixes.is_empty());
    }

    #[test]
    fn parses_custom_settings() {
        let settings = PathSenseSettings::from_json_value(json!({
            "slash_root": "filesystem",
            "path_mappings": {
                "/test": "/tmp/test",
                "@root": {
                    "conditions": [
                        {
                            "when": "src/**",
                            "value": ["${workspace}/assets", "${home}/tmp"]
                        }
                    ]
                }
            },
            "trigger_outside_strings": true,
            "path_separators": " \t(",
            "disable_up_one_folder": true,
            "ignored_files_patterns": ["vendor/**"],
            "ignored_prefixes": ["http://"],
            "directory_suffix": ""
        }));

        assert_eq!(settings.slash_root, SlashRoot::Filesystem);
        assert!(settings.trigger_outside_strings);
        assert_eq!(settings.path_separators, " \t(");
        assert!(settings.disable_up_one_folder);
        assert_eq!(settings.ignored_files_patterns, vec!["vendor/**"]);
        assert_eq!(settings.ignored_prefixes, vec!["http://"]);
        assert_eq!(settings.directory_suffix, "");
        assert!(!settings.should_retrigger_after_directory_completion());
    }

    #[test]
    fn compiled_settings_precomputes_mapping_and_ignores_invalid_globs() {
        let settings = CompiledSettings::from_json_value(json!({
            "path_mappings": {
                "/": "/tmp",
                "/test": {
                    "conditions": [
                        {
                            "when": "src/**",
                            "value": ["${workspace}/assets"]
                        },
                        {
                            "when": "[",
                            "value": ["/ignored"]
                        }
                    ]
                }
            },
            "ignored_files_patterns": ["vendor/**", "["]
        }));

        assert_eq!(
            settings.normalized_path_mapping_keys(),
            &["/".to_string(), "/test".to_string()]
        );
        assert_eq!(settings.mapping_entries()[0].normalized_key(), "/test");
        assert!(
            settings
                .ignored_files_matcher()
                .is_some_and(|matcher| matcher.is_match("vendor/lib.rs"))
        );
        assert_eq!(
            settings.mapping_entries()[0].expand_targets(Some("src/main.rs")),
            vec!["${workspace}/assets"]
        );
        assert!(
            settings.mapping_entries()[0]
                .expand_targets(Some("docs/readme.md"))
                .is_empty()
        );
    }
}
