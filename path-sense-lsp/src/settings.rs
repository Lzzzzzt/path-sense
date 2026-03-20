use std::collections::BTreeMap;

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

    #[must_use]
    pub fn normalized_path_mapping_keys(&self) -> Vec<String> {
        self.path_mappings
            .keys()
            .map(|key| {
                if key == "/" {
                    key.clone()
                } else {
                    key.trim_end_matches('/').to_string()
                }
            })
            .collect()
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
}
