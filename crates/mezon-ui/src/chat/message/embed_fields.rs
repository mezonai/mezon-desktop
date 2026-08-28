use std::time::Instant;

use gpui::{AnyElement, FontWeight, Rgba, SharedString, div, img, prelude::*, px, rgb};
use mezon_store::{
    EmbedAnimation, EmbedField, EmbedGrid, EmbedInput, EmbedRadio, EmbedRadioOption,
    EmbedTextInput, Message, MessageId, MessagesStore, SpriteAtlas,
};

use super::content::{SelectableSectionCursor, SelectableTextContext};
use super::context::RowCtx;
use super::message_actions_panel::{button_bg, render_message_button, render_message_select};
use crate::components::primitives::TextAreaField;

const INPUT_WIDTH: f32 = 300.0;
const INPUT_HEIGHT: f32 = 36.0;
const TEXTAREA_HEIGHT: f32 = 72.0;
const EMBED_INDEX_STRIDE: usize = 10_000;
const EMBED_SELECT_INDEX_BASE: usize = 1000;
const EMBED_BUTTON_INDEX_BASE: usize = 2000;
const RADIO_SIZE: f32 = 20.0;
const RADIO_DOT_SIZE: f32 = 12.0;
const GRID_SIZE: f32 = 480.0;
const GRID_CELL: u32 = 0xef_f6_ff;
const EMBED_CARD_MAX_WIDTH: f32 = 520.0;
const EMBED_CARD_PADDING_X: f32 = 20.0;
const ANIMATION_BOX_LARGE: f32 = 133.0;
const ANIMATION_BOX_SMALL: f32 = 80.0;

pub fn render_embed_fields(
    fields: &[EmbedField],
    msg: &Message,
    embed_index: usize,
    selection_context: &SelectableTextContext,
    selection_cursor: &mut SelectableSectionCursor,
    ctx: &RowCtx,
) -> AnyElement {
    let mut grid = div().mt_2().flex().flex_col().gap_2().w_full();
    let embed_base = embed_index * EMBED_INDEX_STRIDE;
    let mut select_index = embed_base + EMBED_SELECT_INDEX_BASE;
    let mut button_index = embed_base + EMBED_BUTTON_INDEX_BASE;
    for row_fields in group_fields(fields) {
        let multi_column = row_fields.len() > 1;
        let mut row = div().flex().flex_row().gap_4().w_full();
        for field in row_fields {
            row = row.child(render_field(
                field,
                msg,
                embed_index,
                multi_column,
                &mut select_index,
                &mut button_index,
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

#[allow(clippy::too_many_arguments)]
fn render_field(
    field: &EmbedField,
    msg: &Message,
    embed_index: usize,
    multi_column: bool,
    select_index: &mut usize,
    button_index: &mut usize,
    selection_context: &SelectableTextContext,
    selection_cursor: &mut SelectableSectionCursor,
    ctx: &RowCtx,
) -> AnyElement {
    let message_id = msg.id;
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

    let text_block = div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .cursor(gpui::CursorStyle::IBeam)
        .child(name_row)
        .child(value_row);

    let has_buttons = !field.buttons.is_empty();
    let mut column = div().flex().min_w_0();
    column = if has_buttons {
        column.flex_row().justify_between().items_center().gap_2()
    } else {
        column.flex_col()
    };
    if multi_column {
        column = column.flex_1();
    } else {
        column = column.w_full();
    }
    column = column.child(text_block);

    match field.input.as_ref() {
        Some(EmbedInput::Text(text)) => {
            column = column.child(render_embed_text_input(message_id, text, ctx));
        }
        Some(EmbedInput::Select(select)) => {
            let index = *select_index;
            *select_index += 1;
            column = column.child(
                div()
                    .w(px(INPUT_WIDTH))
                    .child(render_message_select(select, message_id, index, true, ctx)),
            );
        }
        Some(EmbedInput::DatePicker(picker)) => {
            column = column.child(render_embed_date_picker(message_id, &picker.id, ctx));
        }
        Some(EmbedInput::Radio(radio)) => {
            column = column.child(render_embed_radio(message_id, embed_index, radio, ctx));
        }
        Some(EmbedInput::Animation(animation)) => {
            column = column.child(render_embed_animation(message_id, animation, ctx));
        }
        None => {}
    }

    if let Some(shape) = field.shape.as_ref() {
        column = column.child(render_embed_grid(shape));
    }

    if has_buttons {
        let sender_id = msg.sender_id.parse::<i64>().unwrap_or(0);
        let user_id = ctx.current_user_id.parse::<i64>().unwrap_or(0);
        let mut buttons = div().flex().flex_row().gap_1().flex_shrink_0();
        for button in &field.buttons {
            let index = *button_index;
            *button_index += 1;
            buttons = buttons.child(render_message_button(
                button, message_id, sender_id, user_id, index, true, ctx,
            ));
        }
        column = column.child(buttons);
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
    let container = div().flex().flex_col().w(px(INPUT_WIDTH));
    if input.disabled {
        let text = if input.default_value.is_empty() {
            input.placeholder.clone()
        } else {
            input.default_value.clone()
        };
        return container
            .child(
                div()
                    .h(px(height))
                    .w_full()
                    .flex()
                    .items_center()
                    .px_3()
                    .rounded(px(4.))
                    .bg(ctx.theme.tokens.bg_markdown_code)
                    .text_size(px(14.))
                    .text_color(ctx.theme.tokens.text_theme_primary)
                    .opacity(0.6)
                    .child(text),
            )
            .into_any_element();
    }
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

fn render_embed_date_picker(
    message_id: MessageId,
    input_id: &SharedString,
    ctx: &RowCtx,
) -> AnyElement {
    let container = div().flex().flex_col().w(px(INPUT_WIDTH));
    let key = (message_id, input_id.clone());
    match ctx.embed_date_pickers.get(&key) {
        Some(picker) => container.child(picker.clone()).into_any_element(),
        None => container
            .child(
                div()
                    .h(px(INPUT_HEIGHT))
                    .w_full()
                    .rounded(px(4.))
                    .bg(ctx.theme.tokens.bg_markdown_code),
            )
            .into_any_element(),
    }
}

fn render_embed_radio(
    message_id: MessageId,
    embed_index: usize,
    radio: &EmbedRadio,
    ctx: &RowCtx,
) -> AnyElement {
    let selected: Vec<SharedString> = MessagesStore::try_global(ctx.app)
        .map(|store| {
            store
                .read(ctx.app)
                .message_select_selection(message_id, &radio.id)
                .to_vec()
        })
        .unwrap_or_default();
    let mut column = div().flex().flex_col().w_full().max_w(px(INPUT_WIDTH));
    for (index, option) in radio.options.iter().enumerate() {
        column = column.child(render_embed_radio_option(
            message_id,
            embed_index,
            radio,
            option,
            index,
            selected.contains(&option.value),
            ctx,
        ));
    }
    column.into_any_element()
}

fn render_embed_radio_option(
    message_id: MessageId,
    embed_index: usize,
    radio: &EmbedRadio,
    option: &EmbedRadioOption,
    index: usize,
    checked: bool,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let mut labels = div().flex().flex_col().min_w_0().flex_1();
    if !option.label.is_empty() {
        labels = labels.child(
            div()
                .mt_2()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_message)
                .child(option.label.clone()),
        );
    }
    if !option.description.is_empty() {
        labels = labels.child(
            div()
                .mt_2()
                .text_size(px(14.))
                .text_color(theme.tokens.text_theme_primary)
                .child(option.description.clone()),
        );
    }

    let accent: Rgba = option
        .style
        .map_or(theme.tokens.text_theme_primary, button_bg);
    let mut button = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(RADIO_SIZE))
        .rounded_full()
        .border_2()
        .border_color(if checked {
            accent
        } else {
            theme.tokens.text_theme_primary
        });
    if checked {
        button = button.child(
            div()
                .size(px(RADIO_DOT_SIZE))
                .rounded_full()
                .bg(accent)
                .into_any_element(),
        );
    }
    let mut row = div()
        .id(SharedString::from(format!(
            "embed-radio-{}-{embed_index}-{}-{index}",
            message_id.get(),
            radio.id
        )))
        .flex()
        .flex_row()
        .justify_between()
        .items_center()
        .gap_4()
        .child(labels)
        .child(button);
    if !option.disabled {
        let radio_id = radio.id.clone();
        let multiple = radio.allows_multiple();
        let max_options = radio.max_options;
        let value = option.value.clone();
        row = row.cursor_pointer().on_click(move |_, _, cx| {
            let radio_id = radio_id.clone();
            let value = value.clone();
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.choose_embed_radio(message_id, radio_id, multiple, max_options, value, cx);
            });
        });
    }

    row.into_any_element()
}

fn render_embed_grid(grid: &EmbedGrid) -> AnyElement {
    let cell_width = GRID_SIZE / grid.columns as f32;
    let cell_height = GRID_SIZE / grid.rows as f32;
    let mut board = div()
        .relative()
        .w(px(GRID_SIZE))
        .max_w(px(GRID_SIZE))
        .h(px(GRID_SIZE))
        .overflow_hidden();
    for item in &grid.items {
        let left = (item.start_col.saturating_sub(1)) as f32 * cell_width;
        let top = (item.start_row.saturating_sub(1)) as f32 * cell_height;
        board = board.child(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(item.width as f32 * cell_width))
                .h(px(item.height as f32 * cell_height))
                .bg(rgb(GRID_CELL)),
        );
    }
    board.into_any_element()
}

fn render_embed_animation(
    message_id: MessageId,
    animation: &EmbedAnimation,
    ctx: &RowCtx,
) -> AnyElement {
    let mut row = div().flex().min_w_0().w_full().gap_2().rounded_md();
    if animation.vertical {
        row = row.flex_col();
    }
    let Some(atlas) = ctx.sprite_atlases.get(&animation.url_position) else {
        let side = px(animation_box_size(animation, ctx));
        for _ in &animation.pool {
            row = row.child(div().w(side).h(side));
        }
        return row.into_any_element();
    };
    for frames in &animation.pool {
        if let Some(box_element) = render_animation_box(message_id, animation, atlas, frames, ctx) {
            row = row.child(box_element);
        }
    }
    row.into_any_element()
}

fn animation_box_size(animation: &EmbedAnimation, ctx: &RowCtx) -> f32 {
    let card = if ctx.content_width > 0. {
        ctx.content_width.min(EMBED_CARD_MAX_WIDTH)
    } else {
        EMBED_CARD_MAX_WIDTH
    };
    let available = (card - EMBED_CARD_PADDING_X * 2.).max(0.);
    if available > ANIMATION_BOX_LARGE * animation.pool.len() as f32 {
        ANIMATION_BOX_LARGE
    } else {
        ANIMATION_BOX_SMALL
    }
}

fn render_animation_box(
    message_id: MessageId,
    animation: &EmbedAnimation,
    atlas: &SpriteAtlas,
    frames: &[SharedString],
    ctx: &RowCtx,
) -> Option<AnyElement> {
    let rects: Vec<_> = frames
        .iter()
        .filter_map(|name| atlas.frame(name))
        .filter(|frame| frame.width > 0. && frame.height > 0.)
        .collect();
    let first = *rects.first()?;
    let ratio = animation_box_size(animation, ctx) / first.width.min(first.height);
    let width = first.width * ratio;
    let height = first.height * ratio;
    let sheet_width = atlas.sheet_width * ratio;
    let sheet_height = atlas.sheet_height * ratio;

    let sheet = div()
        .image_cache(ctx.sprite_cache.clone())
        .absolute()
        .w(px(sheet_width))
        .h(px(sheet_height))
        .child(img(animation.url_image.clone()).size_full());

    let last = *rects.last()?;
    let clipped = div()
        .relative()
        .w(px(width))
        .h(px(height))
        .overflow_hidden();

    if animation.is_result || rects.len() == 1 || !ctx.window_active {
        return Some(
            clipped
                .child(sheet.left(px(-last.x * ratio)).top(px(-last.y * ratio)))
                .into_any_element(),
        );
    }
    let step = animation_step(message_id, animation, rects.len(), ctx);
    let frame = rects[step];
    Some(
        clipped
            .child(sheet.left(px(-frame.x * ratio)).top(px(-frame.y * ratio)))
            .into_any_element(),
    )
}

fn animation_step(
    message_id: MessageId,
    animation: &EmbedAnimation,
    count: usize,
    ctx: &RowCtx,
) -> usize {
    let Some(started) = ctx
        .animation_starts
        .get(&(message_id, animation.id.clone()))
    else {
        return 0;
    };
    let elapsed = Instant::now()
        .saturating_duration_since(*started)
        .as_secs_f32();
    let cycle = animation.duration_seconds.max(0.05);
    let cycles_done = elapsed / cycle;
    if animation
        .repeat
        .is_some_and(|repeat| cycles_done >= repeat as f32)
    {
        return count - 1;
    }
    ((cycles_done.fract() * count as f32) as usize).min(count - 1)
}
