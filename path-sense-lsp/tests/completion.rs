use std::path::Path;

use path_sense_lsp::Backend;
use path_sense_lsp::context::extract_completion_context;
use path_sense_lsp::engine::PathSenseEngine;
use path_sense_lsp::resolver::WorkspaceRoots;
use path_sense_lsp::settings::PathSenseSettings;
use serde_json::json;
use tempfile::tempdir;
use tower::Service;
use tower::ServiceExt;
use tower_lsp::LspService;
use tower_lsp::jsonrpc::Request;
use tower_lsp::lsp_types::Position;

fn position(line: u32, character: u32) -> Position {
    Position::new(line, character)
}

fn default_settings() -> PathSenseSettings {
    PathSenseSettings::default()
}

fn workspace_roots(root: &Path) -> WorkspaceRoots {
    WorkspaceRoots {
        internal_worktree_root: Some(root.to_path_buf()),
        lsp_roots: Vec::new(),
    }
}

async fn initialize_service(
    service: &mut LspService<Backend>,
    project: &Path,
) -> serde_json::Value {
    let initialize = Request::build("initialize")
        .id(1)
        .params(json!({
            "processId": null,
            "rootUri": format!("file://{}", project.display()),
            "capabilities": {},
            "workspaceFolders": [
                {
                    "uri": format!("file://{}", project.display()),
                    "name": "project"
                }
            ],
        }))
        .finish();
    service
        .ready()
        .await
        .expect("service ready")
        .call(initialize)
        .await
        .expect("initialize response")
        .expect("initialize payload")
        .into_parts()
        .1
        .expect("initialize result")
}

async fn open_document(
    service: &mut LspService<Backend>,
    uri: &str,
    language_id: &str,
    text: &str,
) {
    let open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        }))
        .finish();
    let _ = service
        .ready()
        .await
        .expect("service ready")
        .call(open)
        .await
        .expect("open response");
}

async fn completion_items(
    service: &mut LspService<Backend>,
    uri: &str,
    character: u32,
) -> Vec<serde_json::Value> {
    let completion = Request::build("textDocument/completion")
        .id(2)
        .params(json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": character },
            "context": { "triggerKind": 1 }
        }))
        .finish();
    let response = service
        .ready()
        .await
        .expect("service ready")
        .call(completion)
        .await
        .expect("completion response")
        .expect("completion payload");

    let result = response.result().expect("completion result");
    result
        .as_array()
        .cloned()
        .or_else(|| {
            result
                .get("items")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .expect("completion items")
}

#[test]
fn integration_completion_lists_files_in_directory() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("main.rs"), "fn main() {}").expect("write main");
    std::fs::write(src.join("lib.rs"), "pub fn lib() {}").expect("write lib");
    std::fs::write(src.join(".hidden"), "hidden").expect("write hidden");

    let document_path = project.join("config.yaml");
    let text = r#"path: "./src/ma""#;
    let context = extract_completion_context(
        text,
        position(0, 15),
        "YAML",
        Some(document_path.as_path()),
        false,
        None,
    )
    .expect("context");

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();

    assert_eq!(labels, vec!["main.rs"]);
    assert_eq!(items[0].sort_text.as_deref(), Some("1_main.rs"));
}

#[test]
fn integration_completion_shows_hidden_entries_when_prefixed_with_dot() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join(".hidden"), "hidden").expect("write hidden");

    let document_path = project.join("script.sh");
    let text = "cp ./src/.h";
    let context = extract_completion_context(
        text,
        position(0, 11),
        "Shell Script",
        Some(document_path.as_path()),
        false,
        None,
    )
    .expect("context");

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec![".hidden"]);
}

#[test]
fn integration_completion_uses_filesystem_root_for_absolute_paths_by_default() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("config.txt"), "x").expect("write config");

    let document_path = root.join("main.rs");
    let absolute_prefix = format!("{}/co", root.display());
    let text = format!(r#"let p = "{absolute_prefix}""#);
    let cursor = u32::try_from(text.len() - 1).expect("cursor");
    let context = extract_completion_context(
        &text,
        position(0, cursor),
        "Rust",
        Some(document_path.as_path()),
        false,
        None,
    )
    .expect("context");

    let engine = PathSenseEngine;
    let items = engine.items_for_context(&context, &WorkspaceRoots::default(), &default_settings());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "config.txt");
}

#[test]
fn integration_completion_can_use_workspace_root_for_absolute_paths() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join("src")).expect("mkdir src");
    std::fs::write(project.join("config.txt"), "x").expect("write config");

    let document_path = project.join("src/main.rs");
    let text = r#"let p = "/co""#;
    let context = extract_completion_context(
        text,
        position(0, 12),
        "Rust",
        Some(document_path.as_path()),
        false,
        None,
    )
    .expect("context");
    let settings = PathSenseSettings::from_json_value(json!({
        "slash_root": "workspace"
    }));

    let engine = PathSenseEngine;
    let items = engine.items_for_context(&context, &workspace_roots(project.as_path()), &settings);
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["config.txt"]);
}

#[test]
fn integration_completion_supports_tilde_and_path_mappings() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let assets = project.join("assets");
    std::fs::create_dir_all(&assets).expect("mkdir assets");
    std::fs::write(assets.join("logo.svg"), "x").expect("write logo");

    let document_path = project.join("src/app.rs");
    std::fs::create_dir_all(document_path.parent().expect("parent")).expect("mkdir src");

    let text = r#"let p = "@assets""#;
    let settings = PathSenseSettings::from_json_value(json!({
        "path_mappings": {
            "@assets": "${workspace}/assets"
        }
    }));
    let context = extract_completion_context(
        text,
        position(0, 16),
        "Rust",
        Some(document_path.as_path()),
        false,
        None,
    )
    .expect("context");

    let engine = PathSenseEngine;
    let items = engine.items_for_context(&context, &workspace_roots(project.as_path()), &settings);
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["logo.svg"]);

    let tilde_text = r#"let p = "~""#;
    let tilde_context = extract_completion_context(
        tilde_text,
        position(0, 10),
        "Rust",
        Some(document_path.as_path()),
        true,
        None,
    )
    .expect("tilde context");
    let tilde_items = engine.items_for_context(
        &tilde_context,
        &workspace_roots(project.as_path()),
        &settings,
    );
    assert!(!tilde_items.is_empty());
    assert!(tilde_items.iter().all(|item| {
        item.text_edit.as_ref().is_some_and(|edit| match edit {
            tower_lsp::lsp_types::CompletionTextEdit::Edit(edit) => edit.new_text.starts_with("~/"),
            tower_lsp::lsp_types::CompletionTextEdit::InsertAndReplace(edit) => {
                edit.new_text.starts_with("~/")
            }
        })
    }));
}

#[tokio::test]
async fn lsp_completion_round_trip_returns_items() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::create_dir_all(src.join("module_dir")).expect("mkdir module dir");
    std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
    std::fs::write(src.join("module.rs"), "pub fn f() {}\n").expect("write module");

    let document_uri = format!("file://{}", project.join("app.rs").display());
    let document_text = r#"let path = "./src/m";"#;

    let (mut service, _socket) = LspService::new(Backend::new);

    let initialize_result = initialize_service(&mut service, project.as_path()).await;
    let trigger_characters = initialize_result
        .get("capabilities")
        .and_then(|value| value.get("completionProvider"))
        .and_then(|value| value.get("triggerCharacters"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .expect("trigger characters");
    assert!(
        trigger_characters
            .iter()
            .any(|value| value.as_str() == Some("~"))
    );

    open_document(&mut service, &document_uri, "Rust", document_text).await;
    let completion_uri = format!("file://{}", project.join("app.rs").display());
    let items = completion_items(&mut service, &completion_uri, 19).await;
    let labels = items
        .iter()
        .map(|item| item["label"].as_str().expect("label"))
        .collect::<Vec<_>>();

    assert!(labels.contains(&"main.rs"));
    assert!(labels.contains(&"module.rs"));
    let module_dir = items
        .iter()
        .find(|item| item["label"].as_str() == Some("module_dir/"))
        .expect("module dir");
    assert_eq!(
        module_dir["command"]["command"].as_str(),
        Some("editor::ShowCompletions")
    );
}

#[tokio::test]
async fn lsp_workspace_configuration_can_disable_directory_suffix_and_enable_aliases() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let assets = project.join("assets");
    std::fs::create_dir_all(&assets).expect("mkdir assets");
    std::fs::write(assets.join("logo.svg"), "x").expect("write logo");

    let document_uri = format!("file://{}", project.join("app.rs").display());
    let document_text = r#"let path = "@assets";"#;

    let (mut service, _socket) = LspService::new(Backend::new);

    let _ = initialize_service(&mut service, project.as_path()).await;

    let change_configuration = Request::build("workspace/didChangeConfiguration")
        .params(json!({
            "settings": {
                "directory_suffix": "",
                "path_mappings": {
                    "@assets": "${workspace}/assets"
                }
            }
        }))
        .finish();
    let _ = service
        .ready()
        .await
        .expect("service ready")
        .call(change_configuration)
        .await
        .expect("config response");

    open_document(&mut service, &document_uri, "Rust", document_text).await;
    let completion_uri = format!("file://{}", project.join("app.rs").display());
    let items = completion_items(&mut service, &completion_uri, 19).await;
    assert_eq!(items[0]["label"].as_str(), Some("logo.svg"));
}
