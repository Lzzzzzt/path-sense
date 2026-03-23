use std::path::Path;

use tower_lsp::lsp_types::Position;

use super::handlers::handler_for_kind;
use super::query::extract_context_from_query;
use super::types::{CompletionContext, CompletionTrigger, CursorLocation, QueryRequest};
use super::*;
use crate::syntax::{LanguageKind, SyntaxState};
use crate::text::position_to_offset;

fn position(line: u32, character: u32) -> Position {
    Position::new(line, character)
}

fn extract(
    text: &str,
    position: Position,
    language_id: &str,
    document_path: Option<&Path>,
    allow_empty_token: bool,
    mapping_keys: &[String],
    outside_strings: Option<&OutsideStringsConfig<'_>>,
) -> Option<CompletionContext> {
    let syntax = SyntaxState::new(language_id, text);
    let snapshot = syntax.as_ref().map(SyntaxState::snapshot);
    extract_completion_context(
        text,
        position,
        snapshot.as_ref(),
        document_path,
        allow_empty_token,
        mapping_keys,
        outside_strings,
    )
}

fn request<'a>(
    text: &'a str,
    position: Position,
    syntax: &'a crate::syntax::SyntaxSnapshot,
    document_path: Option<&'a Path>,
    allow_empty_token: bool,
) -> QueryRequest<'a> {
    QueryRequest {
        text,
        cursor: CursorLocation {
            offset: position_to_offset(text, position).expect("cursor offset"),
        },
        tree: &syntax.tree,
        document_path,
        allow_empty_token,
    }
}

#[test]
fn javascript_string_context_detects_fragment() {
    let text = r#"const path = "./src/ma";"#;
    let context = extract(
        text,
        position(0, 22),
        "JavaScript",
        Some(Path::new("/work/project/app.js")),
        false,
        &[],
        None,
    )
    .expect("context");

    assert_eq!(context.prefix, "ma");
    assert_eq!(context.raw_token, "./src/ma");
}

#[test]
fn tilde_token_preserves_home_insert_prefix() {
    let text = r#"path = "~""#;
    let context = extract(
        text,
        position(0, 9),
        "TOML",
        Some(Path::new("/work/project/config.toml")),
        true,
        &[],
        None,
    )
    .expect("context");

    assert_eq!(context.raw_token, "~");
    assert_eq!(context.prefix, "");
    assert_eq!(context.insert_prefix, "~/");
}

#[test]
fn javascript_template_string_rejects_prior_substitution() {
    let text = "const path = `${base}/src/ma`;";
    assert!(
        extract(
            text,
            position(0, 28),
            "JavaScript",
            Some(Path::new("/work/project/app.js")),
            false,
            &[],
            None,
        )
        .is_none()
    );
}

#[test]
fn shell_bare_token_detects_relative_path() {
    let text = "cp ./src/ma ./dest";
    let context = extract(
        text,
        position(0, 11),
        "Shell Script",
        Some(Path::new("/work/project/script.sh")),
        false,
        &[],
        None,
    )
    .expect("context");

    assert_eq!(context.prefix, "ma");
    assert_eq!(context.raw_token, "./src/ma");
    assert_eq!(context.trigger, CompletionTrigger::BareToken);
}

#[test]
fn hidden_completion_requires_hidden_prefix() {
    let handler = handler_for_kind(LanguageKind::ShellScript);
    assert!(handler.bare_token_is_supported("./.env"));
    assert!(!handler.bare_token_is_supported("plain"));
}

#[test]
fn incomplete_rust_string_context_is_rejected() {
    let text = r#"let path = "./src/ma"#;
    assert!(
        extract(
            text,
            position(0, 20),
            "Rust",
            Some(Path::new("/work/project/main.rs")),
            false,
            &[],
            None,
        )
        .is_none()
    );
}

#[test]
fn incomplete_javascript_string_context_is_rejected() {
    let text = r#"const path = "./src/ma"#;
    assert!(
        extract(
            text,
            position(0, 22),
            "JavaScript",
            Some(Path::new("/work/project/app.js")),
            false,
            &[],
            None,
        )
        .is_none()
    );
}

#[test]
fn quoted_relative_fragment_without_slash_is_supported() {
    let text = r#"path = "rea""#;
    let context = extract(
        text,
        position(0, 11),
        "TOML",
        Some(Path::new("/work/project/config.toml")),
        false,
        &[],
        None,
    )
    .expect("context");

    assert_eq!(context.prefix, "rea");
    assert_eq!(context.raw_token, "rea");
}

#[test]
fn nix_bare_path_without_dot_prefix_is_supported() {
    let text = "imports = [ nix/modules/co ]";
    let context = extract(
        text,
        position(0, 26),
        "Nix",
        Some(Path::new("/work/project/flake.nix")),
        false,
        &[],
        None,
    )
    .expect("context");

    assert_eq!(context.prefix, "co");
    assert_eq!(context.raw_token, "nix/modules/co");
}

#[test]
fn nix_path_expression_is_supported() {
    let text = "++ lib.filesystem.listFilesRecursive ./modules/de;";
    let context = extract(
        text,
        position(0, 49),
        "Nix",
        Some(Path::new("/work/project/home.nix")),
        false,
        &[],
        None,
    )
    .expect("context");

    assert_eq!(context.trigger, CompletionTrigger::BareToken);
    assert_eq!(context.prefix, "de");
    assert_eq!(context.raw_token, "./modules/de");
}

#[test]
fn unsupported_plain_shell_token_is_rejected() {
    let text = "cp README";
    assert!(
        extract(
            text,
            position(0, 9),
            "Shell Script",
            Some(Path::new("/work/project/script.sh")),
            false,
            &[],
            None,
        )
        .is_none()
    );
}

#[test]
fn yaml_plain_scalar_prefers_native_syntax_over_outside_string_fallback() {
    let text = "path: ~/Doc";
    let mapping_keys = vec!["@assets".to_string()];
    let outside_strings = OutsideStringsConfig {
        path_separators: " \t({[",
        mapping_keys: &mapping_keys,
    };
    let context = extract(
        text,
        position(0, 11),
        "YAML",
        Some(Path::new("/work/project/config.yaml")),
        false,
        &mapping_keys,
        Some(&outside_strings),
    )
    .expect("yaml plain scalar context");

    assert_eq!(context.trigger, CompletionTrigger::BareToken);
    assert_eq!(context.raw_token, "~/Doc");
}

#[test]
fn outside_string_fallback_supports_exact_mapping_keys() {
    let text = "open @assets";
    let mapping_keys = vec!["@assets".to_string()];
    let outside_strings = OutsideStringsConfig {
        path_separators: " \t({[",
        mapping_keys: &mapping_keys,
    };
    let context = extract(
        text,
        position(0, 12),
        "Plain Text",
        Some(Path::new("/work/project/notes.txt")),
        false,
        &mapping_keys,
        Some(&outside_strings),
    )
    .expect("outside string context");

    assert_eq!(context.raw_token, "@assets");
    assert_eq!(context.trigger, CompletionTrigger::OutsideString);
}

#[test]
fn yaml_plain_scalar_path_like_values_are_supported() {
    for (text, character, expected) in [
        ("path: ./mod", 11, "./mod"),
        ("path: ~/src", 11, "~/src"),
        ("imports: /etc/hos", 17, "/etc/hos"),
        ("- ./modules/dev", 15, "./modules/dev"),
        ("src: mo", 7, "mo"),
    ] {
        let context = extract(
            text,
            position(0, character),
            "YAML",
            Some(Path::new("/work/project/config.yaml")),
            false,
            &[String::from("@assets")],
            None,
        )
        .expect("yaml plain scalar context");

        assert_eq!(context.trigger, CompletionTrigger::BareToken);
        assert_eq!(context.raw_token, expected);
    }
}

#[test]
fn yaml_plain_scalar_rejects_keys_non_paths_and_block_scalars() {
    assert!(
        extract(
            "imports:",
            position(0, 7),
            "YAML",
            Some(Path::new("/work/project/config.yaml")),
            false,
            &[],
            None,
        )
        .is_none()
    );

    assert!(
        extract(
            "script: |\n  ./modules/dev\n",
            position(1, 15),
            "YAML",
            Some(Path::new("/work/project/config.yaml")),
            false,
            &[],
            None,
        )
        .is_none()
    );
}

#[test]
fn javascript_family_blocks_template_substitutions() {
    for kind in [
        LanguageKind::JavaScript,
        LanguageKind::TypeScript,
        LanguageKind::Tsx,
    ] {
        assert!(handler_for_kind(kind).is_disallowed_child_kind("template_substitution"));
    }
}

#[test]
fn python_and_nix_block_interpolation() {
    for kind in [LanguageKind::Python, LanguageKind::Nix] {
        assert!(handler_for_kind(kind).is_disallowed_child_kind("interpolation"));
    }
}

#[test]
fn yaml_handler_only_accepts_value_side_plain_scalars() {
    let handler = handler_for_kind(LanguageKind::Yaml);
    let syntax = SyntaxState::new("YAML", "path: ./mod").expect("syntax");
    let snapshot = syntax.snapshot();
    let accepted = extract_context_from_query(
        request(
            "path: ./mod",
            position(0, 11),
            &snapshot,
            Some(Path::new("/work/project/config.yaml")),
            false,
        ),
        handler,
        handler.bare_query().expect("yaml bare query"),
        CompletionTrigger::BareToken,
    );
    assert!(accepted.is_some());

    let syntax = SyntaxState::new("YAML", "name: hello").expect("syntax");
    let snapshot = syntax.snapshot();
    let accepted_plain_value = extract_context_from_query(
        request(
            "name: hello",
            position(0, 11),
            &snapshot,
            Some(Path::new("/work/project/config.yaml")),
            false,
        ),
        handler,
        handler.bare_query().expect("yaml bare query"),
        CompletionTrigger::BareToken,
    );
    assert!(accepted_plain_value.is_some());

    let syntax = SyntaxState::new("YAML", "imports:").expect("syntax");
    let snapshot = syntax.snapshot();
    let rejected = extract_context_from_query(
        request(
            "imports:",
            position(0, 7),
            &snapshot,
            Some(Path::new("/work/project/config.yaml")),
            false,
        ),
        handler,
        handler.bare_query().expect("yaml bare query"),
        CompletionTrigger::BareToken,
    );
    assert!(rejected.is_none());
}

#[test]
fn shell_script_handler_limits_bare_tokens() {
    let handler = handler_for_kind(LanguageKind::ShellScript);
    assert!(handler.bare_token_is_supported("~"));
    assert!(handler.bare_token_is_supported("~/src"));
    assert!(handler.bare_token_is_supported("./src"));
    assert!(!handler.bare_token_is_supported("plain"));
}

#[test]
fn rust_go_and_nix_handlers_report_expected_delimiters() {
    let rust = handler_for_kind(LanguageKind::Rust);
    assert_eq!(
        rust.quoted_delimiter_lengths("string_literal", "\"./src\""),
        Some((1, 1))
    );
    assert_eq!(
        rust.quoted_delimiter_lengths("raw_string_literal", "r#\"./src\"#"),
        Some((3, 2))
    );

    let go = handler_for_kind(LanguageKind::Go);
    assert_eq!(
        go.quoted_delimiter_lengths("interpreted_string_literal", "\"./src\""),
        Some((1, 1))
    );
    assert_eq!(
        go.quoted_delimiter_lengths("raw_string_literal", "`./src`"),
        Some((1, 1))
    );

    let nix = handler_for_kind(LanguageKind::Nix);
    assert_eq!(
        nix.quoted_delimiter_lengths("string_expression", "\"./src\""),
        Some((1, 1))
    );
    assert_eq!(
        nix.quoted_delimiter_lengths("indented_string_expression", "''./src''"),
        Some((2, 2))
    );
}
