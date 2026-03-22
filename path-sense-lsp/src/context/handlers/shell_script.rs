use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, delimited_content_lengths, quoted_delimiters};

pub(crate) struct ShellScript;

impl LanguageHandler for ShellScript {
    fn language(&self) -> Language {
        tree_sitter_bash::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn bare_query(&self) -> Option<&'static Query> {
        Some(&BARE_QUERY)
    }

    fn quoted_delimiter_lengths(&self, node_kind: &str, text: &str) -> Option<(usize, usize)> {
        match node_kind {
            "string" | "raw_string" => quoted_delimiters(text),
            "translated_string" | "ansi_c_string" => {
                let quote = text.get(1..=1)?;
                delimited_content_lengths(text, 2, quote)
            }
            _ => None,
        }
    }

    fn is_disallowed_child_kind(&self, node_kind: &str) -> bool {
        matches!(
            node_kind,
            "arithmetic_expansion"
                | "command_substitution"
                | "expansion"
                | "process_substitution"
                | "simple_expansion"
        )
    }

    fn bare_token_is_supported(&self, token: &str) -> bool {
        token == "~"
            || token.starts_with("~/")
            || token.starts_with('/')
            || token.starts_with("./")
            || token.starts_with("../")
    }
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_bash::LANGUAGE.into();
    compile_query(
        &language,
        "[(string) (raw_string) (translated_string) (ansi_c_string)] @path.context",
    )
});

static BARE_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_bash::LANGUAGE.into();
    compile_query(&language, "(word) @path.context")
});
