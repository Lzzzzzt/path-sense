use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, delimited_content_lengths, quoted_delimiters};

pub(crate) struct Rust;

impl LanguageHandler for Rust {
    fn language(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn quoted_delimiter_lengths(&self, node_kind: &str, text: &str) -> Option<(usize, usize)> {
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
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_rust::LANGUAGE.into();
    compile_query(
        &language,
        "[(string_literal) (raw_string_literal)] @path.context",
    )
});
