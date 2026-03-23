use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::{CompletionItem, Documentation, MarkupContent, MarkupKind};

const MAX_TEXT_PREVIEW_BYTES: usize = 8 * 1024;
const MAX_TEXT_PREVIEW_LINES: usize = 3;
const MAX_TEXT_PREVIEW_LINE_CHARS: usize = 120;
const MAX_DIRECTORY_PREVIEW_ENTRIES: usize = 10;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CompletionItemData {
    path: String,
    kind: CompletionItemPreviewKind,
    annotation: String,
    name: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CompletionItemPreviewKind {
    File,
    Directory,
}

pub(crate) fn completion_item_data(
    path: &Path,
    is_dir: bool,
    annotation: &str,
    name: &str,
) -> CompletionItemData {
    CompletionItemData {
        path: path.to_string_lossy().into_owned(),
        kind: if is_dir {
            CompletionItemPreviewKind::Directory
        } else {
            CompletionItemPreviewKind::File
        },
        annotation: annotation.to_string(),
        name: name.to_string(),
    }
}

pub(crate) fn fallback_completion_documentation(annotation: &str, name: &str) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!("{annotation} path completion for `{name}`."),
    })
}

pub(crate) fn attach_completion_documentation(item: &mut CompletionItem) {
    let Some(data) = item
        .data
        .as_ref()
        .and_then(|value| serde_json::from_value::<CompletionItemData>(value.clone()).ok())
    else {
        return;
    };

    item.documentation = Some(documentation_for_data(&data));
}

fn documentation_for_data(data: &CompletionItemData) -> Documentation {
    match data.kind {
        CompletionItemPreviewKind::File => preview_file_documentation(data)
            .unwrap_or_else(|| fallback_completion_documentation(data.annotation.as_str(), data.name.as_str())),
        CompletionItemPreviewKind::Directory => preview_directory_documentation(data)
            .unwrap_or_else(|| fallback_completion_documentation(data.annotation.as_str(), data.name.as_str())),
    }
}

fn preview_file_documentation(data: &CompletionItemData) -> Option<Documentation> {
    let preview = preview_text_file(Path::new(data.path.as_str()))?;
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!(
            "{} path completion for `{}`.\n\n~~~text\n{}\n~~~{}",
            data.annotation,
            data.name,
            preview.body,
            if preview.truncated {
                "\n\nPreview truncated."
            } else {
                ""
            }
        ),
    }))
}

fn preview_directory_documentation(data: &CompletionItemData) -> Option<Documentation> {
    let preview = preview_directory(Path::new(data.path.as_str()))?;
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!(
            "{} path completion for `{}`.\n\n~~~text\n{}\n~~~",
            data.annotation, data.name, preview
        ),
    }))
}

struct TextPreview {
    body: String,
    truncated: bool,
}

fn preview_text_file(path: &Path) -> Option<TextPreview> {
    let bytes = read_preview_bytes(path)?;
    let text = decode_text_preview(bytes.as_slice())?;
    if !looks_like_text(text.as_str()) {
        return None;
    }

    let mut lines = Vec::new();
    let mut truncated = false;
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_TEXT_PREVIEW_LINES {
            truncated = true;
            break;
        }

        let (line, was_truncated) = truncate_line(line, MAX_TEXT_PREVIEW_LINE_CHARS);
        truncated |= was_truncated;
        lines.push(line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    Some(TextPreview {
        body: lines.join("\n"),
        truncated,
    })
}

fn read_preview_bytes(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    let mut file = File::open(path).ok()?;
    let mut buffer = Vec::with_capacity(MAX_TEXT_PREVIEW_BYTES);
    let mut chunk = vec![0_u8; MAX_TEXT_PREVIEW_BYTES];
    let bytes_read = file.read(chunk.as_mut_slice()).ok()?;
    chunk.truncate(bytes_read);
    buffer.extend(chunk);
    Some(buffer)
}

fn decode_text_preview(bytes: &[u8]) -> Option<String> {
    if let Some(stripped) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode_utf8_preview(stripped);
    }
    if let Some(stripped) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16_preview(stripped, true);
    }
    if let Some(stripped) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16_preview(stripped, false);
    }

    decode_utf8_preview(bytes)
}

fn decode_utf8_preview(bytes: &[u8]) -> Option<String> {
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => Some(text),
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            (valid_up_to > 0).then(|| String::from_utf8_lossy(&bytes[..valid_up_to]).into_owned())
        }
        Err(_) => None,
    }
}

fn decode_utf16_preview(bytes: &[u8], little_endian: bool) -> Option<String> {
    let chunks = bytes.chunks_exact(2);
    if chunks.len() == 0 {
        return Some(String::new());
    }

    let units = chunks
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();

    String::from_utf16(units.as_slice()).ok()
}

fn looks_like_text(text: &str) -> bool {
    !text.chars().any(|character| {
        character != '\n'
            && character != '\r'
            && character != '\t'
            && character != '\u{000C}'
            && character.is_control()
    })
}

fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    let mut result = String::new();
    let mut chars = line.chars();
    for _ in 0..max_chars {
        let Some(character) = chars.next() else {
            return (line.to_string(), false);
        };
        result.push(character);
    }

    if chars.next().is_some() {
        result.push_str("...");
        (result, true)
    } else {
        (line.to_string(), false)
    }
}

fn preview_directory(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return None;
    }

    let root_label = directory_display_name(path);
    let mut preview_lines = vec![format!("{root_label}/")];
    let mut entries = fs::read_dir(path)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.is_empty() || name.starts_with('.') {
                return None;
            }

            Some((name, entry.file_type().ok()?.is_dir()))
        })
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| {
        right.1
            .cmp(&left.1)
            .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
    });

    if entries.is_empty() {
        preview_lines.push("`-- (empty)".to_string());
        return Some(preview_lines.join("\n"));
    }

    let truncated = entries.len() > MAX_DIRECTORY_PREVIEW_ENTRIES;
    let visible_entries = entries
        .into_iter()
        .take(MAX_DIRECTORY_PREVIEW_ENTRIES)
        .collect::<Vec<_>>();

    let total_lines = visible_entries.len() + usize::from(truncated);
    for (index, (name, is_dir)) in visible_entries.into_iter().enumerate() {
        let is_last = index + 1 == total_lines && !truncated;
        preview_lines.push(format!(
            "{} {}{}",
            if is_last { "`--" } else { "|--" },
            name,
            if is_dir { "/" } else { "" }
        ));
    }

    if truncated {
        preview_lines.push("`-- ...".to_string());
    }

    Some(preview_lines.join("\n"))
}

fn directory_display_name(path: &Path) -> String {
    if path == Path::new("/") {
        "/".to_string()
    } else {
        path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tower_lsp::lsp_types::{CompletionItem, Documentation};

    use super::{
        CompletionItemData, CompletionItemPreviewKind, attach_completion_documentation,
        completion_item_data, fallback_completion_documentation,
    };

    fn documentation_value(item: &CompletionItem) -> &str {
        let Some(Documentation::MarkupContent(markup)) = item.documentation.as_ref() else {
            panic!("expected markdown documentation");
        };

        markup.value.as_str()
    }

    #[test]
    fn resolves_utf8_text_preview() {
        let tmp = tempdir().expect("tempdir");
        let file = tmp.path().join("sample.txt");
        std::fs::write(&file, "line one\nline two\nline three\nline four\n").expect("write");

        let mut item = CompletionItem {
            data: Some(serde_json::to_value(completion_item_data(
                &file, false, "File", "sample.txt",
            ))
            .expect("data")),
            documentation: Some(fallback_completion_documentation("File", "sample.txt")),
            ..CompletionItem::default()
        };

        attach_completion_documentation(&mut item);
        let value = documentation_value(&item);
        assert!(value.contains("line one\nline two\nline three"));
        assert!(value.contains("Preview truncated."));
    }

    #[test]
    fn resolves_utf8_bom_and_utf16_text_preview() {
        let tmp = tempdir().expect("tempdir");
        let utf8 = tmp.path().join("utf8.txt");
        std::fs::write(&utf8, b"\xEF\xBB\xBFalpha\nbeta\n").expect("write utf8");

        let utf16 = tmp.path().join("utf16.txt");
        let mut utf16_bytes = vec![0xFF, 0xFE];
        utf16_bytes.extend("gamma\ndelta\n".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&utf16, utf16_bytes).expect("write utf16");

        for file in [&utf8, &utf16] {
            let mut item = CompletionItem {
                data: Some(serde_json::to_value(completion_item_data(
                    file,
                    false,
                    "File",
                    file.file_name().and_then(|name| name.to_str()).unwrap_or("file"),
                ))
                .expect("data")),
                documentation: Some(fallback_completion_documentation("File", "file")),
                ..CompletionItem::default()
            };

            attach_completion_documentation(&mut item);
            let value = documentation_value(&item);
            assert!(value.contains("~~~text"));
        }
    }

    #[test]
    fn binary_files_fall_back_to_base_documentation() {
        let tmp = tempdir().expect("tempdir");
        let file = tmp.path().join("blob.bin");
        std::fs::write(&file, [0x00, 0x01, 0x02, 0x03]).expect("write");

        let fallback = fallback_completion_documentation("File", "blob.bin");
        let mut item = CompletionItem {
            data: Some(serde_json::to_value(completion_item_data(
                &file, false, "File", "blob.bin",
            ))
            .expect("data")),
            documentation: Some(fallback.clone()),
            ..CompletionItem::default()
        };

        attach_completion_documentation(&mut item);
        assert_eq!(item.documentation, Some(fallback));
    }

    #[test]
    fn resolves_directory_preview_with_limit_and_hidden_filtering() {
        let tmp = tempdir().expect("tempdir");
        let directory = tmp.path().join("src");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::create_dir_all(directory.join("z_dir")).expect("mkdir z_dir");
        std::fs::create_dir_all(directory.join("a_dir")).expect("mkdir a_dir");
        std::fs::write(directory.join("main.rs"), "fn main() {}\n").expect("write main");
        std::fs::write(directory.join(".hidden"), "hidden\n").expect("write hidden");
        for index in 0..10 {
            std::fs::write(directory.join(format!("file-{index}.txt")), "x\n").expect("write");
        }

        let mut item = CompletionItem {
            data: Some(serde_json::to_value(completion_item_data(
                &directory, true, "Directory", "src",
            ))
            .expect("data")),
            documentation: Some(fallback_completion_documentation("Directory", "src")),
            ..CompletionItem::default()
        };

        attach_completion_documentation(&mut item);
        let value = documentation_value(&item);
        assert!(value.contains("src/\n|-- a_dir/\n|-- z_dir/"));
        assert!(value.contains("`-- ..."));
        assert!(!value.contains(".hidden"));
    }

    #[test]
    fn leaves_items_without_path_sense_data_unchanged() {
        let fallback = fallback_completion_documentation("Directory", "src");
        let mut item = CompletionItem {
            data: Some(
                serde_json::to_value(CompletionItemData {
                    path: "/tmp/nowhere".to_string(),
                    kind: CompletionItemPreviewKind::Directory,
                    annotation: "Directory".to_string(),
                    name: "src".to_string(),
                })
                .expect("data"),
            ),
            documentation: Some(fallback.clone()),
            ..CompletionItem::default()
        };
        item.data = Some(serde_json::json!({"unexpected": true}));

        attach_completion_documentation(&mut item);
        assert_eq!(item.documentation, Some(fallback));
    }
}
