use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, delimited_content_lengths, quoted_delimiters};

pub(crate) struct Go;

impl LanguageHandler for Go {
    fn language(&self) -> Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn quoted_delimiter_lengths(&self, node_kind: &str, text: &str) -> Option<(usize, usize)> {
        match node_kind {
            "interpreted_string_literal" => quoted_delimiters(text),
            "raw_string_literal" => delimited_content_lengths(text, 1, "`"),
            _ => None,
        }
    }
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_go::LANGUAGE.into();
    compile_query(
        &language,
        "[(interpreted_string_literal) (raw_string_literal)] @path.context",
    )
});
