use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor};

use super::handlers::LanguageHandler;
use super::types::{CompletionContext, CompletionTrigger, QueryRequest};

pub(super) fn extract_context_from_query(
    request: QueryRequest<'_>,
    handler: &dyn LanguageHandler,
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
            CompletionTrigger::QuotedString => handler.build_quoted_context(request, node),
            CompletionTrigger::BareToken => handler.build_bare_context(request, node),
            CompletionTrigger::OutsideString => None,
        };

        if context.is_some() {
            return context;
        }
    }

    None
}

pub(super) fn has_disallowed_content_before_cursor<H: LanguageHandler + ?Sized>(
    node: Node<'_>,
    cursor: usize,
    handler: &H,
) -> bool {
    if cursor <= node.start_byte() {
        return false;
    }

    let mut walk = node.walk();
    node.named_children(&mut walk).any(|child| {
        child.start_byte() < cursor
            && (handler.is_disallowed_child_kind(child.kind())
                || has_disallowed_content_before_cursor(child, cursor, handler))
    })
}

pub(super) fn node_contains_node(ancestor: Node<'_>, descendant: Node<'_>) -> bool {
    ancestor.start_byte() <= descendant.start_byte() && ancestor.end_byte() >= descendant.end_byte()
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

fn node_contains_cursor(node: Node<'_>, offset: usize, text_len: usize) -> bool {
    let start = node.start_byte();
    let end = node.end_byte();
    start <= offset && (offset <= end || (offset == text_len && end == text_len))
}

fn same_node(left: Node<'_>, right: Node<'_>) -> bool {
    left.start_byte() == right.start_byte()
        && left.end_byte() == right.end_byte()
        && left.kind() == right.kind()
}
