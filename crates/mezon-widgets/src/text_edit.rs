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

pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let before = text[..offset].chars().next_back().map(char_kind);
    let after = text[offset..].chars().next().map(char_kind);
    let kind = match (before, after) {
        (Some(b), Some(a)) if b != CharKind::Whitespace && a != CharKind::Whitespace => {
            if b == CharKind::Word { b } else { a }
        }
        (_, Some(a)) if a != CharKind::Whitespace => a,
        (Some(b), _) if b != CharKind::Whitespace => b,
        (_, Some(a)) => a,
        (Some(b), None) => b,
        (None, None) => return 0..0,
    };
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| char_kind(*c) == kind)
        .map(|(idx, _)| idx)
        .last()
        .unwrap_or(offset);
    let end = text[offset..]
        .char_indices()
        .take_while(|(_, c)| char_kind(*c) == kind)
        .map(|(idx, c)| offset + idx + c.len_utf8())
        .last()
        .unwrap_or(offset);
    start..end
}

pub fn line_range_at(text: &str, offset: usize) -> Range<usize> {
    line_start(text, offset)..line_end(text, offset)
}

pub fn byte_offset_from_utf16(text: &str, offset_utf16: usize) -> usize {
    let mut utf16_count = 0;
    for (byte_ix, ch) in text.char_indices() {
        if utf16_count >= offset_utf16 {
            return byte_ix;
        }
        utf16_count += ch.len_utf16();
    }
    text.len()
}

pub fn byte_range_from_utf16(text: &str, range_utf16: &Range<usize>) -> Range<usize> {
    let start = byte_offset_from_utf16(text, range_utf16.start);
    let end = byte_offset_from_utf16(text, range_utf16.end.max(range_utf16.start));
    start..end
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SelectGranularity {
    #[default]
    Character,
    Word,
    Line,
}

pub fn granularity_for_click(click_count: usize) -> SelectGranularity {
    match click_count {
        0 | 1 => SelectGranularity::Character,
        2 => SelectGranularity::Word,
        _ => SelectGranularity::Line,
    }
}

pub fn range_for_granularity(
    text: &str,
    offset: usize,
    granularity: SelectGranularity,
    multi_line: bool,
) -> Range<usize> {
    match granularity {
        SelectGranularity::Character => offset..offset,
        SelectGranularity::Word => word_range_at(text, offset),
        SelectGranularity::Line if multi_line => line_range_at(text, offset),
        SelectGranularity::Line => 0..text.len(),
    }
}

pub fn extend_range_for_granularity(
    text: &str,
    anchor: &Range<usize>,
    offset: usize,
    granularity: SelectGranularity,
    multi_line: bool,
) -> (Range<usize>, bool) {
    let unit = range_for_granularity(text, offset, granularity, multi_line);
    if unit.start < anchor.start {
        (unit.start..anchor.end, true)
    } else {
        (anchor.start..unit.end.max(anchor.end), false)
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

    #[test]
    fn word_range_covers_the_word_under_the_caret() {
        let text = "foo bar baz";
        assert_eq!(word_range_at(text, 5), 4..7);
        assert_eq!(word_range_at(text, 4), 4..7);
        assert_eq!(word_range_at(text, 7), 4..7);
    }

    #[test]
    fn word_range_prefers_the_word_before_a_trailing_space() {
        let text = "foo bar";
        assert_eq!(word_range_at(text, 3), 0..3);
        assert_eq!(word_range_at(text, 7), 4..7);
    }

    #[test]
    fn word_range_treats_punctuation_as_its_own_run() {
        let text = "foo.bar";
        assert_eq!(word_range_at(text, 0), 0..3);
        assert_eq!(word_range_at(text, 4), 4..7);
    }

    #[test]
    fn word_range_at_a_run_boundary_prefers_the_word_over_the_punctuation() {
        let text = "foo.bar";
        assert_eq!(word_range_at(text, 3), 0..3);
        assert_eq!(word_range_at(text, 4), 4..7);
    }

    #[test]
    fn word_range_respects_utf8_boundaries() {
        let text = "chào bạn";
        assert_eq!(word_range_at(text, 2), 0..5);
        assert_eq!(word_range_at(text, 7), 6..text.len());
    }

    #[test]
    fn word_range_on_empty_text_is_empty() {
        assert_eq!(word_range_at("", 0), 0..0);
        assert_eq!(word_range_at("   ", 1), 0..3);
    }

    #[test]
    fn triple_click_selects_the_line_only_when_multi_line() {
        let text = "one\ntwo";
        let line = SelectGranularity::Line;
        assert_eq!(range_for_granularity(text, 5, line, true), 4..7);
        assert_eq!(range_for_granularity(text, 5, line, false), 0..7);
    }

    #[test]
    fn granularity_maps_click_count_to_word_then_line() {
        assert_eq!(granularity_for_click(1), SelectGranularity::Character);
        assert_eq!(granularity_for_click(2), SelectGranularity::Word);
        assert_eq!(granularity_for_click(3), SelectGranularity::Line);
        assert_eq!(granularity_for_click(4), SelectGranularity::Line);
    }

    #[test]
    fn dragging_by_word_keeps_the_anchor_word_whole() {
        let text = "foo bar baz";
        let anchor = 4..7;
        let word = SelectGranularity::Word;

        let (forward, reversed) = extend_range_for_granularity(text, &anchor, 9, word, false);
        assert_eq!(forward, 4..11);
        assert!(!reversed);

        let (backward, reversed) = extend_range_for_granularity(text, &anchor, 1, word, false);
        assert_eq!(backward, 0..7);
        assert!(reversed);
    }

    #[test]
    fn byte_offsets_from_utf16_are_relative_to_the_given_text() {
        assert_eq!(byte_offset_from_utf16("Ư", 0), 0);
        assert_eq!(byte_offset_from_utf16("Ư", 1), 2);
        assert_eq!(byte_offset_from_utf16("Ư", 9), 2);
        assert_eq!(byte_offset_from_utf16("", 0), 0);
        assert_eq!(byte_offset_from_utf16("😀a", 2), 4);
    }

    #[test]
    fn ime_caret_after_a_composed_vietnamese_char_lands_on_a_char_boundary() {
        let content = ":";
        let new_text = "Ư";
        let insert_at = content.len();
        let caret = byte_range_from_utf16(new_text, &(1..1));
        let cursor = insert_at + caret.end;

        let composed = format!("{content}{new_text}");
        assert_eq!(cursor, composed.len());
        assert!(composed.is_char_boundary(cursor));
    }

    #[test]
    fn byte_range_from_utf16_never_yields_an_inverted_range() {
        let inverted = Range { start: 1, end: 0 };
        assert_eq!(byte_range_from_utf16("Ư", &inverted), 2..2);
    }

    #[test]
    fn dragging_back_inside_the_anchor_word_does_not_shrink_it() {
        let text = "foo bar baz";
        let anchor = 4..7;
        let (range, reversed) =
            extend_range_for_granularity(text, &anchor, 5, SelectGranularity::Word, false);
        assert_eq!(range, 4..7);
        assert!(!reversed);
    }
}
