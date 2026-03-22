use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, delimited_content_lengths, quoted_delimiters};

pub(crate) struct Nix;

impl LanguageHandler for Nix {
    fn language(&self) -> Language {
        tree_sitter_nix::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn bare_query(&self) -> Option<&'static Query> {
        Some(&BARE_QUERY)
    }

    fn quoted_delimiter_lengths(&self, node_kind: &str, text: &str) -> Option<(usize, usize)> {
        match node_kind {
            "string_expression" => quoted_delimiters(text),
            "indented_string_expression" => delimited_content_lengths(text, 2, "''"),
            _ => None,
        }
    }

    fn is_disallowed_child_kind(&self, node_kind: &str) -> bool {
        node_kind == "interpolation"
    }

    fn bare_token_is_supported(&self, token: &str) -> bool {
        token.starts_with('/')
            || token.starts_with("./")
            || token.starts_with("../")
            || token.contains('/')
    }
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_nix::LANGUAGE.into();
    compile_query(
        &language,
        "[(string_expression) (indented_string_expression)] @path.context",
    )
});

static BARE_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_nix::LANGUAGE.into();
    compile_query(
        &language,
        "[(path_expression) (hpath_expression) (spath_expression)] @path.context",
    )
});
