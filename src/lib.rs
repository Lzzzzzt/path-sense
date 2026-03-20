use std::path::Path;

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
        let candidate = Path::new(&worktree.root_path())
            .join("target")
            .join("release")
            .join(SERVER_BINARY_NAME);

        candidate.exists().then(|| Command {
            command: candidate.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    fn command_from_path(worktree: &Worktree) -> Option<Command> {
        worktree.which(SERVER_BINARY_NAME).map(|command| Command {
            command,
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    fn completion_is_directory(completion: &zed::lsp::Completion) -> bool {
        matches!(completion.kind, Some(zed::lsp::CompletionKind::Folder))
    }

    fn completion_annotation(completion: &zed::lsp::Completion) -> Option<String> {
        match completion.kind {
            Some(zed::lsp::CompletionKind::Folder) => Some("Directory".to_string()),
            Some(zed::lsp::CompletionKind::File) => Some("File".to_string()),
            _ => completion.detail.clone(),
        }
    }

    fn completion_label(completion: &zed::lsp::Completion) -> String {
        if Self::completion_is_directory(completion) && !completion.label.ends_with('/') {
            format!("{}/", completion.label)
        } else {
            completion.label.clone()
        }
    }

    fn completion_code_label(completion: &zed::lsp::Completion) -> CodeLabel {
        let label = Self::completion_label(completion);
        let annotation = Self::completion_annotation(completion);
        let code = annotation.as_ref().map_or_else(
            || label.clone(),
            |annotation| format!("{label} {annotation}"),
        );
        let mut spans = vec![CodeLabelSpan::literal(label.clone(), None)];
        if let Some(annotation) = annotation {
            spans.push(CodeLabelSpan::literal(" ", None));
            spans.push(CodeLabelSpan::literal(annotation, None));
        }
        CodeLabel {
            code,
            spans,
            filter_range: (0..label.len()).into(),
        }
    }

    fn initialization_options(worktree: &Worktree, user_options: Option<Value>) -> Value {
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
            }),
        );
        Value::Object(options)
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
        kind: Option<zed::lsp::CompletionKind>,
        detail: Option<&str>,
    ) -> zed::lsp::Completion {
        zed::lsp::Completion {
            label: "src".to_string(),
            label_details: None,
            detail: detail.map(str::to_string),
            kind,
            insert_text_format: None,
        }
    }

    #[test]
    fn directory_labels_always_show_trailing_slash() {
        let completion = completion(Some(zed::lsp::CompletionKind::Folder), Some("Directory"));
        assert_eq!(PathSenseExtension::completion_label(&completion), "src/");
    }

    #[test]
    fn file_annotations_prefer_completion_kind() {
        let completion = completion(Some(zed::lsp::CompletionKind::File), None);
        assert_eq!(
            PathSenseExtension::completion_annotation(&completion).as_deref(),
            Some("File")
        );
    }

    #[test]
    fn annotation_spans_do_not_use_italic_highlight() {
        let completion = completion(Some(zed::lsp::CompletionKind::Folder), Some("Directory"));
        let label = PathSenseExtension::completion_code_label(&completion);
        let Some(CodeLabelSpan::Literal(annotation)) = label.spans.get(2) else {
            panic!("expected annotation literal span");
        };

        assert_eq!(annotation.text, "Directory");
        assert!(annotation.highlight_name.is_none());
    }
}
