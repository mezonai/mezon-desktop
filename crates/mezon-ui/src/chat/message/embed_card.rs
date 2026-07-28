use gpui::{AnyElement, FontWeight, ObjectFit, SharedString, div, img, prelude::*, px};
use mezon_store::{Embed, EmbedAuthor, EmbedImage, Message, MessageCode};

use super::content::{
    SelectableSectionCursor, SelectableTextContext, render_selectable_embed_description,
    selectable_spans_text,
};
use super::context::RowCtx;
use super::embed_fields::render_embed_fields;
use super::share_contact_card::render_share_contact_card;

const CARD_MAX_WIDTH: f32 = 520.0;
const CARD_RADIUS: f32 = 8.0;
const ACCENT_BAR_WIDTH: f32 = 4.0;
const AUTHOR_ICON_SIZE: f32 = 24.0;
const THUMBNAIL_SIZE: f32 = 64.0;
const FOOTER_ICON_SIZE: f32 = 20.0;
const EMBED_IMAGE_MAX_HEIGHT: f32 = 300.0;
const SHARE_CONTACT_KEY: &str = "share_contact";

pub fn render_embeds(
    msg: &Message,
    ctx: &RowCtx,
    ranges: &[Option<std::ops::Range<usize>>],
    selection_context: &SelectableTextContext,
) -> AnyElement {
    let mut column = div().flex().flex_col().w_full();
    for (index, embed) in msg.embeds.iter().enumerate() {
        let base = ranges
            .get(index)
            .and_then(|range| range.as_ref())
            .map_or(0, |range| range.start);
        column = column.child(if is_share_contact_embed(embed, msg.code) {
            render_share_contact_card(embed, base, selection_context, ctx)
        } else {
            render_embed_card(embed, msg, base, selection_context, ctx)
        });
    }
    column.into_any_element()
}

fn is_share_contact_embed(embed: &Embed, code: MessageCode) -> bool {
    code == MessageCode::ShareContact
        || embed
            .fields
            .first()
            .is_some_and(|field| field.value.as_ref() == SHARE_CONTACT_KEY)
}

pub fn render_embed_card(
    embed: &Embed,
    msg: &Message,
    base: usize,
    selection_context: &SelectableTextContext,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let has_thumbnail = !embed.thumbnail_proxied.is_empty();
    let mut selection_cursor = SelectableSectionCursor::new(base);

    let mut left = div()
        .flex()
        .flex_col()
        .min_w_0()
        .w_full()
        .cursor(gpui::CursorStyle::IBeam);
    if let Some(author) = embed.author.as_ref() {
        let child = selection_cursor
            .section(&author.name)
            .map(|range| selection_context.text_node(&author.name, range));
        left = left.child(render_embed_author(author, child, ctx));
    }
    if let Some(range) = selection_cursor.section(&embed.title) {
        left = left.child(render_embed_title(
            selection_context.text_node(&embed.title, range),
            ctx,
        ));
    }
    if !embed.description_spans.is_empty() {
        let description = selectable_spans_text(&embed.description_spans, ctx.locale, ctx.app);
        if let Some(range) = selection_cursor.section(&description) {
            left = left.child(div().mt_2().w_full().min_w_0().overflow_hidden().child(
                render_selectable_embed_description(
                    &embed.description_spans,
                    msg,
                    range.start,
                    selection_context,
                    ctx,
                    theme.tokens.text_theme_primary,
                    px(14.),
                ),
            ));
        }
    }
    if !embed.fields.is_empty() {
        left = left.child(render_embed_fields(
            &embed.fields,
            msg.id,
            selection_context,
            &mut selection_cursor,
            ctx,
        ));
    }

    let mut inner = div().flex().flex_col().px_5().pt_2().pb_4();
    if has_thumbnail {
        inner = inner.child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .gap_2()
                .child(left.flex_1())
                .child(render_embed_thumbnail(&embed.thumbnail_proxied, ctx)),
        );
    } else {
        inner = inner.child(left);
    }
    if let Some(image) = embed.image.as_ref() {
        inner = inner.child(render_embed_image(image));
    }
    if embed.footer.is_some() || !embed.timestamp.is_empty() {
        inner = inner.child(render_embed_footer(
            embed,
            selection_context,
            &mut selection_cursor,
            ctx,
        ));
    }

    div()
        .relative()
        .w_full()
        .max_w(px(CARD_MAX_WIDTH))
        .mt_2()
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
        .bg(theme.tokens.theme_setting_primary)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .text_color(theme.tokens.text_theme_message)
        .shadow_sm()
        .when_some(embed.accent, |d, accent| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(CARD_RADIUS))
                    .bottom(px(CARD_RADIUS))
                    .w(px(ACCENT_BAR_WIDTH))
                    .bg(accent),
            )
        })
        .child(inner)
        .into_any_element()
}

fn render_embed_circle_icon(url: SharedString, size: f32, ctx: &RowCtx) -> AnyElement {
    let size = px(size);
    div()
        .size(size)
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .image_cache(ctx.avatar_cache.clone())
        .child(
            img(url)
                .size(size)
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        )
        .into_any_element()
}

fn render_embed_author(
    author: &EmbedAuthor,
    name: Option<gpui::StyledText>,
    ctx: &RowCtx,
) -> AnyElement {
    let mut row = div()
        .flex()
        .items_center()
        .gap_2()
        .mt_2()
        .min_w_0()
        .w_full();
    if !author.icon_proxied.is_empty() {
        row = row.child(render_embed_circle_icon(
            author.icon_proxied.clone(),
            AUTHOR_ICON_SIZE,
            ctx,
        ));
    }
    row.child(
        div()
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ctx.theme.tokens.text_theme_message)
            .child(name.unwrap_or_else(|| gpui::StyledText::new(author.name.clone()))),
    )
    .into_any_element()
}

fn render_embed_title(title: gpui::StyledText, ctx: &RowCtx) -> AnyElement {
    div()
        .mt_2()
        .min_w_0()
        .w_full()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ctx.theme.tokens.text_theme_message)
        .child(title)
        .into_any_element()
}

fn render_embed_thumbnail(url: &SharedString, ctx: &RowCtx) -> AnyElement {
    div()
        .flex_shrink_0()
        .mt_4()
        .size(px(THUMBNAIL_SIZE))
        .rounded(px(4.))
        .overflow_hidden()
        .image_cache(ctx.avatar_cache.clone())
        .child(
            img(url.clone())
                .size(px(THUMBNAIL_SIZE))
                .object_fit(ObjectFit::Cover),
        )
        .into_any_element()
}

fn render_embed_image(image: &EmbedImage) -> AnyElement {
    let has_aspect = matches!((image.width, image.height), (Some(w), Some(h)) if w > 0 && h > 0);
    let mut container = div()
        .mt_2()
        .w_full()
        .max_h(px(EMBED_IMAGE_MAX_HEIGHT))
        .rounded(px(4.))
        .overflow_hidden();
    if let (Some(w), Some(h)) = (image.width, image.height) {
        if w > 0 && h > 0 {
            container = container.aspect_ratio(w as f32 / h as f32);
        } else if w > 0 {
            container = container.max_w(px(w as f32));
        }
    }
    let image_element = if has_aspect {
        img(image.url_proxied.clone())
            .size_full()
            .object_fit(ObjectFit::Contain)
    } else {
        img(image.url_proxied.clone())
            .w_full()
            .max_h(px(EMBED_IMAGE_MAX_HEIGHT))
            .object_fit(ObjectFit::Contain)
    };
    container.child(image_element).into_any_element()
}

fn render_embed_footer(
    embed: &Embed,
    selection_context: &SelectableTextContext,
    selection_cursor: &mut SelectableSectionCursor,
    ctx: &RowCtx,
) -> AnyElement {
    let footer = embed.footer.as_ref();
    let icon = footer
        .map(|f| f.icon_proxied.clone())
        .filter(|s| !s.is_empty());
    let text = footer.map(|f| f.text.clone()).filter(|s| !s.is_empty());
    let date = embed.footer_date.clone();
    let has_text = text.is_some();

    let mut row = div()
        .mt_2()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .w_full();
    if let Some(icon_url) = icon {
        row = row.child(render_embed_circle_icon(icon_url, FOOTER_ICON_SIZE, ctx));
    }

    let mut inner = div()
        .flex()
        .flex_1()
        .min_w_0()
        .flex_wrap()
        .gap_2()
        .items_center()
        .text_size(px(12.));
    if let Some(text) = text
        && let Some(range) = selection_cursor.section(&text)
    {
        inner = inner.child(
            div()
                .min_w_0()
                .child(selection_context.text_node(&text, range)),
        );
    }
    if !date.is_empty() {
        if has_text {
            inner = inner.child(div().child("•"));
        }
        if let Some(range) = selection_cursor.section(&date) {
            inner = inner.child(div().child(selection_context.text_node(&date, range)));
        }
    }
    row.child(inner).into_any_element()
}
