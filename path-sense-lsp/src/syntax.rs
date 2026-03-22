use tree_sitter::{InputEdit, Parser, Tree};

use crate::context::handlers::handler_for_kind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanguageKind {
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Rust,
    Go,
    Nix,
    Toml,
    Yaml,
    Json,
    ShellScript,
}

impl LanguageKind {
    fn from_language_id(language_id: &str) -> Option<Self> {
        match normalize_language_id(language_id).as_str() {
            "javascript" | "javascriptreact" | "jsx" => Some(Self::JavaScript),
            "typescript" => Some(Self::TypeScript),
            "typescriptreact" | "tsx" => Some(Self::Tsx),
            "python" => Some(Self::Python),
            "rust" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "nix" => Some(Self::Nix),
            "toml" => Some(Self::Toml),
            "yaml" | "yml" => Some(Self::Yaml),
            "json" => Some(Self::Json),
            "shell script" | "shellscript" | "bash" | "sh" => Some(Self::ShellScript),
            _ => None,
        }
    }
}

#[must_use]
pub fn language_kind_for_id(language_id: &str) -> Option<LanguageKind> {
    LanguageKind::from_language_id(language_id)
}

#[derive(Clone, Debug)]
pub struct SyntaxSnapshot {
    pub language_kind: LanguageKind,
    pub tree: Tree,
}

pub struct SyntaxState {
    language_kind: LanguageKind,
    parser: Parser,
    tree: Tree,
}

impl SyntaxState {
    #[must_use]
    pub fn new(language_id: &str, text: &str) -> Option<Self> {
        let language_kind = language_kind_for_id(language_id)?;
        let handler = handler_for_kind(language_kind);
        let mut parser = Parser::new();
        parser.set_language(&handler.language()).ok()?;
        let tree = parser.parse(text, None)?;
        Some(Self {
            language_kind,
            parser,
            tree,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> SyntaxSnapshot {
        SyntaxSnapshot {
            language_kind: self.language_kind,
            tree: self.tree.clone(),
        }
    }

    #[must_use]
    pub fn language_kind(&self) -> LanguageKind {
        self.language_kind
    }

    pub fn reparse_full(&mut self, text: &str) -> bool {
        let Some(tree) = self.parser.parse(text, None) else {
            return false;
        };
        self.tree = tree;
        true
    }

    pub fn apply_edit(&mut self, text: &str, edit: &InputEdit) -> bool {
        self.tree.edit(edit);
        let Some(tree) = self.parser.parse(text, Some(&self.tree)) else {
            return self.reparse_full(text);
        };
        self.tree = tree;
        true
    }
}

fn normalize_language_id(language_id: &str) -> String {
    language_id.trim().to_ascii_lowercase()
}
