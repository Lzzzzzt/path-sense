use std::sync::LazyLock;

use tree_sitter::{Language, Node, Query};

use super::{LanguageHandler, compile_query, quoted_delimiters};
use crate::context::query::node_contains_node;
use crate::context::types::QueryRequest;

pub(crate) struct Yaml;

impl LanguageHandler for Yaml {
    fn language(&self) -> Language {
        tree_sitter_yaml::LANGUAGE.into()
    }

    fn quoted_query(&self) -> &'static Query {
        &QUOTED_QUERY
    }

    fn bare_query(&self) -> Option<&'static Query> {
        Some(&BARE_QUERY)
    }

    fn quoted_delimiter_lengths(&self, _node_kind: &str, text: &str) -> Option<(usize, usize)> {
        quoted_delimiters(text)
    }

    fn bare_node_is_supported(
        &self,
        _request: &QueryRequest<'_>,
        node: Node<'_>,
        _token: &str,
    ) -> bool {
        yaml_plain_scalar_is_supported_position(node)
    }
}

fn yaml_plain_scalar_is_supported_position(node: Node<'_>) -> bool {
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

static QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_yaml::LANGUAGE.into();
    compile_query(
        &language,
        "[(double_quote_scalar) (single_quote_scalar)] @path.context",
    )
});

static BARE_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = tree_sitter_yaml::LANGUAGE.into();
    compile_query(&language, "(plain_scalar) @path.context")
});
