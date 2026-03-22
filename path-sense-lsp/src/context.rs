use std::path::{Path, PathBuf};

use streaming_iterator::StreamingIterator;
use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::{Node, Query, QueryCursor, Tree};

use crate::syntax::{LanguageKind, SyntaxSnapshot, language_profile_for_kind};
use crate::text::{position_to_offset, range_from_offsets};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionTrigger {
    QuotedString,
    BareToken,
    OutsideString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionContext {
    pub trigger: CompletionTrigger,
    pub allow_empty_token: bool,
    pub document_path: Option<PathBuf>,
    pub raw_token: String,
    pub line_prefix: String,
    pub insert_prefix: String,
    pub replacement_range: Range,
    pub prefix: String,
}

pub struct OutsideStringsConfig<'a> {
    pub path_separators: &'a str,
    pub mapping_keys: &'a [String],
}

#[derive(Clone, Copy, Debug)]
struct CursorLocation {
    offset: usize,
}

#[derive(Clone, Copy)]
struct QueryExtraction<'a> {
    text: &'a str,
    cursor: CursorLocation,
    tree: &'a Tree,
    language_kind: LanguageKind,
    document_path: Option<&'a Path>,
    mapping_keys: &'a [String],
    allow_empty_token: bool,
}

#[must_use]
pub fn extract_completion_context(
    text: &str,
    position: Position,
    syntax: Option<&SyntaxSnapshot>,
    document_path: Option<&Path>,
    allow_empty_token: bool,
    mapping_keys: &[String],
    outside_strings: Option<&OutsideStringsConfig<'_>>,
) -> Option<CompletionContext> {
    let cursor = CursorLocation {
        offset: position_to_offset(text, position)?,
    };

    if let Some(syntax) = syntax {
        let profile = language_profile_for_kind(syntax.language_kind);
        let request = QueryExtraction {
            text,
            cursor,
            tree: &syntax.tree,
            language_kind: syntax.language_kind,
            document_path,
            mapping_keys,
            allow_empty_token,
        };

        if let Some(context) = extract_context_from_query(
            request,
            profile.quoted_query,
            CompletionTrigger::QuotedString,
        ) {
            return Some(context);
        }

        if let Some(bare_query) = profile.bare_query
            && let Some(context) =
                extract_context_from_query(request, bare_query, CompletionTrigger::BareToken)
        {
            return Some(context);
        }
    }

    outside_strings.and_then(|outside_strings| {
        extract_outside_string_context(
            text,
            cursor,
            document_path,
            allow_empty_token,
            outside_strings,
        )
    })
}

fn extract_context_from_query(
    request: QueryExtraction<'_>,
    query: &Query,
    trigger: CompletionTrigger,
) -> Option<CompletionContext> {
    let root = request.tree.root_node();
    let nodes = captured_nodes_containing_cursor(
        query,
        root,
        request.text.as_bytes(),
        request.cursor.offset,
    );

    for node in nodes {
        let context = match trigger {
            CompletionTrigger::QuotedString => build_quoted_context(
                request.text,
                request.cursor.offset,
                request.document_path,
                request.language_kind,
                node,
                request.allow_empty_token,
            ),
            CompletionTrigger::BareToken => build_bare_context(
                request.text,
                request.cursor.offset,
                request.document_path,
                request.language_kind,
                node,
                request.mapping_keys,
                request.allow_empty_token,
            ),
            CompletionTrigger::OutsideString => None,
        };

        if context.is_some() {
            return context;
        }
    }

    None
}

fn captured_nodes_containing_cursor<'tree>(
    query: &Query,
    root: Node<'tree>,
    text: &[u8],
    offset: usize,
) -> Vec<Node<'tree>> {
    let mut query_cursor = QueryCursor::new();
    let range_start = offset.saturating_sub(1);
    let range_end = offset.saturating_add(1).min(text.len());
    query_cursor.set_byte_range(range_start..range_end);

    let mut nodes = Vec::new();
    let mut matches = query_cursor.matches(query, root, text);

    while let Some(matched) = {
        matches.advance();
        matches.get()
    } {
        for capture in matched.captures {
            let node = capture.node;
            if !node_contains_cursor(node, offset, text.len()) {
                continue;
            }

            if nodes.iter().any(|existing| same_node(*existing, node)) {
                continue;
            }
            nodes.push(node);
        }
    }

    nodes.sort_by_key(|node| node.byte_range().len());
    nodes
}

fn same_node(left: Node<'_>, right: Node<'_>) -> bool {
    left.start_byte() == right.start_byte()
        && left.end_byte() == right.end_byte()
        && left.kind() == right.kind()
}

fn build_quoted_context(
    text: &str,
    cursor: usize,
    document_path: Option<&Path>,
    language_kind: LanguageKind,
    node: Node,
    allow_empty_token: bool,
) -> Option<CompletionContext> {
    if has_disallowed_content_before_cursor(node, cursor, language_kind) {
        return None;
    }

    let node_text = text.get(node.byte_range())?;
    let (opening_len, closing_len) =
        quoted_delimiter_lengths(language_kind, node.kind(), node_text)?;
    let content_start = node.start_byte() + opening_len;
    let content_end = node.end_byte().checked_sub(closing_len)?;

    if cursor < content_start || cursor > content_end {
        return None;
    }

    build_context(
        text,
        cursor,
        document_path,
        content_start,
        language_kind,
        CompletionTrigger::QuotedString,
        allow_empty_token,
    )
}

fn build_bare_context(
    text: &str,
    cursor: usize,
    document_path: Option<&Path>,
    language_kind: LanguageKind,
    node: Node,
    mapping_keys: &[String],
    allow_empty_token: bool,
) -> Option<CompletionContext> {
    if cursor < node.start_byte() || cursor > node.end_byte() {
        return None;
    }
    if has_disallowed_content_before_cursor(node, cursor, language_kind) {
        return None;
    }

    if language_kind == LanguageKind::Yaml {
        let token = text.get(node.start_byte()..cursor)?;
        if !yaml_plain_scalar_is_supported_position(node)
            || !path_like_token_is_supported(token, mapping_keys)
        {
            return None;
        }

        return build_context_unchecked(
            text,
            cursor,
            document_path,
            node.start_byte(),
            CompletionTrigger::BareToken,
            allow_empty_token,
        );
    }

    build_context(
        text,
        cursor,
        document_path,
        node.start_byte(),
        language_kind,
        CompletionTrigger::BareToken,
        allow_empty_token,
    )
}

fn extract_outside_string_context(
    text: &str,
    cursor: CursorLocation,
    document_path: Option<&Path>,
    allow_empty_token: bool,
    outside_strings: &OutsideStringsConfig<'_>,
) -> Option<CompletionContext> {
    let token_start = outside_token_start(text, cursor.offset, outside_strings.path_separators)?;
    build_outside_context(
        text,
        cursor.offset,
        document_path,
        token_start,
        allow_empty_token,
        outside_strings.mapping_keys,
    )
}

fn outside_token_start(text: &str, cursor: usize, separators: &str) -> Option<usize> {
    if cursor > text.len() {
        return None;
    }

    let mut token_start = 0usize;
    for (byte_index, ch) in text[..cursor].char_indices() {
        if ch == '\n' || separators.contains(ch) {
            token_start = byte_index + ch.len_utf8();
        }
    }

    Some(token_start)
}

fn build_outside_context(
    text: &str,
    cursor: usize,
    document_path: Option<&Path>,
    token_start: usize,
    allow_empty_token: bool,
    mapping_keys: &[String],
) -> Option<CompletionContext> {
    let token = text.get(token_start..cursor)?;
    if token.is_empty() && !allow_empty_token {
        return None;
    }
    if !path_like_token_is_supported(token, mapping_keys) {
        return None;
    }

    build_context(
        text,
        cursor,
        document_path,
        token_start,
        LanguageKind::JavaScript,
        CompletionTrigger::OutsideString,
        allow_empty_token,
    )
}

fn path_like_token_is_supported(token: &str, mapping_keys: &[String]) -> bool {
    token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token == "~"
        || token.starts_with("~/")
        || token.contains('/')
        || mapping_keys.iter().any(|key| {
            let key = normalize_mapping_key(key);
            token == key
                || if key == "/" {
                    token.starts_with('/')
                } else {
                    token.starts_with(format!("{key}/").as_str())
                }
        })
}

fn yaml_plain_scalar_is_supported_position(node: Node) -> bool {
    let mut current = node;
    let mut in_flow_sequence = false;

    while let Some(parent) = current.parent() {
        match parent.kind() {
            "block_mapping_pair" | "flow_pair" => {
                return parent
                    .child_by_field_name("value")
                    .is_some_and(|value| node_contains_node(value, node));
            }
            "block_sequence_item" => return true,
            "flow_sequence" => in_flow_sequence = true,
            _ => {}
        }
        current = parent;
    }

    in_flow_sequence
}

fn node_contains_node(ancestor: Node, descendant: Node) -> bool {
    ancestor.start_byte() <= descendant.start_byte() && ancestor.end_byte() >= descendant.end_byte()
}

fn normalize_mapping_key(key: &str) -> &str {
    if key == "/" {
        "/"
    } else {
        key.trim_end_matches('/')
    }
}

fn has_disallowed_content_before_cursor(
    node: Node,
    cursor: usize,
    language_kind: LanguageKind,
) -> bool {
    if cursor <= node.start_byte() {
        return false;
    }

    let mut walk = node.walk();
    node.named_children(&mut walk).any(|child| {
        child.start_byte() < cursor
            && (is_disallowed_node_kind(language_kind, child.kind())
                || has_disallowed_content_before_cursor(child, cursor, language_kind))
    })
}

fn is_disallowed_node_kind(language_kind: LanguageKind, node_kind: &str) -> bool {
    match language_kind {
        LanguageKind::JavaScript | LanguageKind::TypeScript | LanguageKind::Tsx => {
            node_kind == "template_substitution"
        }
        LanguageKind::Python | LanguageKind::Nix => node_kind == "interpolation",
        LanguageKind::ShellScript => matches!(
            node_kind,
            "arithmetic_expansion"
                | "command_substitution"
                | "expansion"
                | "process_substitution"
                | "simple_expansion"
        ),
        LanguageKind::Rust
        | LanguageKind::Go
        | LanguageKind::Toml
        | LanguageKind::Yaml
        | LanguageKind::Json => false,
    }
}

fn node_contains_cursor(node: Node, offset: usize, text_len: usize) -> bool {
    let start = node.start_byte();
    let end = node.end_byte();
    start <= offset && (offset <= end || (offset == text_len && end == text_len))
}

fn quoted_delimiter_lengths(
    language_kind: LanguageKind,
    node_kind: &str,
    text: &str,
) -> Option<(usize, usize)> {
    match language_kind {
        LanguageKind::JavaScript | LanguageKind::TypeScript | LanguageKind::Tsx => {
            javascript_delimiters(node_kind, text)
        }
        LanguageKind::Python => python_delimiters(text),
        LanguageKind::Rust => rust_delimiters(node_kind, text),
        LanguageKind::Go => go_delimiters(node_kind, text),
        LanguageKind::Nix => nix_delimiters(node_kind, text),
        LanguageKind::Toml | LanguageKind::Yaml | LanguageKind::Json => quoted_delimiters(text),
        LanguageKind::ShellScript => shell_delimiters(node_kind, text),
    }
}

fn javascript_delimiters(node_kind: &str, text: &str) -> Option<(usize, usize)> {
    match node_kind {
        "string" => quoted_delimiters(text),
        "template_string" => delimited_content_lengths(text, 1, "`"),
        _ => None,
    }
}

fn python_delimiters(text: &str) -> Option<(usize, usize)> {
    let first_quote = text.find(['"', '\''])?;
    let quote = text.get(first_quote..=first_quote)?;
    let triple = text
        .get(first_quote..first_quote + 3)
        .is_some_and(|slice| slice == format!("{quote}{quote}{quote}"));
    let quote_len = if triple { 3 } else { 1 };
    let opening_len = first_quote + quote_len;
    let closing = quote.repeat(quote_len);
    delimited_content_lengths(text, opening_len, &closing)
}

fn rust_delimiters(node_kind: &str, text: &str) -> Option<(usize, usize)> {
    match node_kind {
        "string_literal" => quoted_delimiters(text),
        "raw_string_literal" => {
            let first_quote = text.find('"')?;
            let hashes = first_quote.checked_sub(1)?;
            let suffix = format!("\"{}", "#".repeat(hashes));
            delimited_content_lengths(text, first_quote + 1, &suffix)
        }
        _ => None,
    }
}

fn go_delimiters(node_kind: &str, text: &str) -> Option<(usize, usize)> {
    match node_kind {
        "interpreted_string_literal" => quoted_delimiters(text),
        "raw_string_literal" => delimited_content_lengths(text, 1, "`"),
        _ => None,
    }
}

fn nix_delimiters(node_kind: &str, text: &str) -> Option<(usize, usize)> {
    match node_kind {
        "string_expression" => quoted_delimiters(text),
        "indented_string_expression" => delimited_content_lengths(text, 2, "''"),
        _ => None,
    }
}

fn shell_delimiters(node_kind: &str, text: &str) -> Option<(usize, usize)> {
    match node_kind {
        "string" | "raw_string" => quoted_delimiters(text),
        "translated_string" | "ansi_c_string" => {
            let quote = text.get(1..=1)?;
            delimited_content_lengths(text, 2, quote)
        }
        _ => None,
    }
}

fn quoted_delimiters(text: &str) -> Option<(usize, usize)> {
    let quote = text.get(..1)?;
    delimited_content_lengths(text, 1, quote)
}

fn delimited_content_lengths(
    text: &str,
    opening_len: usize,
    closing: &str,
) -> Option<(usize, usize)> {
    if text.len() < opening_len + closing.len() || !text.ends_with(closing) {
        return None;
    }

    Some((opening_len, closing.len()))
}

fn build_context(
    text: &str,
    cursor: usize,
    document_path: Option<&Path>,
    token_start: usize,
    language_kind: LanguageKind,
    trigger: CompletionTrigger,
    allow_empty_token: bool,
) -> Option<CompletionContext> {
    let token = text.get(token_start..cursor)?;
    if token.is_empty() && !allow_empty_token {
        return None;
    }

    if !token_is_supported(language_kind, token, trigger) {
        return None;
    }

    build_context_unchecked(
        text,
        cursor,
        document_path,
        token_start,
        trigger,
        allow_empty_token,
    )
}

fn build_context_unchecked(
    text: &str,
    cursor: usize,
    document_path: Option<&Path>,
    token_start: usize,
    trigger: CompletionTrigger,
    allow_empty_token: bool,
) -> Option<CompletionContext> {
    let token = text.get(token_start..cursor)?;
    if token.is_empty() && !allow_empty_token {
        return None;
    }

    let token_parts = split_token_for_completion(token);
    let replacement_start = token_start + token_parts.replacement_offset;
    let replacement_range = range_from_offsets(text, replacement_start, cursor)?;
    let line_prefix_start = text[..token_start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);

    Some(CompletionContext {
        trigger,
        allow_empty_token,
        document_path: document_path.map(Path::to_path_buf),
        raw_token: token.to_string(),
        line_prefix: text.get(line_prefix_start..token_start)?.to_string(),
        insert_prefix: token_parts.insert_prefix.to_string(),
        replacement_range,
        prefix: token_parts.prefix.to_string(),
    })
}

fn token_is_supported(
    language_kind: LanguageKind,
    token: &str,
    trigger: CompletionTrigger,
) -> bool {
    match trigger {
        CompletionTrigger::BareToken => bare_token_is_supported(language_kind, token),
        CompletionTrigger::QuotedString | CompletionTrigger::OutsideString => true,
    }
}

fn bare_token_is_supported(language_kind: LanguageKind, token: &str) -> bool {
    match language_kind {
        LanguageKind::Nix => {
            token.starts_with('/')
                || token.starts_with("./")
                || token.starts_with("../")
                || token.contains('/')
        }
        LanguageKind::ShellScript => {
            token == "~"
                || token.starts_with("~/")
                || token.starts_with('/')
                || token.starts_with("./")
                || token.starts_with("../")
        }
        LanguageKind::JavaScript
        | LanguageKind::TypeScript
        | LanguageKind::Tsx
        | LanguageKind::Python
        | LanguageKind::Rust
        | LanguageKind::Go
        | LanguageKind::Toml
        | LanguageKind::Yaml
        | LanguageKind::Json => false,
    }
}

struct TokenParts<'a> {
    insert_prefix: &'a str,
    prefix: &'a str,
    replacement_offset: usize,
}

fn split_token_for_completion(token: &str) -> TokenParts<'_> {
    if token == "~" {
        return TokenParts {
            insert_prefix: "~/",
            prefix: "",
            replacement_offset: 0,
        };
    }

    let last_slash = token.rfind('/');
    let prefix = last_slash.map_or(token, |index| &token[index + 1..]);

    TokenParts {
        insert_prefix: "",
        prefix,
        replacement_offset: last_slash.map_or(0, |index| index + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::SyntaxState;

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
        assert!(bare_token_is_supported(LanguageKind::ShellScript, "./.env"));
        assert!(!bare_token_is_supported(LanguageKind::ShellScript, "plain"));
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
        ] {
            let context = extract(
                text,
                position(0, character),
                "YAML",
                Some(Path::new("/work/project/config.yaml")),
                false,
                &[],
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
                "name: hello",
                position(0, 11),
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
}
