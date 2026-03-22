use std::path::Path;

use tower_lsp::lsp_types::Position;

use crate::syntax::SyntaxSnapshot;
use crate::text::position_to_offset;

pub(crate) mod handlers;
mod query;
mod token;
mod types;

pub(crate) use token::mapping_key_supports_prefix_completion;
pub use types::{CompletionContext, CompletionTrigger, OutsideStringsConfig};

use handlers::handler_for_kind;
use query::extract_context_from_query;
use token::{build_context, path_like_token_is_supported};
use types::{CursorLocation, QueryRequest};

#[must_use]
pub fn extract_completion_context(
    text: &str,
    position: Position,
    syntax: Option<&SyntaxSnapshot>,
    document_path: Option<&Path>,
    allow_empty_token: bool,
    mapping_keys: &[String],
    outside_strings: Option<&OutsideStringsConfig<'_>>,
) -> Option<CompletionContext> {
    let cursor = CursorLocation {
        offset: position_to_offset(text, position)?,
    };

    if let Some(syntax) = syntax {
        let handler = handler_for_kind(syntax.language_kind);
        let request = QueryRequest {
            text,
            cursor,
            tree: &syntax.tree,
            document_path,
            mapping_keys,
            allow_empty_token,
        };

        if let Some(context) = extract_context_from_query(
            request,
            handler,
            handler.quoted_query(),
            CompletionTrigger::QuotedString,
        ) {
            return Some(context);
        }

        if let Some(bare_query) = handler.bare_query()
            && let Some(context) = extract_context_from_query(
                request,
                handler,
                bare_query,
                CompletionTrigger::BareToken,
            )
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
    if !path_like_token_is_supported(token, mapping_keys) {
        return None;
    }

    build_context(
        text,
        cursor,
        document_path,
        token_start,
        CompletionTrigger::OutsideString,
        allow_empty_token,
    )
}

#[cfg(test)]
mod tests;
