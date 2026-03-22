use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use super::{LanguageHandler, compile_query, delimited_content_lengths};

pub(crate) struct Python;

impl LanguageHandler for Python {
    fn language(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn quoted_delimiter_lengths(&self, _node_kind: &str, text: &str) -> Option<(usize, usize)> {
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

    fn is_disallowed_child_kind(&self, node_kind: &str) -> bool {
        node_kind == "interpolation"
    }
}

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_python::LANGUAGE.into();
    compile_query(&language, "(string) @path.context")
});
