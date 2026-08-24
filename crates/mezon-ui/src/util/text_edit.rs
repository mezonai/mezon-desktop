pub(crate) use mezon_widgets::text_edit::{
    EditKind, HistoryEntry, MAX_UNDO_HISTORY, SelectGranularity, extend_range_for_granularity,
    granularity_for_click, home_target, ime_replace_range, line_end, line_start,
    marked_caret_range, next_word_boundary, previous_word_boundary, range_for_granularity,
    should_coalesce, surrounding_delete_range, swallow_discarded_ime_commit,
};
