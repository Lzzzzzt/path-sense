use std::path::Path;

use crate::text::range_from_offsets;

use super::types::{CompletionContext, CompletionTrigger};

pub(super) fn build_context(
    text: &str,
    cursor: usize,
    document_path: Option<&Path>,
    token_start: usize,
    trigger: CompletionTrigger,
    allow_empty_token: bool,
) -> Option<CompletionContext> {
    let token = text.get(token_start..cursor)?;
    if token.is_empty() && !allow_empty_token {
        return None;
    }

    let token_parts = split_token_for_completion(token);
    let replacement_start = token_start + token_parts.replacement_offset;
    let replacement_range = range_from_offsets(text, replacement_start, cursor)?;
    let line_prefix_start = text[..token_start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);

    Some(CompletionContext {
        trigger,
        allow_empty_token,
        document_path: document_path.map(Path::to_path_buf),
        raw_token: token.to_string(),
        line_prefix: text.get(line_prefix_start..token_start)?.to_string(),
        insert_prefix: token_parts.insert_prefix.to_string(),
        replacement_range,
        prefix: token_parts.prefix.to_string(),
    })
}

pub(super) fn path_like_token_is_supported(token: &str, mapping_keys: &[String]) -> bool {
    token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token == "~"
        || token.starts_with("~/")
        || token.contains('/')
        || mapping_keys.iter().map(String::as_str).any(|key| {
            let key = normalize_mapping_key(key);
            token == key
                || if key == "/" {
                    token.starts_with('/')
                } else {
                    token
                        .strip_prefix(key)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                }
        })
}

fn normalize_mapping_key(key: &str) -> &str {
    if key == "/" {
        "/"
    } else {
        key.trim_end_matches('/')
    }
}

struct TokenParts<'a> {
    insert_prefix: &'a str,
    prefix: &'a str,
    replacement_offset: usize,
}

fn split_token_for_completion(token: &str) -> TokenParts<'_> {
    if token == "~" {
        return TokenParts {
            insert_prefix: "~/",
            prefix: "",
            replacement_offset: 0,
        };
    }

    let last_slash = token.rfind('/');
    let prefix = last_slash.map_or(token, |index| &token[index + 1..]);

    TokenParts {
        insert_prefix: "",
        prefix,
        replacement_offset: last_slash.map_or(0, |index| index + 1),
    }
}
