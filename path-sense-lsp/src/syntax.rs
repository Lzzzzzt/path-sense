use std::sync::LazyLock;

use tree_sitter::{InputEdit, Language, Parser, Query, Tree};

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
    fn language(self) -> Language {
        match self {
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Nix => tree_sitter_nix::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::ShellScript => tree_sitter_bash::LANGUAGE.into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LanguageProfile {
    pub kind: LanguageKind,
    pub quoted_query: &'static Query,
    pub bare_query: Option<&'static Query>,
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
        let profile = language_profile(language_id)?;
        let mut parser = Parser::new();
        parser.set_language(&profile.kind.language()).ok()?;
        let tree = parser.parse(text, None)?;
        Some(Self {
            language_kind: profile.kind,
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

#[must_use]
pub fn language_profile(language_id: &str) -> Option<LanguageProfile> {
    let kind = match normalize_language_id(language_id).as_str() {
        "javascript" | "javascriptreact" | "jsx" => LanguageKind::JavaScript,
        "typescript" => LanguageKind::TypeScript,
        "typescriptreact" | "tsx" => LanguageKind::Tsx,
        "python" => LanguageKind::Python,
        "rust" => LanguageKind::Rust,
        "go" => LanguageKind::Go,
        "nix" => LanguageKind::Nix,
        "toml" => LanguageKind::Toml,
        "yaml" | "yml" => LanguageKind::Yaml,
        "json" => LanguageKind::Json,
        "shell script" | "shellscript" | "bash" | "sh" => LanguageKind::ShellScript,
        _ => return None,
    };

    Some(language_profile_for_kind(kind))
}

#[must_use]
pub fn language_profile_for_kind(kind: LanguageKind) -> LanguageProfile {
    LanguageProfile {
        kind,
        quoted_query: quoted_query(kind),
        bare_query: bare_query(kind),
    }
}

fn normalize_language_id(language_id: &str) -> String {
    language_id.trim().to_ascii_lowercase()
}

fn compile_query(kind: LanguageKind, query: &str) -> Query {
    Query::new(&kind.language(), query).expect("valid tree-sitter query")
}

fn quoted_query(kind: LanguageKind) -> &'static Query {
    match kind {
        LanguageKind::JavaScript => &JAVASCRIPT_QUOTED_QUERY,
        LanguageKind::TypeScript => &TYPESCRIPT_QUOTED_QUERY,
        LanguageKind::Tsx => &TSX_QUOTED_QUERY,
        LanguageKind::Python => &PYTHON_QUOTED_QUERY,
        LanguageKind::Rust => &RUST_QUOTED_QUERY,
        LanguageKind::Go => &GO_QUOTED_QUERY,
        LanguageKind::Nix => &NIX_QUOTED_QUERY,
        LanguageKind::Toml => &TOML_QUOTED_QUERY,
        LanguageKind::Yaml => &YAML_QUOTED_QUERY,
        LanguageKind::Json => &JSON_QUOTED_QUERY,
        LanguageKind::ShellScript => &SHELL_QUOTED_QUERY,
    }
}

fn bare_query(kind: LanguageKind) -> Option<&'static Query> {
    match kind {
        LanguageKind::Yaml => Some(&YAML_BARE_QUERY),
        LanguageKind::Nix => Some(&NIX_BARE_QUERY),
        LanguageKind::ShellScript => Some(&SHELL_BARE_QUERY),
        LanguageKind::JavaScript
        | LanguageKind::TypeScript
        | LanguageKind::Tsx
        | LanguageKind::Python
        | LanguageKind::Rust
        | LanguageKind::Go
        | LanguageKind::Toml
        | LanguageKind::Json => None,
    }
}

static JAVASCRIPT_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::JavaScript,
        "[(string) (template_string)] @path.context",
    )
});
static TYPESCRIPT_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::TypeScript,
        "[(string) (template_string)] @path.context",
    )
});
static TSX_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::Tsx,
        "[(string) (template_string)] @path.context",
    )
});
static PYTHON_QUOTED_QUERY: LazyLock<Query> =
    LazyLock::new(|| compile_query(LanguageKind::Python, "(string) @path.context"));
static RUST_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::Rust,
        "[(string_literal) (raw_string_literal)] @path.context",
    )
});
static GO_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::Go,
        "[(interpreted_string_literal) (raw_string_literal)] @path.context",
    )
});
static NIX_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::Nix,
        "[(string_expression) (indented_string_expression)] @path.context",
    )
});
static TOML_QUOTED_QUERY: LazyLock<Query> =
    LazyLock::new(|| compile_query(LanguageKind::Toml, "(string) @path.context"));
static YAML_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::Yaml,
        "[(double_quote_scalar) (single_quote_scalar)] @path.context",
    )
});
static JSON_QUOTED_QUERY: LazyLock<Query> =
    LazyLock::new(|| compile_query(LanguageKind::Json, "(string) @path.context"));
static SHELL_QUOTED_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::ShellScript,
        "[(string) (raw_string) (translated_string) (ansi_c_string)] @path.context",
    )
});
static YAML_BARE_QUERY: LazyLock<Query> =
    LazyLock::new(|| compile_query(LanguageKind::Yaml, "(plain_scalar) @path.context"));
static NIX_BARE_QUERY: LazyLock<Query> = LazyLock::new(|| {
    compile_query(
        LanguageKind::Nix,
        "[(path_expression) (hpath_expression) (spath_expression)] @path.context",
    )
});
static SHELL_BARE_QUERY: LazyLock<Query> =
    LazyLock::new(|| compile_query(LanguageKind::ShellScript, "(word) @path.context"));
