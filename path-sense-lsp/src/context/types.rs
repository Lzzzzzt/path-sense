use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::Range;
use tree_sitter::Tree;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionTrigger {
    QuotedString,
    BareToken,
    OutsideString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionContext {
    pub trigger: CompletionTrigger,
    pub allow_empty_token: bool,
    pub document_path: Option<PathBuf>,
    pub raw_token: String,
    pub line_prefix: String,
    pub insert_prefix: String,
    pub replacement_range: Range,
    pub prefix: String,
}

pub struct OutsideStringsConfig<'a> {
    pub path_separators: &'a str,
    pub mapping_keys: &'a [String],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CursorLocation {
    pub(crate) offset: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct QueryRequest<'a> {
    pub(crate) text: &'a str,
    pub(crate) cursor: CursorLocation,
    pub(crate) tree: &'a Tree,
    pub(crate) document_path: Option<&'a Path>,
    pub(crate) mapping_keys: &'a [String],
    pub(crate) allow_empty_token: bool,
}
