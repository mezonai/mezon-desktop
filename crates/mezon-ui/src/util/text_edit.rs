pub(crate) use mezon_widgets::text_edit::{
    EditKind, HistoryEntry, MAX_UNDO_HISTORY, SelectGranularity, byte_range_from_utf16,
    extend_range_for_granularity, granularity_for_click, home_target, line_end, line_start,
    next_word_boundary, previous_word_boundary, range_for_granularity, should_coalesce,
};
