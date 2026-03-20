use std::path::{Path, PathBuf};

use streaming_iterator::StreamingIterator;
use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

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
struct LanguageProfile {
    kind: LanguageKind,
    quoted_query: &'static str,
    bare_query: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LanguageKind {
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Rust,
    Go,
    Nix,
    Toml,
    Yaml,
    Json,
    ShellScript,
}

impl LanguageKind {
    fn language(self) -> Language {
        match self {
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Nix => tree_sitter_nix::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::ShellScript => tree_sitter_bash::LANGUAGE.into(),
        }
    }
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
    allow_empty_token: bool,
}

#[must_use]
pub fn extract_completion_context(
    text: &str,
    position: Position,
    language_id: &str,
    document_path: Option<&Path>,
    allow_empty_token: bool,
    outside_strings: Option<&OutsideStringsConfig<'_>>,
) -> Option<CompletionContext> {
    let cursor = CursorLocation {
        offset: position_to_offset(text, position)?,
    };

    if let Some(profile) = language_profile(language_id) {
        let tree = parse_syntax_tree(text, profile.kind)?;
        let request = QueryExtraction {
            text,
            cursor,
            tree: &tree,
            language_kind: profile.kind,
            document_path,
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

fn language_profile(language_id: &str) -> Option<LanguageProfile> {
    let kind = match normalize_language_id(language_id).as_str() {
        "javascript" | "javascriptreact" | "jsx" => LanguageKind::JavaScript,
        "typescript" => LanguageKind::TypeScript,
        "typescriptreact" | "tsx" => LanguageKind::Tsx,
        "python" => LanguageKind::Python,
        "rust" => LanguageKind::Rust,
        "go" => LanguageKind::Go,
        "nix" => LanguageKind::Nix,
        "toml" => LanguageKind::Toml,
        "yaml" | "yml" => LanguageKind::Yaml,
        "json" => LanguageKind::Json,
        "shell script" | "shellscript" | "bash" | "sh" => LanguageKind::ShellScript,
        _ => return None,
    };

    Some(LanguageProfile {
        kind,
        quoted_query: quoted_query(kind),
        bare_query: bare_query(kind),
    })
}

fn quoted_query(kind: LanguageKind) -> &'static str {
    match kind {
        LanguageKind::JavaScript | LanguageKind::TypeScript | LanguageKind::Tsx => {
            "[(string) (template_string)] @path.context"
        }
        LanguageKind::Python | LanguageKind::Toml | LanguageKind::Json => "(string) @path.context",
        LanguageKind::Rust => "[(string_literal) (raw_string_literal)] @path.context",
        LanguageKind::Go => "[(interpreted_string_literal) (raw_string_literal)] @path.context",
        LanguageKind::Nix => "[(string_expression) (indented_string_expression)] @path.context",
        LanguageKind::Yaml => "[(double_quote_scalar) (single_quote_scalar)] @path.context",
        LanguageKind::ShellScript => {
            "[(string) (raw_string) (translated_string) (ansi_c_string)] @path.context"
        }
    }
}

fn bare_query(kind: LanguageKind) -> Option<&'static str> {
    match kind {
        LanguageKind::Nix => {
            Some("[(path_expression) (hpath_expression) (spath_expression)] @path.context")
        }
        LanguageKind::ShellScript => Some("(word) @path.context"),
        _ => None,
    }
}

fn normalize_language_id(language_id: &str) -> String {
    language_id.trim().to_ascii_lowercase()
}

fn parse_syntax_tree(text: &str, language_kind: LanguageKind) -> Option<Tree> {
    let mut parser = Parser::new();
    let language = language_kind.language();
    parser.set_language(&language).ok()?;
    parser.parse(text, None)
}

fn extract_context_from_query(
    request: QueryExtraction<'_>,
    query_source: &str,
    trigger: CompletionTrigger,
) -> Option<CompletionContext> {
    let language = request.language_kind.language();
    let query = Query::new(&language, query_source).ok()?;
    let root = request.tree.root_node();
    let nodes = captured_nodes_containing_cursor(
        &query,
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
    allow_empty_token: bool,
) -> Option<CompletionContext> {
    if cursor < node.start_byte() || cursor > node.end_byte() {
        return None;
    }
    if has_disallowed_content_before_cursor(node, cursor, language_kind) {
        return None;
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
    if !outside_string_token_is_supported(token, mapping_keys) {
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

fn outside_string_token_is_supported(token: &str, mapping_keys: &[String]) -> bool {
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

fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let target_line = usize::try_from(position.line).ok()?;
    let target_character = usize::try_from(position.character).ok()?;
    let mut line = 0usize;
    let mut character = 0usize;
    let mut index = 0usize;

    for (byte_index, ch) in text.char_indices() {
        if line == target_line && character == target_character {
            return Some(byte_index);
        }
        if ch == '\n' {
            line += 1;
            character = 0;
            if line > target_line {
                return None;
            }
        } else {
            character += ch.len_utf16();
        }
        index = byte_index + ch.len_utf8();
    }

    if line == target_line && character == target_character {
        Some(index)
    } else if target_line == 0 && target_character == 0 && text.is_empty() {
        Some(0)
    } else {
        None
    }
}

fn range_from_offsets(text: &str, start: usize, end: usize) -> Option<Range> {
    Some(Range::new(
        offset_to_position(text, start)?,
        offset_to_position(text, end)?,
    ))
}

fn offset_to_position(text: &str, offset: usize) -> Option<Position> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }

    let mut line = 0u32;
    let mut character = 0u32;

    for (byte_index, ch) in text.char_indices() {
        if byte_index == offset {
            return Some(Position::new(line, character));
        }

        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += u32::try_from(ch.len_utf16()).ok()?;
        }
    }

    if offset == text.len() {
        Some(Position::new(line, character))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    #[test]
    fn javascript_string_context_detects_fragment() {
        let text = r#"const path = "./src/ma";"#;
        let context = extract_completion_context(
            text,
            position(0, 22),
            "JavaScript",
            Some(Path::new("/work/project/app.js")),
            false,
            None,
        )
        .expect("context");

        assert_eq!(context.prefix, "ma");
        assert_eq!(context.raw_token, "./src/ma");
    }

    #[test]
    fn tilde_token_preserves_home_insert_prefix() {
        let text = r#"path = "~""#;
        let context = extract_completion_context(
            text,
            position(0, 9),
            "TOML",
            Some(Path::new("/work/project/config.toml")),
            true,
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
            extract_completion_context(
                text,
                position(0, 28),
                "JavaScript",
                Some(Path::new("/work/project/app.js")),
                false,
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn shell_bare_token_detects_relative_path() {
        let text = "cp ./src/ma ./dest";
        let context = extract_completion_context(
            text,
            position(0, 11),
            "Shell Script",
            Some(Path::new("/work/project/script.sh")),
            false,
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
            extract_completion_context(
                text,
                position(0, 20),
                "Rust",
                Some(Path::new("/work/project/main.rs")),
                false,
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn incomplete_javascript_string_context_is_rejected() {
        let text = r#"const path = "./src/ma"#;
        assert!(
            extract_completion_context(
                text,
                position(0, 22),
                "JavaScript",
                Some(Path::new("/work/project/app.js")),
                false,
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn quoted_relative_fragment_without_slash_is_supported() {
        let text = r#"path = "rea""#;
        let context = extract_completion_context(
            text,
            position(0, 11),
            "TOML",
            Some(Path::new("/work/project/config.toml")),
            false,
            None,
        )
        .expect("context");

        assert_eq!(context.prefix, "rea");
        assert_eq!(context.raw_token, "rea");
    }

    #[test]
    fn nix_bare_path_without_dot_prefix_is_supported() {
        let text = "imports = [ nix/modules/co ]";
        let context = extract_completion_context(
            text,
            position(0, 26),
            "Nix",
            Some(Path::new("/work/project/flake.nix")),
            false,
            None,
        )
        .expect("context");

        assert_eq!(context.prefix, "co");
        assert_eq!(context.raw_token, "nix/modules/co");
    }

    #[test]
    fn nix_path_expression_is_supported() {
        let text = "++ lib.filesystem.listFilesRecursive ./modules/de;";
        let context = extract_completion_context(
            text,
            position(0, 49),
            "Nix",
            Some(Path::new("/work/project/home.nix")),
            false,
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
            extract_completion_context(
                text,
                position(0, 9),
                "Shell Script",
                Some(Path::new("/work/project/script.sh")),
                false,
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn outside_string_fallback_is_opt_in() {
        let text = "path: ~/Doc";
        assert!(
            extract_completion_context(
                text,
                position(0, 11),
                "YAML",
                Some(Path::new("/work/project/config.yaml")),
                false,
                None,
            )
            .is_none()
        );

        let mapping_keys = vec!["@assets".to_string()];
        let outside_strings = OutsideStringsConfig {
            path_separators: " \t({[",
            mapping_keys: &mapping_keys,
        };
        let context = extract_completion_context(
            text,
            position(0, 11),
            "YAML",
            Some(Path::new("/work/project/config.yaml")),
            false,
            Some(&outside_strings),
        )
        .expect("outside string context");

        assert_eq!(context.trigger, CompletionTrigger::OutsideString);
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
        let context = extract_completion_context(
            text,
            position(0, 12),
            "Plain Text",
            Some(Path::new("/work/project/notes.txt")),
            false,
            Some(&outside_strings),
        )
        .expect("outside string context");

        assert_eq!(context.raw_token, "@assets");
        assert_eq!(context.trigger, CompletionTrigger::OutsideString);
    }
}
