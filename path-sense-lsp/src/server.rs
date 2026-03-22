use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, CompletionTriggerKind,
    DidChangeConfigurationParams, InitializeParams, InitializeResult, InitializedParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

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

        let allow_empty_token = params
            .context
            .as_ref()
            .is_none_or(|context| matches!(context.trigger_kind, CompletionTriggerKind::INVOKED));
        let settings = self.settings().await;
        let workspace_roots = self.workspace_roots().await;

        let request = CompletionRequest {
            text: snapshot.text.as_str(),
            position: params.text_document_position.position,
            syntax: snapshot.syntax.as_ref(),
            document_path: snapshot.path.as_deref(),
            workspace_roots: &workspace_roots,
            allow_empty_token,
            settings: settings.as_ref(),
        };
        Ok(self.engine.complete(&request))
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
    let mut characters = vec!["/".to_string(), ".".to_string(), "~".to_string()];
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

pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::completion_trigger_characters;

    #[test]
    fn trigger_characters_include_alias_prefixes_without_duplicates() {
        assert_eq!(
            completion_trigger_characters(&[
                "@".to_string(),
                "$".to_string(),
                "/".to_string(),
                "xx".to_string(),
            ]),
            vec![
                "/".to_string(),
                ".".to_string(),
                "~".to_string(),
                "@".to_string(),
                "$".to_string(),
            ]
        );
    }
}
