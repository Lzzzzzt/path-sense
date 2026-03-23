use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionOptions, CompletionParams, CompletionResponse, CompletionTriggerKind,
    DidChangeConfigurationParams, InitializeParams, InitializeResult, InitializedParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::completion_documentation::attach_completion_documentation;
use crate::document_store::{DocumentSnapshot, DocumentStore};
use crate::engine::{CompletionRequest, PathSenseEngine};
use crate::resolver::WorkspaceRoots;
use crate::settings::CompiledSettings;

#[derive(Default, Deserialize)]
struct InitializationOptions {
    #[serde(default)]
    _path_sense_internal: InternalInitializationOptions,
}

#[derive(Default, Deserialize)]
struct InternalInitializationOptions {
    worktree_root: Option<PathBuf>,
    #[serde(default)]
    alias_trigger_characters: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CompletionTriggerState {
    allow_empty_token: bool,
    is_auto_trigger: bool,
    trigger_character: Option<char>,
}

pub struct Backend {
    client: Client,
    documents: Arc<RwLock<DocumentStore>>,
    settings: Arc<RwLock<Arc<CompiledSettings>>>,
    workspace_roots: Arc<RwLock<WorkspaceRoots>>,
    engine: PathSenseEngine,
}

impl Backend {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(RwLock::new(DocumentStore::default())),
            settings: Arc::new(RwLock::new(Arc::new(CompiledSettings::default()))),
            workspace_roots: Arc::new(RwLock::new(WorkspaceRoots::default())),
            engine: PathSenseEngine,
        }
    }

    async fn snapshot_for_uri(&self, uri: &Url) -> Option<DocumentSnapshot> {
        self.documents.read().await.snapshot(uri)
    }

    async fn settings(&self) -> Arc<CompiledSettings> {
        let settings = self.settings.read().await;
        Arc::clone(&settings)
    }

    async fn workspace_roots(&self) -> WorkspaceRoots {
        self.workspace_roots.read().await.clone()
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let initialization_options = params
            .initialization_options
            .clone()
            .and_then(|value| serde_json::from_value::<InitializationOptions>(value).ok())
            .unwrap_or_default();
        *self.workspace_roots.write().await = WorkspaceRoots {
            internal_worktree_root: initialization_options._path_sense_internal.worktree_root,
            lsp_roots: roots_from_initialize_params(&params),
        };

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(completion_trigger_characters(
                        initialization_options
                            ._path_sense_internal
                            .alias_trigger_characters
                            .as_slice(),
                    )),
                    ..CompletionOptions::default()
                }),
                ..ServerCapabilities::default()
            },
            server_info: Some(tower_lsp::lsp_types::ServerInfo {
                name: "path-sense-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                tower_lsp::lsp_types::MessageType::INFO,
                "path-sense-lsp initialized",
            )
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: tower_lsp::lsp_types::DidOpenTextDocumentParams) {
        self.documents.write().await.open(
            params.text_document.uri,
            params.text_document.language_id,
            params.text_document.text,
            Some(params.text_document.version),
        );
    }

    async fn did_change(&self, params: tower_lsp::lsp_types::DidChangeTextDocumentParams) {
        self.documents.write().await.apply_changes(
            &params.text_document.uri,
            params.content_changes,
            Some(params.text_document.version),
        );
    }

    async fn did_close(&self, params: tower_lsp::lsp_types::DidCloseTextDocumentParams) {
        self.documents
            .write()
            .await
            .close(&params.text_document.uri);
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        *self.settings.write().await = Arc::new(CompiledSettings::from_json_value(params.settings));
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let Some(snapshot) = self
            .snapshot_for_uri(&params.text_document_position.text_document.uri)
            .await
        else {
            return Ok(None);
        };

        let trigger = completion_trigger_state(params.context.as_ref());
        let settings = self.settings().await;
        let workspace_roots = self.workspace_roots().await;

        let request = CompletionRequest {
            text: snapshot.text.as_str(),
            position: params.text_document_position.position,
            syntax: snapshot.syntax.as_ref(),
            document_path: snapshot.path.as_deref(),
            workspace_roots: &workspace_roots,
            allow_empty_token: trigger.allow_empty_token,
            is_auto_trigger: trigger.is_auto_trigger,
            trigger_character: trigger.trigger_character,
            settings: settings.as_ref(),
        };
        Ok(self.engine.complete(&request))
    }

    async fn completion_resolve(&self, mut params: CompletionItem) -> Result<CompletionItem> {
        attach_completion_documentation(&mut params);
        Ok(params)
    }
}

fn roots_from_initialize_params(params: &InitializeParams) -> Vec<PathBuf> {
    let mut roots = params
        .workspace_folders
        .as_ref()
        .map(|folders| {
            folders
                .iter()
                .filter_map(|folder| folder.uri.to_file_path().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if roots.is_empty()
        && let Some(root_uri) = &params.root_uri
        && let Ok(root_path) = root_uri.to_file_path()
    {
        roots.push(root_path);
    }

    roots
}

fn completion_trigger_characters(alias_trigger_characters: &[String]) -> Vec<String> {
    let mut characters = word_trigger_characters();
    characters.extend(["/".to_string(), ".".to_string(), "~".to_string()]);
    let mut seen = characters.iter().cloned().collect::<BTreeSet<_>>();

    for character in alias_trigger_characters {
        if character.chars().count() != 1 {
            continue;
        }
        if seen.insert(character.clone()) {
            characters.push(character.clone());
        }
    }

    characters
}

fn word_trigger_characters() -> Vec<String> {
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
        .chars()
        .map(|character| character.to_string())
        .collect()
}

fn completion_trigger_state(
    context: Option<&tower_lsp::lsp_types::CompletionContext>,
) -> CompletionTriggerState {
    let allow_empty_token = context
        .is_none_or(|context| matches!(context.trigger_kind, CompletionTriggerKind::INVOKED));

    CompletionTriggerState {
        allow_empty_token,
        is_auto_trigger: !allow_empty_token,
        trigger_character: context
            .and_then(|context| context.trigger_character.as_deref())
            .and_then(|character| character.chars().next()),
    }
}

pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{CompletionContext, CompletionTriggerKind};

    use super::completion_trigger_state;
    use super::completion_trigger_characters;

    #[test]
    fn trigger_characters_include_alias_prefixes_without_duplicates() {
        let characters = completion_trigger_characters(&[
            "@".to_string(),
            "$".to_string(),
            "/".to_string(),
            "xx".to_string(),
        ]);

        assert!(characters.contains(&"a".to_string()));
        assert!(characters.contains(&"Z".to_string()));
        assert!(characters.contains(&"0".to_string()));
        assert!(characters.contains(&"_".to_string()));
        assert!(characters.contains(&"-".to_string()));
        assert!(characters.contains(&"/".to_string()));
        assert!(characters.contains(&".".to_string()));
        assert!(characters.contains(&"~".to_string()));
        assert!(characters.contains(&"@".to_string()));
        assert!(characters.contains(&"$".to_string()));
        assert_eq!(
            characters
                .iter()
                .filter(|character| character.as_str() == "/")
                .count(),
            1
        );
    }

    #[test]
    fn completion_trigger_state_distinguishes_manual_and_auto_requests() {
        let manual = completion_trigger_state(Some(&CompletionContext {
            trigger_kind: CompletionTriggerKind::INVOKED,
            trigger_character: None,
        }));
        assert!(manual.allow_empty_token);
        assert!(!manual.is_auto_trigger);
        assert_eq!(manual.trigger_character, None);

        let automatic = completion_trigger_state(Some(&CompletionContext {
            trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
            trigger_character: Some("/".to_string()),
        }));
        assert!(!automatic.allow_empty_token);
        assert!(automatic.is_auto_trigger);
        assert_eq!(automatic.trigger_character, Some('/'));
    }

    #[test]
    fn completion_options_enable_resolve_provider() {
        let options = tower_lsp::lsp_types::CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(completion_trigger_characters(&[])),
            ..tower_lsp::lsp_types::CompletionOptions::default()
        };

        assert_eq!(options.resolve_provider, Some(true));
    }
}
