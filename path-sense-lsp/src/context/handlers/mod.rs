use tree_sitter::{Language, Node, Query};

use crate::syntax::LanguageKind;

use super::query::has_disallowed_content_before_cursor;
use super::token::build_context;
use super::types::{CompletionContext, CompletionTrigger, QueryRequest};

mod go;
mod javascript;
mod json;
mod nix;
mod python;
mod rust;
mod shell_script;
mod toml;
mod tsx;
mod typescript;
mod yaml;

pub(crate) trait LanguageHandler: Sync {
    fn language(&self) -> Language;

    fn quoted_query(&self) -> &'static Query;

    fn bare_query(&self) -> Option<&'static Query> {
        None
    }

    fn quoted_delimiter_lengths(&self, node_kind: &str, text: &str) -> Option<(usize, usize)>;

    fn is_disallowed_child_kind(&self, _node_kind: &str) -> bool {
        false
    }

    fn bare_token_is_supported(&self, _token: &str) -> bool {
        true
    }

    fn bare_node_is_supported(
        &self,
        _request: &QueryRequest<'_>,
        _node: Node<'_>,
        _token: &str,
    ) -> bool {
        true
    }

    fn build_quoted_context(
        &self,
        request: QueryRequest<'_>,
        node: Node<'_>,
    ) -> Option<CompletionContext> {
        if has_disallowed_content_before_cursor(node, request.cursor.offset, self) {
            return None;
        }

        let node_text = request.text.get(node.byte_range())?;
        let (opening_len, closing_len) = self.quoted_delimiter_lengths(node.kind(), node_text)?;
        let content_start = node.start_byte() + opening_len;
        let content_end = node.end_byte().checked_sub(closing_len)?;

        if request.cursor.offset < content_start || request.cursor.offset > content_end {
            return None;
        }

        build_context(
            request.text,
            request.cursor.offset,
            request.document_path,
            content_start,
            CompletionTrigger::QuotedString,
            request.allow_empty_token,
        )
    }

    fn build_bare_context(
        &self,
        request: QueryRequest<'_>,
        node: Node<'_>,
    ) -> Option<CompletionContext> {
        if request.cursor.offset < node.start_byte() || request.cursor.offset > node.end_byte() {
            return None;
        }
        if has_disallowed_content_before_cursor(node, request.cursor.offset, self) {
            return None;
        }

        let token = request.text.get(node.start_byte()..request.cursor.offset)?;
        if !self.bare_token_is_supported(token)
            || !self.bare_node_is_supported(&request, node, token)
        {
            return None;
        }

        build_context(
            request.text,
            request.cursor.offset,
            request.document_path,
            node.start_byte(),
            CompletionTrigger::BareToken,
            request.allow_empty_token,
        )
    }
}

#[must_use]
pub(crate) fn handler_for_kind(kind: LanguageKind) -> &'static dyn LanguageHandler {
    match kind {
        LanguageKind::JavaScript => &JAVASCRIPT,
        LanguageKind::TypeScript => &TYPESCRIPT,
        LanguageKind::Tsx => &TSX,
        LanguageKind::Python => &PYTHON,
        LanguageKind::Rust => &RUST,
        LanguageKind::Go => &GO,
        LanguageKind::Nix => &NIX,
        LanguageKind::Toml => &TOML,
        LanguageKind::Yaml => &YAML,
        LanguageKind::Json => &JSON,
        LanguageKind::ShellScript => &SHELL_SCRIPT,
    }
}

pub(super) fn compile_query(language: &Language, query: &str) -> Query {
    Query::new(language, query).expect("valid tree-sitter query")
}

pub(super) fn quoted_delimiters(text: &str) -> Option<(usize, usize)> {
    let quote = text.get(..1)?;
    delimited_content_lengths(text, 1, quote)
}

pub(super) fn delimited_content_lengths(
    text: &str,
    opening_len: usize,
    closing: &str,
) -> Option<(usize, usize)> {
    if text.len() < opening_len + closing.len() || !text.ends_with(closing) {
        return None;
    }

    Some((opening_len, closing.len()))
}

static JAVASCRIPT: javascript::JavaScript = javascript::JavaScript;
static TYPESCRIPT: typescript::TypeScript = typescript::TypeScript;
static TSX: tsx::Tsx = tsx::Tsx;
static PYTHON: python::Python = python::Python;
static RUST: rust::Rust = rust::Rust;
static GO: go::Go = go::Go;
static NIX: nix::Nix = nix::Nix;
static TOML: toml::Toml = toml::Toml;
static YAML: yaml::Yaml = yaml::Yaml;
static JSON: json::Json = json::Json;
static SHELL_SCRIPT: shell_script::ShellScript = shell_script::ShellScript;
