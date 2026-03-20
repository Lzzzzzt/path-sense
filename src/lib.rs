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

    fn completion_label(completion: &zed::lsp::Completion) -> String {
        completion.label.clone()
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

        let label = Self::completion_label(&completion);
        Some(CodeLabel {
            code: label.clone(),
            spans: vec![CodeLabelSpan::literal(label.clone(), None)],
            filter_range: (0..label.len()).into(),
        })
    }
}

zed::register_extension!(PathSenseExtension);
