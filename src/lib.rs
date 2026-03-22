use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use zed_extension_api::{
    self as zed, CodeLabel, CodeLabelSpan, Command, LanguageServerId, Result, Worktree,
    serde_json::{Map, Value, json},
    settings::LspSettings,
};

const LANGUAGE_SERVER_ID: &str = "path-sense";
const SERVER_BINARY_NAME: &str = "path-sense-lsp";

struct PathSenseExtension;

impl PathSenseExtension {
    fn is_path_sense_server(language_server_id: &LanguageServerId) -> bool {
        language_server_id.as_ref() == LANGUAGE_SERVER_ID
    }

    fn command_from_override(worktree: &Worktree) -> Result<Option<Command>> {
        let settings = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)?;
        let Some(binary) = settings.binary else {
            return Ok(None);
        };
        let Some(path) = binary.path else {
            return Ok(None);
        };

        Ok(Some(Command {
            command: path,
            args: binary.arguments.unwrap_or_default(),
            env: binary
                .env
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>(),
        }))
    }

    fn command_from_worktree(worktree: &Worktree) -> Option<Command> {
        let candidate = Self::worktree_binary_path(worktree.root_path().as_str());

        candidate.exists().then(|| Command {
            command: candidate.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    fn worktree_binary_path(root_path: &str) -> PathBuf {
        Path::new(root_path)
            .join("target")
            .join("debug")
            .join(SERVER_BINARY_NAME)
    }

    fn command_from_path(worktree: &Worktree) -> Option<Command> {
        worktree.which(SERVER_BINARY_NAME).map(|command| Command {
            command,
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    fn completion_annotation(completion: &zed::lsp::Completion) -> Option<String> {
        completion
            .label_details
            .as_ref()
            .and_then(|details| details.description.clone().or(details.detail.clone()))
            .or_else(|| completion.detail.clone())
    }

    fn completion_annotation_highlight(completion: &zed::lsp::Completion) -> Option<String> {
        match completion.kind {
            Some(zed::lsp::CompletionKind::Folder) => Some("type".to_string()),
            Some(zed::lsp::CompletionKind::File) => Some("string".to_string()),
            _ => None,
        }
    }

    fn completion_label(completion: &zed::lsp::Completion) -> String {
        completion.label.clone()
    }

    fn completion_code_label(completion: &zed::lsp::Completion) -> CodeLabel {
        let label = Self::completion_label(completion);
        let annotation = Self::completion_annotation(completion);
        let annotation_highlight = Self::completion_annotation_highlight(completion);
        let code = annotation.as_ref().map_or_else(
            || label.clone(),
            |annotation| format!("{label} [{annotation}]"),
        );
        let mut spans = vec![CodeLabelSpan::literal(label.clone(), None)];
        if let Some(annotation) = annotation {
            spans.push(CodeLabelSpan::literal(" ", None));
            spans.push(CodeLabelSpan::literal(
                format!("[{annotation}]"),
                annotation_highlight,
            ));
        }
        CodeLabel {
            code,
            spans,
            filter_range: (0..label.len()).into(),
        }
    }

    fn initialization_options(
        worktree: &Worktree,
        user_options: Option<Value>,
        workspace_settings: Option<&Value>,
    ) -> Value {
        let mut options = match user_options {
            Some(Value::Object(object)) => object,
            Some(other) => {
                let mut object = Map::new();
                object.insert("_user_initialization_options".to_string(), other);
                object
            }
            None => Map::new(),
        };
        options.insert(
            "_path_sense_internal".to_string(),
            json!({
                "worktree_root": worktree.root_path(),
                "alias_trigger_characters": Self::alias_trigger_characters(workspace_settings),
            }),
        );
        Value::Object(options)
    }

    fn alias_trigger_characters(workspace_settings: Option<&Value>) -> Vec<String> {
        let Some(path_mappings) = workspace_settings
            .and_then(Value::as_object)
            .and_then(|settings| settings.get("path_mappings"))
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };

        let mut characters = BTreeSet::new();
        for key in path_mappings.keys() {
            let normalized = Self::normalize_mapping_key(key.as_str());
            let Some(character) = normalized.chars().next() else {
                continue;
            };
            if character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '~') {
                continue;
            }

            characters.insert(character.to_string());
        }

        characters.into_iter().collect()
    }

    fn normalize_mapping_key(key: &str) -> &str {
        if key == "/" {
            "/"
        } else {
            key.trim_end_matches('/')
        }
    }
}

impl zed::Extension for PathSenseExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        if !Self::is_path_sense_server(language_server_id) {
            return Err(format!(
                "unsupported language server id `{}`",
                language_server_id.as_ref()
            ));
        }

        if let Some(command) = Self::command_from_override(worktree)? {
            return Ok(command);
        }

        if let Some(command) = Self::command_from_worktree(worktree) {
            return Ok(command);
        }

        if let Some(command) = Self::command_from_path(worktree) {
            return Ok(command);
        }

        Err(format!(
            "could not find `{SERVER_BINARY_NAME}`. Set `lsp.{LANGUAGE_SERVER_ID}.binary.path`, build `target/debug/{SERVER_BINARY_NAME}` inside the worktree, or add `{SERVER_BINARY_NAME}` to PATH."
        ))
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        if !Self::is_path_sense_server(language_server_id) {
            return Ok(None);
        }

        let settings = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)?;
        Ok(Some(Self::initialization_options(
            worktree,
            settings.initialization_options,
            settings.settings.as_ref(),
        )))
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        if !Self::is_path_sense_server(language_server_id) {
            return Ok(None);
        }

        let settings = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)?;
        Ok(settings.settings)
    }

    fn label_for_completion(
        &self,
        language_server_id: &LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<CodeLabel> {
        if !Self::is_path_sense_server(language_server_id) {
            return None;
        }

        Some(Self::completion_code_label(&completion))
    }
}

zed::register_extension!(PathSenseExtension);

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(
        label: &str,
        kind: Option<zed::lsp::CompletionKind>,
        detail: Option<&str>,
    ) -> zed::lsp::Completion {
        zed::lsp::Completion {
            label: label.to_string(),
            label_details: None,
            detail: detail.map(str::to_string),
            kind,
            insert_text_format: None,
        }
    }

    #[test]
    fn worktree_binary_path_points_to_debug_build() {
        assert_eq!(
            PathSenseExtension::worktree_binary_path("/tmp/demo"),
            PathBuf::from("/tmp/demo/target/debug/path-sense-lsp")
        );
    }

    #[test]
    fn directory_labels_use_server_label_verbatim() {
        let completion = completion(
            "src/",
            Some(zed::lsp::CompletionKind::Folder),
            Some("ignored"),
        );
        assert_eq!(PathSenseExtension::completion_label(&completion), "src/");
    }

    #[test]
    fn annotation_uses_server_detail() {
        let completion = completion("src/", Some(zed::lsp::CompletionKind::File), Some("File"));
        assert_eq!(
            PathSenseExtension::completion_annotation(&completion).as_deref(),
            Some("File")
        );
    }

    #[test]
    fn annotation_spans_use_semantic_highlights() {
        let completion = completion(
            "src/",
            Some(zed::lsp::CompletionKind::Folder),
            Some("Directory"),
        );
        let label = PathSenseExtension::completion_code_label(&completion);
        let Some(CodeLabelSpan::Literal(annotation)) = label.spans.get(2) else {
            panic!("expected annotation literal span");
        };

        assert_eq!(annotation.text, "[Directory]");
        assert_eq!(annotation.highlight_name.as_deref(), Some("type"));
    }

    #[test]
    fn alias_trigger_characters_are_derived_from_path_mappings() {
        let settings = json!({
            "path_mappings": {
                "@assets": "/tmp/assets",
                "$lib": "/tmp/lib",
                "/test": "/tmp/test",
                "plain": "/tmp/plain"
            }
        });

        assert_eq!(
            PathSenseExtension::alias_trigger_characters(Some(&settings)),
            vec!["$".to_string(), "@".to_string()]
        );
    }
}
