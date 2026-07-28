use std::any::Any;
use std::ops::Range;
use std::rc::Rc;

use gpui::SharedString;

pub const MAX_UNDO_HISTORY: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Whitespace,
    Word,
    Punctuation,
}

fn char_kind(c: char) -> CharKind {
    if c.is_whitespace() {
        CharKind::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharKind::Word
    } else {
        CharKind::Punctuation
    }
}

pub fn previous_word_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut run_kind: Option<CharKind> = None;
    let mut boundary = 0;
    for (idx, c) in text[..offset].char_indices().rev() {
        let kind = char_kind(c);
        match run_kind {
            None => {
                if kind == CharKind::Whitespace {
                    continue;
                }
                run_kind = Some(kind);
                boundary = idx;
            }
            Some(run) if kind == run => boundary = idx,
            Some(_) => return idx + c.len_utf8(),
        }
    }
    boundary
}

pub fn next_word_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut run_kind: Option<CharKind> = None;
    for (idx, c) in text[offset..].char_indices() {
        let kind = char_kind(c);
        match run_kind {
            None => {
                if kind == CharKind::Whitespace {
                    continue;
                }
                run_kind = Some(kind);
            }
            Some(run) if kind == run => {}
            Some(_) => return offset + idx,
        }
    }
    text.len()
}

pub fn line_start(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

pub fn line_end(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .unwrap_or(text.len())
}

pub fn home_target(text: &str, offset: usize) -> usize {
    let start = line_start(text, offset);
    let end = line_end(text, offset);
    let indent_end = text[start..end]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| start + i)
        .unwrap_or(start);
    if offset == indent_end {
        start
    } else {
        indent_end
    }
}

#[derive(Clone)]
pub struct HistoryEntry {
    pub content: SharedString,
    pub selected_range: Range<usize>,
    pub selection_reversed: bool,
    pub payload: Option<Rc<dyn Any>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Delete,
    Other,
}

pub fn should_coalesce(last: Option<EditKind>, kind: EditKind) -> bool {
    matches!(kind, EditKind::Insert | EditKind::Delete) && last == Some(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_word_boundary_skips_trailing_space_then_word() {
        let text = "hello world";
        assert_eq!(previous_word_boundary(text, 11), 6);
        assert_eq!(previous_word_boundary(text, 6), 0);
        assert_eq!(previous_word_boundary(text, 8), 6);
        assert_eq!(previous_word_boundary(text, 0), 0);
    }

    #[test]
    fn next_word_boundary_skips_leading_space_then_word() {
        let text = "hello world";
        assert_eq!(next_word_boundary(text, 0), 5);
        assert_eq!(next_word_boundary(text, 5), 11);
        assert_eq!(next_word_boundary(text, 2), 5);
        assert_eq!(next_word_boundary(text, 11), 11);
    }

    #[test]
    fn word_boundary_treats_punctuation_as_its_own_run() {
        let text = "foo.bar baz";
        assert_eq!(next_word_boundary(text, 0), 3);
        assert_eq!(next_word_boundary(text, 3), 4);
        assert_eq!(previous_word_boundary(text, 7), 4);
        assert_eq!(previous_word_boundary(text, 4), 3);
    }

    #[test]
    fn word_boundary_respects_utf8_boundaries() {
        let text = "chào bạn";
        assert_eq!("chào".len(), 5);
        assert_eq!(next_word_boundary(text, 0), 5);
        assert_eq!(previous_word_boundary(text, text.len()), 6);
    }

    #[test]
    fn line_start_and_end_bracket_the_current_line() {
        let text = "ab\ncde\nf";
        assert_eq!(line_start(text, 5), 3);
        assert_eq!(line_end(text, 5), 6);
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_end(text, 0), 2);
        assert_eq!(line_start(text, 8), 7);
        assert_eq!(line_end(text, 8), 8);
    }

    #[test]
    fn home_target_toggles_between_indent_and_column_zero() {
        let text = "    code";
        assert_eq!(home_target(text, 8), 4);
        assert_eq!(home_target(text, 4), 0);
        assert_eq!(home_target(text, 0), 4);
    }

    #[test]
    fn home_target_on_unindented_line_is_line_start() {
        let text = "one\ntwo";
        assert_eq!(home_target(text, 6), 4);
        assert_eq!(home_target(text, 4), 4);
    }
}
