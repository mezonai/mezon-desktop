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

pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    if text.is_char_boundary(index) {
        return index;
    }
    let mut i = index;
    while i > 0 {
        i -= 1;
        if text.is_char_boundary(i) {
            return i;
        }
    }
    0
}

pub fn ceil_char_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    if text.is_char_boundary(index) {
        return index;
    }
    let mut i = index + 1;
    while i < text.len() {
        if text.is_char_boundary(i) {
            return i;
        }
        i += 1;
    }
    text.len()
}

pub fn ime_replace_range(selected: &Range<usize>, marked: Option<&Range<usize>>) -> Range<usize> {
    marked.cloned().unwrap_or_else(|| selected.clone())
}

fn offset_after_delete(offset: usize, deleted: &Range<usize>) -> usize {
    if offset <= deleted.start {
        offset
    } else if offset <= deleted.end {
        deleted.start
    } else {
        offset - (deleted.end - deleted.start)
    }
}

pub fn marked_range_after_delete(
    marked: Option<&Range<usize>>,
    deleted: &Range<usize>,
) -> Option<Range<usize>> {
    let marked = marked?;
    let start = offset_after_delete(marked.start, deleted);
    let end = offset_after_delete(marked.end, deleted);
    (start < end).then_some(start..end)
}

pub fn swallow_discarded_ime_commit(
    discard: &mut Option<String>,
    _range_utf16: Option<&Range<usize>>,
    _has_marked: bool,
    new_text: &str,
) -> bool {
    let Some(expected) = discard.as_deref() else {
        return false;
    };
    if new_text.is_empty() {
        return false;
    }
    if new_text.chars().count() < expected.chars().count() {
        return false;
    }
    if ime_token_eq(expected, new_text) {
        return true;
    }
    *discard = None;
    false
}

fn ime_token_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let folded_a = fold_ime_token(a);
    let folded_b = fold_ime_token(b);
    folded_a == folded_b && folded_a.chars().count() > 1
}

fn fold_ime_token(s: &str) -> String {
    s.chars()
        .filter(|c| !is_combining_mark(*c))
        .map(fold_latin_letter)
        .collect()
}

fn is_combining_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}')
}

fn fold_latin_letter(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'á' | 'ả' | 'ã' | 'ạ' | 'ă' | 'ằ' | 'ắ' | 'ẳ' | 'ẵ' | 'ặ' | 'â' | 'ầ' | 'ấ' | 'ẩ'
        | 'ẫ' | 'ậ' => 'a',
        'è' | 'é' | 'ẻ' | 'ẽ' | 'ẹ' | 'ê' | 'ề' | 'ế' | 'ể' | 'ễ' | 'ệ' => 'e',
        'ì' | 'í' | 'ỉ' | 'ĩ' | 'ị' => 'i',
        'ò' | 'ó' | 'ỏ' | 'õ' | 'ọ' | 'ô' | 'ồ' | 'ố' | 'ổ' | 'ỗ' | 'ộ' | 'ơ' | 'ờ' | 'ớ' | 'ở'
        | 'ỡ' | 'ợ' => 'o',
        'ù' | 'ú' | 'ủ' | 'ũ' | 'ụ' | 'ư' | 'ừ' | 'ứ' | 'ử' | 'ữ' | 'ự' => 'u',
        'ỳ' | 'ý' | 'ỷ' | 'ỹ' | 'ỵ' => 'y',
        'đ' => 'd',
        _ => c,
    }
}

pub fn surrounding_delete_range(
    text: &str,
    selected_range: &Range<usize>,
    marked_range: Option<&Range<usize>>,
    selection_reversed: bool,
    before_len: usize,
    after_len: usize,
) -> Range<usize> {
    if selected_range.start != selected_range.end {
        return selected_range.start.min(text.len())..selected_range.end.min(text.len());
    }
    let (left, right) = if let Some(marked) = marked_range {
        (marked.start.min(text.len()), marked.end.min(text.len()))
    } else {
        let caret = if selection_reversed {
            selected_range.start
        } else {
            selected_range.end
        }
        .min(text.len());
        (caret, caret)
    };
    let start = floor_char_boundary(text, left.saturating_sub(before_len));
    let end = ceil_char_boundary(text, right.saturating_add(after_len));
    start..end
}

pub fn utf16_offset_to_bytes(text: &str, utf16_offset: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= utf16_offset {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

pub fn marked_caret_range(
    marked_start: usize,
    new_text: &str,
    selected_utf16: Option<&Range<usize>>,
) -> Range<usize> {
    match selected_utf16 {
        Some(range) => {
            let start = marked_start + utf16_offset_to_bytes(new_text, range.start);
            let end = marked_start + utf16_offset_to_bytes(new_text, range.end);
            start..end
        }
        None => {
            let end = marked_start + new_text.len();
            end..end
        }
    }
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
    fn word_range_treats_vietnamese_letters_as_one_word() {
        let text = "tiếng Việt";
        assert_eq!(word_range_at(text, 2), 0.."tiếng".len());
        assert_eq!(next_word_boundary(text, 0), "tiếng".len());
        assert_eq!(previous_word_boundary(text, text.len()), "tiếng ".len());
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
    fn dragging_back_inside_the_anchor_word_does_not_shrink_it() {
        let text = "foo bar baz";
        let anchor = 4..7;
        let (range, reversed) =
            extend_range_for_granularity(text, &anchor, 5, SelectGranularity::Word, false);
        assert_eq!(range, 4..7);
        assert!(!reversed);
    }

    #[test]
    fn ime_replace_uses_marked_when_selection_is_inside_preedit() {
        assert_eq!(ime_replace_range(&(2..4), Some(&(0..5))), 0..5);
    }

    #[test]
    fn ime_replace_uses_marked_when_preedit_is_present() {
        assert_eq!(ime_replace_range(&(0..6), Some(&(3..5))), 3..5);
    }

    #[test]
    fn marked_range_shrinks_after_tail_delete_within_preedit() {
        let text = "được";
        let len = text.len();
        assert_eq!(
            marked_range_after_delete(Some(&(0..len)), &(len - 1..len)),
            Some(0..len - 1)
        );
    }

    #[test]
    fn marked_range_clears_after_full_preedit_delete() {
        let text = "được";
        let len = text.len();
        assert_eq!(marked_range_after_delete(Some(&(0..len)), &(0..len)), None);
    }

    #[test]
    fn marked_range_shifts_left_when_delete_is_before_it() {
        assert_eq!(
            marked_range_after_delete(Some(&(2..5)), &(0..1)),
            Some(1..4)
        );
        assert_eq!(
            marked_range_after_delete(Some(&(2..5)), &(0..2)),
            Some(0..3)
        );
    }

    #[test]
    fn marked_range_is_untouched_when_delete_is_after_it() {
        assert_eq!(
            marked_range_after_delete(Some(&(2..5)), &(5..8)),
            Some(2..5)
        );
    }

    #[test]
    fn marked_range_keeps_the_surviving_side_on_partial_overlap() {
        assert_eq!(
            marked_range_after_delete(Some(&(2..6)), &(1..3)),
            Some(1..4)
        );
        assert_eq!(
            marked_range_after_delete(Some(&(2..6)), &(5..8)),
            Some(2..5)
        );
    }

    #[test]
    fn ime_replace_with_none_deletes_whole_mark_not_partial_selection() {
        let selected = 2..4;
        let marked = 0..5;
        assert_eq!(ime_replace_range(&selected, Some(&marked)), marked);
        assert_ne!(ime_replace_range(&selected, Some(&marked)), selected);
    }

    #[test]
    fn discarded_ime_commit_swallows_the_echoed_preedit() {
        let mut discard = Some("hoa".to_string());
        assert!(swallow_discarded_ime_commit(
            &mut discard,
            None,
            false,
            "hoa"
        ));
        assert_eq!(discard.as_deref(), Some("hoa"));
    }

    #[test]
    fn discarded_ime_commit_swallows_composed_vietnamese() {
        let mut discard = Some("ban".to_string());
        assert!(swallow_discarded_ime_commit(
            &mut discard,
            None,
            false,
            "bạn"
        ));
        assert_eq!(discard.as_deref(), Some("ban"));
    }

    #[test]
    fn discarded_ime_commit_does_not_swallow_the_next_sentence_first_char() {
        let mut discard = Some("ban".to_string());
        assert!(!swallow_discarded_ime_commit(
            &mut discard,
            None,
            false,
            "t"
        ));
        assert_eq!(discard.as_deref(), Some("ban"));
        assert!(!swallow_discarded_ime_commit(
            &mut discard,
            None,
            true,
            "to"
        ));
        assert_eq!(discard.as_deref(), Some("ban"));
    }

    #[test]
    fn discarded_ime_commit_keeps_the_token_until_the_echo_arrives() {
        let mut discard = Some("ban".to_string());
        assert!(!swallow_discarded_ime_commit(
            &mut discard,
            None,
            false,
            "t"
        ));
        assert_eq!(discard.as_deref(), Some("ban"));
        assert!(swallow_discarded_ime_commit(
            &mut discard,
            None,
            true,
            "bạn"
        ));
        assert_eq!(discard.as_deref(), Some("ban"));
    }

    #[test]
    fn discarded_ime_commit_clears_on_a_new_word_of_equal_length() {
        let mut discard = Some("ban".to_string());
        assert!(!swallow_discarded_ime_commit(
            &mut discard,
            None,
            false,
            "xin"
        ));
        assert!(discard.is_none());
    }

    #[test]
    fn discarded_ime_commit_does_not_swallow_a_shorter_preedit_update() {
        let mut discard = Some("được".to_string());
        assert!(!swallow_discarded_ime_commit(
            &mut discard,
            None,
            true,
            "đượ"
        ));
        assert_eq!(discard.as_deref(), Some("được"));
    }

    #[test]
    fn discarded_ime_commit_does_not_swallow_telex_d_as_echo_of_d() {
        let mut discard = Some("đ".to_string());
        assert!(!swallow_discarded_ime_commit(
            &mut discard,
            None,
            false,
            "d"
        ));
        assert!(discard.is_none());
    }

    #[test]
    fn surrounding_delete_removes_the_vowel_before_a_telex_tone() {
        assert_eq!(
            surrounding_delete_range("hoa", &(3..3), None, false, 1, 0),
            2..3
        );
    }

    #[test]
    fn surrounding_delete_keeps_a_full_selection_instead_of_the_last_marked_char() {
        assert_eq!(
            surrounding_delete_range("hoas", &(0..4), Some(&(3..4)), false, 1, 0),
            0..4
        );
    }

    #[test]
    fn surrounding_delete_snaps_mid_character_offsets_to_utf8_boundaries() {
        let text = "á";
        let end = text.len();
        let range = surrounding_delete_range(text, &(end..end), None, false, 1, 0);
        assert!(text.is_char_boundary(range.start));
        assert!(text.is_char_boundary(range.end));
        assert_eq!(&text[range], "á");
    }

    #[test]
    fn marked_caret_is_relative_to_preedit_not_the_document() {
        let prefix = "câu có nhiều từ dễ gõ sai ";
        let preedit = "dấu";
        let vowel = marked_caret_range(prefix.len(), preedit, Some(&(2..2)));
        assert_eq!(vowel, prefix.len() + "dấ".len()..prefix.len() + "dấ".len());
        let at_end = marked_caret_range(prefix.len(), preedit, None);
        assert_eq!(
            at_end,
            prefix.len() + preedit.len()..prefix.len() + preedit.len()
        );
        assert_eq!(
            marked_caret_range(prefix.len(), preedit, Some(&(3..3))),
            at_end
        );
    }
}
