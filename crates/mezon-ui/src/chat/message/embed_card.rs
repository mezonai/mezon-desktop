use gpui::{
    AnyElement, App, FontWeight, ObjectFit, SharedString, Window, div, img, prelude::*, px,
};
use mezon_store::{
    AttachmentSeedInput, ChannelId, ChannelList, ClanId, ClanList, Embed, EmbedAuthor, EmbedImage,
    Message, MessageCode, MessageId, UserId,
};

use super::content::{
    SelectableSectionCursor, SelectableTextContext, open_message_link,
    render_selectable_embed_description, selectable_spans_text,
};
use super::context::RowCtx;
use super::embed_fields::render_embed_fields;
use super::parts::{open_viewer_from_message, viewer_uploader_id};
use super::share_contact_card::render_share_contact_card;
use crate::channel_app::launch_channel_app_from_store;
use crate::router::Route;

const CARD_MAX_WIDTH: f32 = 520.0;
const CARD_RADIUS: f32 = 8.0;
const ACCENT_BAR_WIDTH: f32 = 4.0;
const AUTHOR_ICON_SIZE: f32 = 24.0;
const THUMBNAIL_SIZE: f32 = 64.0;
const THUMBNAIL_OFFSET: f32 = 16.0;
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
            render_embed_card(embed, msg, index, base, selection_context, ctx)
        });
    }
    column.into_any_element()
}

fn embed_card_width(ctx: &RowCtx) -> f32 {
    if ctx.content_width > 0. {
        ctx.content_width.min(CARD_MAX_WIDTH)
    } else {
        CARD_MAX_WIDTH
    }
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
    embed_index: usize,
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
        .cursor(gpui::CursorStyle::IBeam);
    if let Some(author) = embed.author.as_ref() {
        let child = selection_cursor
            .section(&author.name)
            .map(|range| selection_context.text_node(&author.name, range));
        left = left.child(render_embed_author(author, child, msg.id, embed_index, ctx));
    }
    if let Some(range) = selection_cursor.section(&embed.title) {
        left = left.child(render_embed_title(
            selection_context.text_node(&embed.title, range),
            embed.url.clone(),
            msg.id,
            embed_index,
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
            msg,
            embed_index,
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
                .child(left.flex_auto())
                .child(render_embed_thumbnail(embed, msg, embed_index, ctx)),
        );
    } else {
        inner = inner.child(left.w_full());
    }
    if let Some(image) = embed.image.as_ref() {
        inner = inner.child(render_embed_image(image, msg, embed_index, ctx));
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
        .w(px(embed_card_width(ctx)))
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
                    .top_0()
                    .bottom_0()
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
    message_id: MessageId,
    embed_index: usize,
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
    let text = name.unwrap_or_else(|| gpui::StyledText::new(author.name.clone()));
    let label = div()
        .id(SharedString::from(format!(
            "embed-author-link-{}-{embed_index}",
            message_id.get()
        )))
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ctx.theme.tokens.text_theme_message)
        .child(text);
    let label = match sanitize_href(author.url.as_deref()) {
        Some(url) => label
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(move |_, _, cx| open_message_link(external_href(&url), cx))
            .into_any_element(),
        None => label.into_any_element(),
    };
    row.child(label).into_any_element()
}

fn render_embed_title(
    title: gpui::StyledText,
    url: Option<SharedString>,
    message_id: MessageId,
    embed_index: usize,
    ctx: &RowCtx,
) -> AnyElement {
    let element = div()
        .id(SharedString::from(format!(
            "embed-title-link-{}-{embed_index}",
            message_id.get()
        )))
        .mt_2()
        .min_w_0()
        .w_full()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ctx.theme.tokens.text_theme_message)
        .child(title);
    match sanitize_href(url.as_deref()) {
        Some(url) => element
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(move |_, _, cx| open_embed_url(&url, cx))
            .into_any_element(),
        None => element.into_any_element(),
    }
}

fn sanitize_href(url: Option<&str>) -> Option<String> {
    let url = url?.trim();
    if url.is_empty() || url.starts_with(['/', '#', '?']) {
        return None;
    }
    let Some((scheme, _)) = url.split_once(':') else {
        return Some(url.to_string());
    };
    let scheme = scheme.to_ascii_lowercase();
    let is_scheme = scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'));
    if !is_scheme {
        return Some(url.to_string());
    }
    matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel").then(|| url.to_string())
}

fn external_href(url: &str) -> String {
    if url.contains("://") || url.starts_with("mailto:") || url.starts_with("tel:") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

fn open_embed_url(url: &str, cx: &mut App) {
    let Some((clan_id, channel_id)) = extract_channel_app_target(url) else {
        open_message_link(external_href(url), cx);
        return;
    };
    crate::router::navigate(
        cx,
        Route::Channel {
            clan_id,
            channel_id,
        },
    );
    let channel_list = ChannelList::global(cx);
    let app = channel_list
        .read(cx)
        .app_channel_for_id(clan_id, channel_id)
        .cloned();
    let Some(app) = app else {
        return;
    };
    let Ok(app_id) = app.app_id.parse::<i64>() else {
        return;
    };
    let clan_name = ClanList::try_global(cx)
        .and_then(|store| {
            store
                .read(cx)
                .clan_by_id(clan_id)
                .map(|clan| clan.name.clone())
        })
        .unwrap_or_default();
    launch_channel_app_from_store(app_id, app.app_url, clan_id, clan_name, channel_list, cx);
}

fn extract_channel_app_target(url: &str) -> Option<(ClanId, ChannelId)> {
    let url = url.to_ascii_lowercase();
    let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_prefix("mezon.ai/")?;
    let rest = rest.strip_prefix("channel-app/")?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let mut parts = rest.split('/');
    let channel_id = parts.next()?.parse::<i64>().ok()?;
    let clan_id = parts.next()?.parse::<i64>().ok()?;
    if channel_id == 0 || clan_id == 0 {
        return None;
    }
    Some((ClanId(clan_id), ChannelId(channel_id)))
}

fn open_embed_media(
    url: SharedString,
    size: (u32, u32),
    msg: &Message,
    ctx: &RowCtx,
) -> impl Fn(&mut Window, &mut App) + use<> {
    let settings = ctx.settings.clone();
    let message_id = msg.id;
    let create_time = msg.create_time;
    let uploader_id: UserId = viewer_uploader_id(msg);
    move |window, cx| {
        let filename = url
            .rsplit('/')
            .next()
            .and_then(|name| name.split(['?', '#']).next())
            .unwrap_or("image")
            .to_string();
        open_viewer_from_message(
            &settings,
            AttachmentSeedInput {
                url: url.to_string(),
                filename,
                filetype: "image".to_string(),
                width: size.0,
                height: size.1,
                presign_pending: false,
            },
            message_id,
            create_time,
            uploader_id,
            window,
            cx,
        );
    }
}

fn render_embed_thumbnail(
    embed: &Embed,
    msg: &Message,
    embed_index: usize,
    ctx: &RowCtx,
) -> AnyElement {
    let open = open_embed_media(embed.thumbnail_url.clone(), (0, 0), msg, ctx);
    div()
        .image_cache(ctx.avatar_cache.clone())
        .id(SharedString::from(format!(
            "embed-thumbnail-{}-{embed_index}",
            msg.id.get()
        )))
        .flex_shrink_0()
        .relative()
        .top(px(THUMBNAIL_OFFSET))
        .size(px(THUMBNAIL_SIZE))
        .rounded(px(4.))
        .overflow_hidden()
        .cursor_pointer()
        .child(
            img(embed.thumbnail_proxied.clone())
                .size(px(THUMBNAIL_SIZE))
                .object_fit(ObjectFit::Cover),
        )
        .on_click(move |_, window, cx| open(window, cx))
        .into_any_element()
}

fn render_embed_image(
    image: &EmbedImage,
    msg: &Message,
    embed_index: usize,
    ctx: &RowCtx,
) -> AnyElement {
    let has_aspect = matches!((image.width, image.height), (Some(w), Some(h)) if w > 0 && h > 0);
    let open = open_embed_media(
        image.url.clone(),
        (image.width.unwrap_or(0), image.height.unwrap_or(0)),
        msg,
        ctx,
    );
    let mut container = div()
        .id(SharedString::from(format!(
            "embed-image-{}-{embed_index}",
            msg.id.get()
        )))
        .mt_2()
        .w_full()
        .max_h(px(EMBED_IMAGE_MAX_HEIGHT))
        .rounded(px(4.))
        .overflow_hidden()
        .cursor_pointer()
        .on_click(move |_, window, cx| open(window, cx));
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
        .items_center()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_href_matches_the_react_allow_list() {
        assert_eq!(
            sanitize_href(Some(" https://mezon.ai/x ")).as_deref(),
            Some("https://mezon.ai/x")
        );
        assert_eq!(
            sanitize_href(Some("mailto:a@b.c")).as_deref(),
            Some("mailto:a@b.c")
        );
        assert_eq!(
            sanitize_href(Some("mezon.ai/channel-app/1/2")).as_deref(),
            Some("mezon.ai/channel-app/1/2"),
            "a scheme-less url stays a link, as in React"
        );
        assert!(sanitize_href(Some("javascript:alert(1)")).is_none());
        assert!(sanitize_href(Some("DATA:text/html,x")).is_none());
        assert!(sanitize_href(Some("file:///etc/passwd")).is_none());
        assert!(sanitize_href(Some("ftp://host/f")).is_none());
        assert!(sanitize_href(Some("/relative")).is_none());
        assert!(sanitize_href(Some("   ")).is_none());
        assert!(sanitize_href(None).is_none());
    }

    #[test]
    fn external_href_adds_a_scheme_only_when_missing() {
        assert_eq!(external_href("mezon.ai/x"), "https://mezon.ai/x");
        assert_eq!(external_href("http://mezon.ai"), "http://mezon.ai");
        assert_eq!(external_href("mailto:a@b.c"), "mailto:a@b.c");
    }

    #[test]
    fn channel_app_target_is_extracted_with_or_without_a_scheme() {
        assert_eq!(
            extract_channel_app_target("https://mezon.ai/channel-app/12/34?code=x&subpath=y"),
            Some((ClanId(34), ChannelId(12)))
        );
        assert_eq!(
            extract_channel_app_target("mezon.ai/channel-app/12/34"),
            Some((ClanId(34), ChannelId(12)))
        );
        assert_eq!(
            extract_channel_app_target("https://www.mezon.ai/channel-app/12/34"),
            Some((ClanId(34), ChannelId(12)))
        );
        assert!(extract_channel_app_target("https://mezon.ai/invite/abc").is_none());
        assert!(extract_channel_app_target("https://other.ai/channel-app/1/2").is_none());
        assert!(extract_channel_app_target("https://mezon.ai/channel-app/12").is_none());
    }
}
