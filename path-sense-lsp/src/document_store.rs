use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

use crate::syntax::{SyntaxSnapshot, SyntaxState};
use crate::text::apply_range_change;

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    pub text: Arc<String>,
    pub version: Option<i32>,
    pub language_id: String,
    pub path: Option<PathBuf>,
    pub syntax: Option<SyntaxSnapshot>,
}

struct Document {
    text: Arc<String>,
    version: Option<i32>,
    language_id: String,
    path: Option<PathBuf>,
    syntax: Option<SyntaxState>,
}

#[derive(Default)]
pub struct DocumentStore {
    documents: HashMap<Url, Document>,
}

impl DocumentStore {
    pub fn open(&mut self, uri: Url, language_id: String, text: String, version: Option<i32>) {
        let path = uri.to_file_path().ok();
        let text = Arc::new(text);
        let syntax = SyntaxState::new(language_id.as_str(), text.as_str());

        self.documents.insert(
            uri,
            Document {
                text,
                version,
                language_id,
                path,
                syntax,
            },
        );
    }

    pub fn apply_changes(
        &mut self,
        uri: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: Option<i32>,
    ) {
        let Some(document) = self.documents.get_mut(uri) else {
            return;
        };

        for change in changes {
            document.apply_change(change);
        }
        document.version = version;
    }

    pub fn close(&mut self, uri: &Url) {
        self.documents.remove(uri);
    }

    #[must_use]
    pub fn snapshot(&self, uri: &Url) -> Option<DocumentSnapshot> {
        self.documents.get(uri).map(Document::snapshot)
    }
}

impl Document {
    fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            text: Arc::clone(&self.text),
            version: self.version,
            language_id: self.language_id.clone(),
            path: self.path.clone(),
            syntax: self.syntax.as_ref().map(SyntaxState::snapshot),
        }
    }

    fn apply_change(&mut self, change: TextDocumentContentChangeEvent) {
        if let Some(range) = change.range {
            let text = Arc::make_mut(&mut self.text);
            let Some(input_edit) = apply_range_change(text, range, change.text.as_str()) else {
                self.reparse_syntax_full();
                return;
            };

            if let Some(syntax) = self.syntax.as_mut() {
                if !syntax.apply_edit(text.as_str(), &input_edit) {
                    self.syntax = SyntaxState::new(self.language_id.as_str(), text.as_str());
                }
            }
            return;
        }

        self.text = Arc::new(change.text);
        self.reparse_syntax_full();
    }

    fn reparse_syntax_full(&mut self) {
        let text = self.text.as_str();
        if let Some(syntax) = self.syntax.as_mut()
            && syntax.reparse_full(text)
        {
            return;
        }

        self.syntax = SyntaxState::new(self.language_id.as_str(), text);
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{Position, Range};

    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(path).expect("uri")
    }

    #[test]
    fn applies_incremental_insert_delete_and_replace() {
        let uri = uri("file:///tmp/app.rs");
        let mut store = DocumentStore::default();
        store.open(
            uri.clone(),
            "Rust".to_string(),
            "let path = \"./src/ma\";".to_string(),
            Some(1),
        );

        store.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 20), Position::new(0, 20))),
                range_length: None,
                text: "i".to_string(),
            }],
            Some(2),
        );
        assert_eq!(
            store.snapshot(&uri).expect("snapshot").text.as_str(),
            "let path = \"./src/mai\";"
        );

        store.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 20), Position::new(0, 21))),
                range_length: None,
                text: String::new(),
            }],
            Some(3),
        );
        assert_eq!(
            store.snapshot(&uri).expect("snapshot").text.as_str(),
            "let path = \"./src/ma\";"
        );

        store.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 18), Position::new(0, 20))),
                range_length: None,
                text: "in".to_string(),
            }],
            Some(4),
        );
        assert_eq!(
            store.snapshot(&uri).expect("snapshot").text.as_str(),
            "let path = \"./src/in\";"
        );
    }

    #[test]
    fn applies_multiple_changes_in_order() {
        let uri = uri("file:///tmp/app.rs");
        let mut store = DocumentStore::default();
        store.open(
            uri.clone(),
            "Rust".to_string(),
            "let path = \"./src/ma\";".to_string(),
            Some(1),
        );

        store.apply_changes(
            &uri,
            vec![
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(0, 18), Position::new(0, 20))),
                    range_length: None,
                    text: "in".to_string(),
                },
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(0, 20), Position::new(0, 20))),
                    range_length: None,
                    text: ".rs".to_string(),
                },
            ],
            Some(2),
        );

        let snapshot = store.snapshot(&uri).expect("snapshot");
        assert_eq!(snapshot.text.as_str(), "let path = \"./src/in.rs\";");
        assert!(snapshot.syntax.is_some());
    }

    #[test]
    fn full_text_change_rebuilds_document() {
        let uri = uri("file:///tmp/app.rs");
        let mut store = DocumentStore::default();
        store.open(
            uri.clone(),
            "Rust".to_string(),
            "let path = \"./src/ma\";".to_string(),
            Some(1),
        );

        store.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "let path = \"./src/main.rs\";".to_string(),
            }],
            Some(2),
        );

        let snapshot = store.snapshot(&uri).expect("snapshot");
        assert_eq!(snapshot.text.as_str(), "let path = \"./src/main.rs\";");
        assert!(snapshot.syntax.is_some());
    }

    #[test]
    fn invalid_incremental_range_keeps_existing_text_consistent() {
        let uri = uri("file:///tmp/app.rs");
        let mut store = DocumentStore::default();
        store.open(
            uri.clone(),
            "Rust".to_string(),
            "let path = \"./src/ma\";".to_string(),
            Some(1),
        );

        store.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(99, 0), Position::new(99, 1))),
                range_length: None,
                text: "broken".to_string(),
            }],
            Some(2),
        );

        let snapshot = store.snapshot(&uri).expect("snapshot");
        assert_eq!(snapshot.text.as_str(), "let path = \"./src/ma\";");
        assert!(snapshot.syntax.is_some());
    }

    #[test]
    fn unsupported_languages_still_update_text() {
        let uri = uri("file:///tmp/app.txt");
        let mut store = DocumentStore::default();
        store.open(
            uri.clone(),
            "Plain Text".to_string(),
            "open ./src/ma".to_string(),
            Some(1),
        );

        store.apply_changes(
            &uri,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 11), Position::new(0, 13))),
                range_length: None,
                text: "in".to_string(),
            }],
            Some(2),
        );

        let snapshot = store.snapshot(&uri).expect("snapshot");
        assert_eq!(snapshot.text.as_str(), "open ./src/in");
        assert!(snapshot.syntax.is_none());
    }
}
