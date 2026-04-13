use crossterm::event::{KeyCode, KeyModifiers};

use crate::shared::key_event::KeyEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Insert(char),
    Backspace,
    Delete,
    ClearAll,
    WordForward,
    WordBackward,
    SuggestionsUp,
    SuggestionsDown,
    Accept,
    Cancel,
    Nop,
}

pub fn resolve_edit_command(key: &KeyEvent) -> EditCommand {
    let modifiers = key.modifiers();
    let has_ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let has_alt = modifiers.contains(KeyModifiers::ALT);

    match key.code() {
        KeyCode::Esc => EditCommand::Cancel,
        KeyCode::Enter => EditCommand::Accept,
        KeyCode::Backspace => EditCommand::Backspace,
        KeyCode::Delete => EditCommand::Delete,
        KeyCode::Up => EditCommand::SuggestionsUp,
        KeyCode::Down => EditCommand::SuggestionsDown,
        KeyCode::Char(c) if has_ctrl => match c.to_ascii_lowercase() {
            'c' | 'l' => EditCommand::ClearAll,
            'n' => EditCommand::SuggestionsDown,
            'p' => EditCommand::SuggestionsUp,
            _ => EditCommand::Nop,
        },
        KeyCode::Char(c) if has_alt => match c.to_ascii_lowercase() {
            'f' => EditCommand::WordForward,
            'b' => EditCommand::WordBackward,
            _ => EditCommand::Nop,
        },
        KeyCode::Char(c) => EditCommand::Insert(c),
        _ => EditCommand::Nop,
    }
}

pub fn apply_edit_command(
    command: EditCommand,
    value: &mut String,
    cursor: &mut usize,
    numeric_only: bool,
) -> bool {
    let len = value.chars().count();
    *cursor = (*cursor).min(len);

    match command {
        EditCommand::Insert(c) => {
            if numeric_only && !c.is_ascii_digit() {
                return false;
            }

            insert_char_at(value, cursor, c);
            true
        }
        EditCommand::Backspace => remove_char_before_cursor(value, cursor),
        EditCommand::Delete => remove_char_at_cursor(value, cursor),
        EditCommand::ClearAll => {
            if value.is_empty() && *cursor == 0 {
                false
            } else {
                value.clear();
                *cursor = 0;
                true
            }
        }
        EditCommand::WordForward => move_word_forward(value, cursor),
        EditCommand::WordBackward => move_word_backward(value, cursor),
        EditCommand::SuggestionsUp
        | EditCommand::SuggestionsDown
        | EditCommand::Accept
        | EditCommand::Cancel
        | EditCommand::Nop => false,
    }
}

fn insert_char_at(value: &mut String, cursor: &mut usize, c: char) {
    let byte_idx = char_to_byte_index(value, *cursor);
    value.insert(byte_idx, c);
    *cursor += 1;
}

fn remove_char_before_cursor(value: &mut String, cursor: &mut usize) -> bool {
    if *cursor == 0 {
        return false;
    }

    let start = char_to_byte_index(value, *cursor - 1);
    let end = char_to_byte_index(value, *cursor);
    value.replace_range(start..end, "");
    *cursor -= 1;

    true
}

fn remove_char_at_cursor(value: &mut String, cursor: &mut usize) -> bool {
    let len = value.chars().count();
    if *cursor >= len {
        return false;
    }

    let start = char_to_byte_index(value, *cursor);
    let end = char_to_byte_index(value, *cursor + 1);
    value.replace_range(start..end, "");

    true
}

fn move_word_forward(value: &str, cursor: &mut usize) -> bool {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    let mut idx = (*cursor).min(len);
    let old = idx;

    while idx < len && !is_word_char(chars[idx]) {
        idx += 1;
    }

    while idx < len && is_word_char(chars[idx]) {
        idx += 1;
    }

    *cursor = idx;
    idx != old
}

fn move_word_backward(value: &str, cursor: &mut usize) -> bool {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    let mut idx = (*cursor).min(len);
    let old = idx;

    if idx == 0 {
        return false;
    }

    idx -= 1;
    while idx > 0 && !is_word_char(chars[idx]) {
        idx -= 1;
    }

    while idx > 0 && is_word_char(chars[idx - 1]) {
        idx -= 1;
    }

    *cursor = idx;
    idx != old
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn char_to_byte_index(value: &str, char_idx: usize) -> usize {
    value.char_indices().nth(char_idx).map_or(value.len(), |(idx, _)| idx)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent as CKeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn resolves_ctrl_clear_aliases() {
        let ctrl_c = KeyEvent::from(CKeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let ctrl_l = KeyEvent::from(CKeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));

        assert_eq!(resolve_edit_command(&ctrl_c), EditCommand::ClearAll);
        assert_eq!(resolve_edit_command(&ctrl_l), EditCommand::ClearAll);
    }

    #[test]
    fn inserts_at_cursor_position() {
        let mut value = String::from("helo");
        let mut cursor = 2;

        let changed = apply_edit_command(EditCommand::Insert('l'), &mut value, &mut cursor, false);

        assert!(changed);
        assert_eq!(value, "hello");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn numeric_mode_blocks_non_digits() {
        let mut value = String::from("12");
        let mut cursor = 2;

        let changed = apply_edit_command(EditCommand::Insert('a'), &mut value, &mut cursor, true);

        assert!(!changed);
        assert_eq!(value, "12");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn word_navigation_moves_cursor() {
        let mut value = String::from("hello world");
        let mut cursor = 0;

        let changed_fwd =
            apply_edit_command(EditCommand::WordForward, &mut value, &mut cursor, false);
        assert!(changed_fwd);
        assert_eq!(cursor, 5);

        let changed_back =
            apply_edit_command(EditCommand::WordBackward, &mut value, &mut cursor, false);
        assert!(changed_back);
        assert_eq!(cursor, 0);
    }
}
