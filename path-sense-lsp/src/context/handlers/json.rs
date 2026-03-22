use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, quoted_delimiters};

pub(crate) struct Json;

impl LanguageHandler for Json {
    fn language(&self) -> Language {
        tree_sitter_json::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn quoted_delimiter_lengths(&self, _node_kind: &str, text: &str) -> Option<(usize, usize)> {
        quoted_delimiters(text)
    }
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_json::LANGUAGE.into();
    compile_query(&language, "(string) @path.context")
});
