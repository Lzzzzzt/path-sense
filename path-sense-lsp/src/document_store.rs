use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::lsp_types::Url;

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    pub text: String,
    pub version: Option<i32>,
    pub language_id: String,
    pub path: Option<PathBuf>,
}

#[derive(Default, Debug)]
pub struct DocumentStore {
    documents: HashMap<Url, DocumentSnapshot>,
}

impl DocumentStore {
    pub fn open(&mut self, uri: Url, language_id: String, text: String, version: Option<i32>) {
        let path = uri.to_file_path().ok();
        self.documents.insert(
            uri,
            DocumentSnapshot {
                text,
                version,
                language_id,
                path,
            },
        );
    }

    pub fn replace_text(&mut self, uri: &Url, text: String, version: Option<i32>) {
        if let Some(document) = self.documents.get_mut(uri) {
            document.text = text;
            document.version = version;
        }
    }

    pub fn close(&mut self, uri: &Url) {
        self.documents.remove(uri);
    }

    #[must_use]
    pub fn get(&self, uri: &Url) -> Option<&DocumentSnapshot> {
        self.documents.get(uri)
    }
}
