pub(crate) use mezon_widgets::text_edit::{
    HistoryEntry, SelectGranularity, extend_range_for_granularity, granularity_for_click,
    home_target, ime_replace_range, line_end, line_start, marked_caret_range,
    marked_range_after_delete, next_word_boundary, previous_word_boundary, push_undo_entry,
    range_for_granularity, splice_out_byte_range, surrounding_delete_range,
    swallow_discarded_ime_commit,
};
