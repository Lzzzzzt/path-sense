use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, delimited_content_lengths, quoted_delimiters};

pub(crate) struct TypeScript;

impl LanguageHandler for TypeScript {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn quoted_delimiter_lengths(&self, node_kind: &str, text: &str) -> Option<(usize, usize)> {
        match node_kind {
            "string" => quoted_delimiters(text),
            "template_string" => delimited_content_lengths(text, 1, "`"),
            _ => None,
        }
    }

    fn is_disallowed_child_kind(&self, node_kind: &str) -> bool {
        node_kind == "template_substitution"
    }
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    compile_query(&language, "[(string) (template_string)] @path.context")
});
