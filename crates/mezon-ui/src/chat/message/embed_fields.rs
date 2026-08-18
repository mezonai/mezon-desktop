use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use mezon_store::{EmbedField, EmbedInput, EmbedTextInput, MessageId};

use super::content::{SelectableSectionCursor, SelectableTextContext};
use super::context::RowCtx;
use super::message_actions_panel::render_message_select;
use crate::components::primitives::TextAreaField;

const INPUT_MIN_WIDTH: f32 = 300.0;
const INPUT_MAX_WIDTH: f32 = 500.0;
const INPUT_HEIGHT: f32 = 36.0;
const TEXTAREA_HEIGHT: f32 = 72.0;
const EMBED_SELECT_INDEX_BASE: usize = 1000;

pub fn render_embed_fields(
    fields: &[EmbedField],
    message_id: MessageId,
    selection_context: &SelectableTextContext,
    selection_cursor: &mut SelectableSectionCursor,
    ctx: &RowCtx,
) -> AnyElement {
    let mut grid = div().mt_2().flex().flex_col().gap_2().w_full();
    let mut select_index = EMBED_SELECT_INDEX_BASE;
    for row_fields in group_fields(fields) {
        let multi_column = row_fields.len() > 1;
        let mut row = div().flex().flex_row().gap_4().w_full();
        for field in row_fields {
            row = row.child(render_field(
                field,
                message_id,
                multi_column,
                &mut select_index,
                selection_context,
                selection_cursor,
                ctx,
            ));
        }
        grid = grid.child(row);
    }
    grid.into_any_element()
}

fn group_fields(fields: &[EmbedField]) -> Vec<Vec<&EmbedField>> {
    let mut rows: Vec<Vec<&EmbedField>> = Vec::new();
    for field in fields {
        if field.inline
            && let Some(last) = rows.last_mut()
            && last.first().is_some_and(|f| f.inline)
            && last.len() < 3
        {
            last.push(field);
        } else {
            rows.push(vec![field]);
        }
    }
    rows
}

fn render_field(
    field: &EmbedField,
    message_id: MessageId,
    multi_column: bool,
    select_index: &mut usize,
    selection_context: &SelectableTextContext,
    selection_cursor: &mut SelectableSectionCursor,
    ctx: &RowCtx,
) -> AnyElement {
    let mut column = div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .cursor(gpui::CursorStyle::IBeam);
    if multi_column {
        column = column.flex_1();
    }
    let name = selection_cursor
        .section(&field.name)
        .map(|range| selection_context.text_node(&field.name, range));
    let value_text = field.value.strip_suffix('\n').unwrap_or(&field.value);
    let value = selection_cursor
        .section(&field.value)
        .filter(|_| !value_text.is_empty())
        .map(|range| {
            selection_context.text_node(value_text, range.start..range.start + value_text.len())
        });
    let mut name_row = div()
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(14.))
        .text_color(ctx.theme.tokens.text_theme_message);
    if let Some(name) = name {
        name_row = name_row.child(name);
    }
    let mut value_row = div()
        .text_size(px(14.))
        .text_color(ctx.theme.tokens.text_theme_message);
    if let Some(value) = value {
        value_row = value_row.child(value);
    }
    column = column.child(name_row).child(value_row);
    match field.input.as_ref() {
        Some(EmbedInput::Text(text)) if !text.disabled => {
            column = column.child(render_embed_text_input(message_id, text, ctx));
        }
        Some(EmbedInput::Select(select)) => {
            let index = *select_index;
            *select_index += 1;
            column = column.child(
                div()
                    .w_full()
                    .min_w(px(INPUT_MIN_WIDTH))
                    .max_w(px(INPUT_MAX_WIDTH))
                    .child(render_message_select(select, message_id, index, true, ctx)),
            );
        }
        _ => {}
    }
    column.into_any_element()
}

fn render_embed_text_input(
    message_id: MessageId,
    input: &EmbedTextInput,
    ctx: &RowCtx,
) -> AnyElement {
    let height = if input.multiline {
        TEXTAREA_HEIGHT
    } else {
        INPUT_HEIGHT
    };
    let container = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w(px(INPUT_MIN_WIDTH))
        .max_w(px(INPUT_MAX_WIDTH));
    let key = (message_id, input.id.clone());
    match ctx.embed_inputs.get(&key) {
        Some(state) => container
            .child(TextAreaField::new(state))
            .into_any_element(),
        None => container
            .child(
                div()
                    .h(px(height))
                    .w_full()
                    .rounded(px(4.))
                    .bg(ctx.theme.tokens.bg_markdown_code),
            )
            .into_any_element(),
    }
}
