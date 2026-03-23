use std::path::Path;

use path_sense_lsp::Backend;
use path_sense_lsp::context::{CompletionContext, extract_completion_context};
use path_sense_lsp::engine::PathSenseEngine;
use path_sense_lsp::resolver::WorkspaceRoots;
use path_sense_lsp::settings::{CompiledSettings, PathSenseSettings};
use path_sense_lsp::syntax::SyntaxState;
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

fn default_settings() -> CompiledSettings {
    CompiledSettings::default()
}

fn workspace_roots(root: &Path) -> WorkspaceRoots {
    WorkspaceRoots {
        internal_worktree_root: Some(root.to_path_buf()),
        lsp_roots: Vec::new(),
    }
}

fn extract(
    text: &str,
    position: Position,
    language_id: &str,
    document_path: &Path,
    allow_empty_token: bool,
) -> CompletionContext {
    let syntax = SyntaxState::new(language_id, text);
    let snapshot = syntax.as_ref().map(SyntaxState::snapshot);
    extract_completion_context(
        text,
        position,
        snapshot.as_ref(),
        Some(document_path),
        allow_empty_token,
        None,
    )
    .expect("context")
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

async fn change_document(
    service: &mut LspService<Backend>,
    uri: &str,
    version: i32,
    changes: serde_json::Value,
) {
    let change = Request::build("textDocument/didChange")
        .params(json!({
            "textDocument": {
                "uri": uri,
                "version": version
            },
            "contentChanges": changes
        }))
        .finish();
    let _ = service
        .ready()
        .await
        .expect("service ready")
        .call(change)
        .await
        .expect("change response");
}

async fn completion_items(
    service: &mut LspService<Backend>,
    uri: &str,
    character: u32,
) -> Vec<serde_json::Value> {
    completion_items_with_trigger(service, uri, character, 1, None).await
}

async fn completion_items_with_trigger(
    service: &mut LspService<Backend>,
    uri: &str,
    character: u32,
    trigger_kind: u8,
    trigger_character: Option<&str>,
) -> Vec<serde_json::Value> {
    let completion = Request::build("textDocument/completion")
        .id(2)
        .params(json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": character },
            "context": {
                "triggerKind": trigger_kind,
                "triggerCharacter": trigger_character
            }
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
    if result.is_null() {
        return Vec::new();
    }
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

async fn resolve_completion_item(
    service: &mut LspService<Backend>,
    item: serde_json::Value,
) -> serde_json::Value {
    let resolve = Request::build("completionItem/resolve")
        .id(3)
        .params(item)
        .finish();
    let response = service
        .ready()
        .await
        .expect("service ready")
        .call(resolve)
        .await
        .expect("resolve response")
        .expect("resolve payload");

    response.result().expect("resolve result").clone()
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
    let context = extract(
        text,
        position(0, 15),
        "YAML",
        document_path.as_path(),
        false,
    );

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();

    assert_eq!(labels, vec!["main.rs"]);
    assert_eq!(items[0].sort_text.as_deref(), Some("1main.rs"));
}

#[test]
fn integration_completion_supports_contains_matching_with_prefix_priority() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(src.join("result_dir")).expect("mkdir result_dir");
    std::fs::create_dir_all(src.join("feature_dir")).expect("mkdir feature_dir");
    std::fs::write(src.join("report.txt"), "x").expect("write report");
    std::fs::write(src.join("feature.txt"), "x").expect("write feature");

    let document_path = project.join("config.yaml");
    let text = r#"path: "./src/re""#;
    let context = extract(
        text,
        position(0, 15),
        "YAML",
        document_path.as_path(),
        false,
    );

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();

    assert_eq!(
        labels,
        vec!["result_dir/", "report.txt", "feature_dir/", "feature.txt"]
    );
}

#[test]
fn integration_completion_supports_unquoted_yaml_plain_scalars() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("main.rs"), "fn main() {}").expect("write main");

    let document_path = project.join("config.yaml");
    let text = "path: ./src/ma";
    let context = extract(
        text,
        position(0, 14),
        "YAML",
        document_path.as_path(),
        false,
    );

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();

    assert_eq!(labels, vec!["main.rs"]);
    assert_eq!(context.raw_token, "./src/ma");
}

#[test]
fn integration_completion_supports_unquoted_yaml_plain_scalars_without_slash() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    std::fs::create_dir_all(project.join("modules")).expect("mkdir modules");
    std::fs::write(project.join("main.rs"), "fn main() {}").expect("write main");

    let document_path = project.join("config.yaml");
    let text = "src: mo";
    let context = extract(
        text,
        position(0, 7),
        "YAML",
        document_path.as_path(),
        false,
    );

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();

    assert_eq!(labels, vec!["modules/"]);
    assert_eq!(context.raw_token, "mo");
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
    let context = extract(
        text,
        position(0, 11),
        "Shell Script",
        document_path.as_path(),
        false,
    );

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
    let context = extract(
        &text,
        position(0, cursor),
        "Rust",
        document_path.as_path(),
        false,
    );

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
    let context = extract(
        text,
        position(0, 12),
        "Rust",
        document_path.as_path(),
        false,
    );
    let settings = CompiledSettings::from(PathSenseSettings::from_json_value(json!({
        "slash_root": "workspace"
    })));

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
    let settings = CompiledSettings::from(PathSenseSettings::from_json_value(json!({
        "path_mappings": {
            "@assets": "${workspace}/assets"
        }
    })));
    let context = extract(
        text,
        position(0, 16),
        "Rust",
        document_path.as_path(),
        false,
    );

    let engine = PathSenseEngine;
    let items = engine.items_for_context(&context, &workspace_roots(project.as_path()), &settings);
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["logo.svg"]);

    let tilde_text = r#"let p = "~""#;
    let tilde_context = extract(
        tilde_text,
        position(0, 10),
        "Rust",
        document_path.as_path(),
        true,
    );
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

#[test]
fn integration_completion_uses_workspace_root_for_plain_fragments() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(project.join("readme.md"), "x").expect("write readme");
    std::fs::create_dir_all(project.join("resources")).expect("mkdir resources");
    std::fs::write(src.join("read_local.rs"), "x").expect("write local");

    let document_path = src.join("main.rs");
    let text = r#"let p = "re""#;
    let cursor = u32::try_from(text.len() - 1).expect("cursor");
    let context = extract(
        text,
        position(0, cursor),
        "Rust",
        document_path.as_path(),
        false,
    );

    let engine = PathSenseEngine;
    let items = engine.items_for_context(
        &context,
        &workspace_roots(project.as_path()),
        &default_settings(),
    );
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();

    assert!(labels.contains(&"readme.md"));
    assert!(labels.contains(&"resources/"));
    assert!(!labels.contains(&"read_local.rs"));
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
    assert_eq!(
        initialize_result["capabilities"]["textDocumentSync"].as_u64(),
        Some(2)
    );
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
    assert_eq!(
        initialize_result["capabilities"]["completionProvider"]["resolveProvider"].as_bool(),
        Some(true)
    );

    open_document(&mut service, &document_uri, "Rust", document_text).await;
    let items = completion_items(&mut service, &document_uri, 19).await;
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
async fn lsp_completion_resolve_previews_utf8_text_files() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(
        src.join("sample.txt"),
        "first line\nsecond line\nthird line\nfourth line\n",
    )
    .expect("write sample");

    let document_uri = format!("file://{}", project.join("config.yaml").display());
    let document_text = r#"path: "./src/sa""#;

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;
    open_document(&mut service, &document_uri, "YAML", document_text).await;

    let items = completion_items(&mut service, &document_uri, 15).await;
    let sample = items
        .into_iter()
        .find(|item| item["label"].as_str() == Some("sample.txt"))
        .expect("sample item");
    let resolved = resolve_completion_item(&mut service, sample).await;
    let documentation = resolved["documentation"]["value"]
        .as_str()
        .expect("documentation");

    assert!(documentation.contains("first line\nsecond line\nthird line"));
    assert!(documentation.contains("Preview truncated."));
}

#[tokio::test]
async fn lsp_completion_resolve_previews_utf8_bom_and_utf16_files() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(src.join("utf8.txt"), b"\xEF\xBB\xBFalpha\nbeta\n").expect("write utf8");

    let mut utf16_bytes = vec![0xFF, 0xFE];
    utf16_bytes.extend("gamma\ndelta\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(src.join("utf16.txt"), utf16_bytes).expect("write utf16");

    let document_uri = format!("file://{}", project.join("config.yaml").display());
    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;

    for (document_text, label, expected_line) in [
        (r#"path: "./src/ut""#, "utf8.txt", "alpha"),
        (r#"path: "./src/utf1""#, "utf16.txt", "gamma"),
    ] {
        open_document(&mut service, &document_uri, "YAML", document_text).await;
        let cursor = u32::try_from(document_text.len() - 1).expect("cursor");
        let items = completion_items(&mut service, &document_uri, cursor).await;
        let candidate = items
            .into_iter()
            .find(|item| item["label"].as_str() == Some(label))
            .expect("candidate");
        let resolved = resolve_completion_item(&mut service, candidate).await;
        let documentation = resolved["documentation"]["value"]
            .as_str()
            .expect("documentation");
        assert!(documentation.contains(expected_line));
    }
}

#[tokio::test]
async fn lsp_completion_resolve_falls_back_for_binary_files() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(src.join("blob.bin"), [0x00, 0x01, 0x02, 0x03]).expect("write blob");

    let document_uri = format!("file://{}", project.join("config.yaml").display());
    let document_text = r#"path: "./src/bl""#;

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;
    open_document(&mut service, &document_uri, "YAML", document_text).await;

    let items = completion_items(&mut service, &document_uri, 15).await;
    let blob = items
        .into_iter()
        .find(|item| item["label"].as_str() == Some("blob.bin"))
        .expect("blob item");
    let resolved = resolve_completion_item(&mut service, blob).await;
    assert_eq!(
        resolved["documentation"]["value"].as_str(),
        Some("File path completion for `blob.bin`.")
    );
}

#[tokio::test]
async fn lsp_completion_resolve_previews_directory_structure() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    let docs = src.join("docs");
    std::fs::create_dir_all(docs.join("guide")).expect("mkdir guide");
    std::fs::write(docs.join("index.md"), "hello\n").expect("write index");
    std::fs::write(docs.join(".hidden"), "secret\n").expect("write hidden");
    for index in 0..10 {
        std::fs::write(docs.join(format!("entry-{index}.txt")), "x\n").expect("write entry");
    }

    let document_uri = format!("file://{}", project.join("config.yaml").display());
    let document_text = r#"path: "./src/do""#;

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;
    open_document(&mut service, &document_uri, "YAML", document_text).await;

    let items = completion_items(&mut service, &document_uri, 15).await;
    let docs_item = items
        .into_iter()
        .find(|item| item["label"].as_str() == Some("docs/"))
        .expect("docs item");
    let resolved = resolve_completion_item(&mut service, docs_item).await;
    let documentation = resolved["documentation"]["value"]
        .as_str()
        .expect("documentation");

    assert!(documentation.contains("docs/\n|-- guide/"));
    assert!(documentation.contains("|-- entry-0.txt"));
    assert!(documentation.contains("`-- ..."));
    assert!(!documentation.contains(".hidden"));
}

#[tokio::test]
async fn lsp_incremental_change_updates_completion_results() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
    std::fs::write(src.join("module.rs"), "pub fn f() {}\n").expect("write module");

    let document_uri = format!("file://{}", project.join("app.rs").display());
    let document_text = r#"let path = "./src/ma";"#;

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;
    open_document(&mut service, &document_uri, "Rust", document_text).await;

    change_document(
        &mut service,
        &document_uri,
        2,
        json!([
            {
                "range": {
                    "start": { "line": 0, "character": 18 },
                    "end": { "line": 0, "character": 20 }
                },
                "text": "mo"
            }
        ]),
    )
    .await;

    let items = completion_items(&mut service, &document_uri, 20).await;
    let labels = items
        .iter()
        .map(|item| item["label"].as_str().expect("label"))
        .collect::<Vec<_>>();
    assert!(labels.contains(&"module.rs"));
    assert!(!labels.contains(&"main.rs"));
}

#[tokio::test]
async fn lsp_multiple_incremental_changes_apply_in_order() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
    std::fs::write(src.join("module.rs"), "pub fn f() {}\n").expect("write module");

    let document_uri = format!("file://{}", project.join("app.rs").display());
    let document_text = r#"let path = "./src/ma";"#;

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;
    open_document(&mut service, &document_uri, "Rust", document_text).await;

    change_document(
        &mut service,
        &document_uri,
        2,
        json!([
            {
                "range": {
                    "start": { "line": 0, "character": 18 },
                    "end": { "line": 0, "character": 20 }
                },
                "text": "mo"
            },
            {
                "range": {
                    "start": { "line": 0, "character": 20 },
                    "end": { "line": 0, "character": 20 }
                },
                "text": "d"
            }
        ]),
    )
    .await;

    let items = completion_items(&mut service, &document_uri, 21).await;
    let labels = items
        .iter()
        .map(|item| item["label"].as_str().expect("label"))
        .collect::<Vec<_>>();
    assert!(labels.contains(&"module.rs"));
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
    let items = completion_items(&mut service, &document_uri, 19).await;
    assert_eq!(items[0]["label"].as_str(), Some("logo.svg"));
}

#[tokio::test]
async fn lsp_auto_trigger_respects_min_auto_trigger_word_length() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(project.join("readme.md"), "x").expect("write readme");
    std::fs::write(project.join("release.toml"), "x").expect("write release");

    let document_uri = format!("file://{}", project.join("src/app.rs").display());
    let document_text = r#"let path = "re";"#;
    let cursor = u32::try_from(document_text.find("re").expect("token start") + 2).expect("cursor");

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;

    let change_configuration = Request::build("workspace/didChangeConfiguration")
        .params(json!({
            "settings": {
                "min_auto_trigger_word_length": 3
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

    let auto_items =
        completion_items_with_trigger(&mut service, &document_uri, cursor, 3, None).await;
    assert!(auto_items.is_empty());

    let manual_items = completion_items(&mut service, &document_uri, cursor).await;
    let labels = manual_items
        .iter()
        .map(|item| item["label"].as_str().expect("label"))
        .collect::<Vec<_>>();
    assert!(labels.contains(&"readme.md"));
    assert!(labels.contains(&"release.toml"));
}

#[tokio::test]
async fn lsp_auto_trigger_supports_yaml_plain_scalars_without_slash() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join("modules")).expect("mkdir modules");
    std::fs::write(project.join("main.rs"), "x").expect("write main");

    let document_uri = format!("file://{}", project.join("config.yaml").display());
    let document_text = "src: mo";
    let cursor = u32::try_from(document_text.len()).expect("cursor");

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;

    let change_configuration = Request::build("workspace/didChangeConfiguration")
        .params(json!({
            "settings": {
                "min_auto_trigger_word_length": 1
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

    open_document(&mut service, &document_uri, "YAML", document_text).await;

    let auto_items =
        completion_items_with_trigger(&mut service, &document_uri, cursor, 2, None).await;
    let labels = auto_items
        .iter()
        .map(|item| item["label"].as_str().expect("label"))
        .collect::<Vec<_>>();
    assert!(labels.contains(&"modules/"));
}

#[tokio::test]
async fn lsp_slash_trigger_continues_workspace_root_completion_chain() {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let nested = project.join("nested");
    let modules = project.join("modules");
    std::fs::create_dir_all(&nested).expect("mkdir nested");
    std::fs::create_dir_all(modules.join("home-manager")).expect("mkdir home-manager");

    let document_uri = format!("file://{}", nested.join("config.yaml").display());
    let document_text = "src: modules/";
    let cursor = u32::try_from(document_text.len()).expect("cursor");

    let (mut service, _socket) = LspService::new(Backend::new);
    let _ = initialize_service(&mut service, project.as_path()).await;

    open_document(&mut service, &document_uri, "YAML", document_text).await;

    let auto_items =
        completion_items_with_trigger(&mut service, &document_uri, cursor, 2, Some("/")).await;
    let labels = auto_items
        .iter()
        .map(|item| item["label"].as_str().expect("label"))
        .collect::<Vec<_>>();
    assert!(labels.contains(&"home-manager/"));
}
