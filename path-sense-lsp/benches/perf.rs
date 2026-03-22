use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use path_sense_lsp::Backend;
use path_sense_lsp::context::extract_completion_context;
use path_sense_lsp::document_store::DocumentStore;
use path_sense_lsp::engine::{CompletionRequest, PathSenseEngine};
use path_sense_lsp::resolver::WorkspaceRoots;
use path_sense_lsp::settings::{CompiledSettings, PathSenseSettings};
use path_sense_lsp::syntax::SyntaxState;
use serde_json::json;
use tempfile::tempdir;
use tower::Service;
use tower::ServiceExt;
use tower_lsp::LspService;
use tower_lsp::jsonrpc::Request;
use tower_lsp::lsp_types::{Position, TextDocumentContentChangeEvent, Url};

fn bench_compiled_settings(c: &mut Criterion) {
    let settings = json!({
        "path_mappings": {
            "@assets": "${workspace}/assets",
            "/": "/tmp",
            "@root": {
                "conditions": [
                    { "when": "src/**", "value": ["${workspace}/src", "${workspace}/assets"] },
                    { "when": "nix/**", "value": "${workspace}/nix" }
                ]
            }
        },
        "ignored_files_patterns": ["vendor/**", ".direnv/**"],
        "ignored_prefixes": ["http://", "https://"],
        "directory_suffix": "/"
    });

    c.bench_function("compiled_settings/build", |b| {
        b.iter(|| CompiledSettings::from_json_value(black_box(settings.clone())));
    });
}

fn bench_context_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("context_extraction");
    let samples = [
        (
            "rust_quoted",
            "Rust",
            r#"let path = "./src/module";"#,
            Position::new(0, 22),
            PathBuf::from("/tmp/project/src/app.rs"),
        ),
        (
            "yaml_plain",
            "YAML",
            "path: ./modules/dev",
            Position::new(0, 19),
            PathBuf::from("/tmp/project/config.yaml"),
        ),
        (
            "nix_bare",
            "Nix",
            "imports = [ ./modules/dev ];",
            Position::new(0, 25),
            PathBuf::from("/tmp/project/home.nix"),
        ),
    ];

    for (name, language_id, text, position, document_path) in samples {
        let syntax = SyntaxState::new(language_id, text).expect("syntax");
        let snapshot = syntax.snapshot();
        group.bench_function(name, |b| {
            b.iter(|| {
                let _ = extract_completion_context(
                    black_box(text),
                    black_box(position),
                    Some(black_box(&snapshot)),
                    Some(black_box(document_path.as_path())),
                    false,
                    &[],
                    None,
                );
            });
        });
    }

    group.finish();
}

fn bench_repeated_completion(c: &mut Criterion) {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(src.join("module_dir")).expect("mkdir");
    std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
    std::fs::write(src.join("module.rs"), "pub fn f() {}\n").expect("write module");

    let text = r#"let path = "./src/m";"#;
    let document_path = project.join("app.rs");
    let syntax = SyntaxState::new("Rust", text).expect("syntax");
    let snapshot = syntax.snapshot();
    let workspace_roots = WorkspaceRoots {
        internal_worktree_root: Some(project.clone()),
        lsp_roots: Vec::new(),
    };
    let settings = CompiledSettings::default();
    let engine = PathSenseEngine;

    c.bench_function("completion/repeated_rust_open_document", |b| {
        b.iter(|| {
            let request = CompletionRequest {
                text: black_box(text),
                position: black_box(Position::new(0, 19)),
                syntax: Some(black_box(&snapshot)),
                document_path: Some(black_box(document_path.as_path())),
                workspace_roots: black_box(&workspace_roots),
                allow_empty_token: true,
                settings: black_box(&settings),
            };
            let _ = engine.complete(&request);
        });
    });
}

fn bench_incremental_change(c: &mut Criterion) {
    let uri = Url::parse("file:///tmp/project/app.rs").expect("uri");

    c.bench_function("incremental_change/document_store_small_edit", |b| {
        b.iter_batched(
            || {
                let mut store = DocumentStore::default();
                store.open(
                    uri.clone(),
                    "Rust".to_string(),
                    "let path = \"./src/ma\";".to_string(),
                    Some(1),
                );
                store
            },
            |mut store| {
                store.apply_changes(
                    &uri,
                    vec![TextDocumentContentChangeEvent {
                        range: Some(tower_lsp::lsp_types::Range::new(
                            Position::new(0, 18),
                            Position::new(0, 20),
                        )),
                        range_length: None,
                        text: "mo".to_string(),
                    }],
                    Some(2),
                );
                let _ = store.snapshot(&uri);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_lsp_round_trip(c: &mut Criterion) {
    let tmp = tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let src = project.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
    std::fs::write(src.join("module.rs"), "pub fn f() {}\n").expect("write module");

    let document_uri = format!("file://{}", project.join("app.rs").display());
    let document_text = r#"let path = "./src/ma";"#;
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    c.bench_function("lsp_round_trip/incremental_completion", |b| {
        b.iter_batched(
            || LspService::new(Backend::new).0,
            |mut service| {
                runtime.block_on(async {
                    initialize_service(&mut service, project.as_path()).await;
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
                    let _ = completion_items(&mut service, &document_uri, 20).await;
                });
            },
            BatchSize::SmallInput,
        );
    });
}

fn initialize_settings_fixture() -> CompiledSettings {
    CompiledSettings::from(PathSenseSettings::from_json_value(json!({
        "path_mappings": {
            "@assets": "${workspace}/assets"
        }
    })))
}

async fn initialize_service(service: &mut LspService<Backend>, project: &Path) {
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
    let _ = service
        .ready()
        .await
        .expect("service ready")
        .call(initialize)
        .await
        .expect("initialize response");
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

fn criterion_benchmark(c: &mut Criterion) {
    let _settings_fixture = initialize_settings_fixture();
    bench_compiled_settings(c);
    bench_context_extraction(c);
    bench_repeated_completion(c);
    bench_incremental_change(c);
    bench_lsp_round_trip(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
