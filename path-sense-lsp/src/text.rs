use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::{InputEdit, Point};

#[must_use]
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let target_line = usize::try_from(position.line).ok()?;
    let target_character = usize::try_from(position.character).ok()?;
    let mut line = 0usize;
    let mut character = 0usize;
    let mut index = 0usize;

    for (byte_index, ch) in text.char_indices() {
        if line == target_line && character == target_character {
            return Some(byte_index);
        }
        if ch == '\n' {
            line += 1;
            character = 0;
            if line > target_line {
                return None;
            }
        } else {
            character += ch.len_utf16();
        }
        index = byte_index + ch.len_utf8();
    }

    if line == target_line && character == target_character {
        Some(index)
    } else if target_line == 0 && target_character == 0 && text.is_empty() {
        Some(0)
    } else {
        None
    }
}

#[must_use]
pub fn offset_to_position(text: &str, offset: usize) -> Option<Position> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }

    let mut line = 0u32;
    let mut character = 0u32;

    for (byte_index, ch) in text.char_indices() {
        if byte_index == offset {
            return Some(Position::new(line, character));
        }

        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += u32::try_from(ch.len_utf16()).ok()?;
        }
    }

    if offset == text.len() {
        Some(Position::new(line, character))
    } else {
        None
    }
}

#[must_use]
pub fn range_from_offsets(text: &str, start: usize, end: usize) -> Option<Range> {
    Some(Range::new(
        offset_to_position(text, start)?,
        offset_to_position(text, end)?,
    ))
}

#[must_use]
pub fn offset_to_point(text: &str, offset: usize) -> Option<Point> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }

    let mut row = 0usize;
    let mut line_start = 0usize;
    for (byte_index, ch) in text.char_indices() {
        if byte_index == offset {
            return Some(Point {
                row,
                column: offset - line_start,
            });
        }

        if ch == '\n' {
            row += 1;
            line_start = byte_index + 1;
        }
    }

    if offset == text.len() {
        Some(Point {
            row,
            column: offset - line_start,
        })
    } else {
        None
    }
}

pub fn apply_range_change(text: &mut String, range: Range, new_text: &str) -> Option<InputEdit> {
    let start_byte = position_to_offset(text, range.start)?;
    let old_end_byte = position_to_offset(text, range.end)?;
    let start_position = offset_to_point(text, start_byte)?;
    let old_end_position = offset_to_point(text, old_end_byte)?;
    let new_end_position = advance_point(start_position, new_text);
    let new_end_byte = start_byte + new_text.len();

    text.replace_range(start_byte..old_end_byte, new_text);

    Some(InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position,
        old_end_position,
        new_end_position,
    })
}

fn advance_point(mut point: Point, text: &str) -> Point {
    for ch in text.chars() {
        if ch == '\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += ch.len_utf8();
        }
    }

    point
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_range_change_returns_expected_input_edit() {
        let mut text = "let path = \"./src/ma\";".to_string();
        let edit = apply_range_change(
            &mut text,
            Range::new(Position::new(0, 18), Position::new(0, 20)),
            "in",
        )
        .expect("edit");

        assert_eq!(text, "let path = \"./src/in\";");
        assert_eq!(edit.start_byte, 18);
        assert_eq!(edit.old_end_byte, 20);
        assert_eq!(edit.new_end_byte, 20);
    }
}
