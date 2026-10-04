use gpui::{AnyElement, Entity, ObjectFit, SharedString, div, hsla, img, prelude::*, px, rgb};
use mezon_store::{ChannelId, Message, MessageId, MessagesStore, OgpPreview};

use crate::image_cache::LruImageCache;

use super::content::SelectableSectionCursor;
use super::context::RowCtx;
use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

const OGP_TITLE_COLOR: u32 = 0x3b82f6;
const OGP_TITLE_HOVER_COLOR: u32 = 0x60a5fa;

pub fn render_ogp_embed(
    msg: &Message,
    ctx: &RowCtx,
    base: usize,
    selection_context: &super::content::SelectableTextContext,
) -> Option<AnyElement> {
    render_ogp_preview_impl(
        msg.ogp.as_deref()?,
        msg.row_anchor_id,
        ctx.theme,
        ogp_remove_target(msg, ctx.current_user_id),
        ctx.ogp_cache.clone(),
        Some((base, selection_context, ctx.selection.clone())),
    )
}

fn ogp_remove_target(msg: &Message, current_user_id: &str) -> Option<(ChannelId, MessageId)> {
    (super::message_context_menu::message_is_editable(msg, current_user_id)
        && !msg.id.is_optimistic()
        && msg.raw_content.is_some()
        && msg.channel_id != ChannelId(0))
    .then_some((msg.channel_id, msg.id))
}

pub fn render_ogp_preview(
    ogp: &OgpPreview,
    message_id: MessageId,
    theme: &Theme,
    ogp_cache: Entity<LruImageCache>,
) -> Option<AnyElement> {
    render_ogp_preview_impl(ogp, message_id, theme, None, ogp_cache, None)
}

fn render_ogp_preview_impl(
    ogp: &OgpPreview,
    message_id: MessageId,
    theme: &Theme,
    remove_target: Option<(ChannelId, MessageId)>,
    og_cache: Entity<LruImageCache>,
    selectable: Option<(
        usize,
        &super::content::SelectableTextContext,
        super::selection::SharedSelection,
    )>,
) -> Option<AnyElement> {
    let url = ogp.url.clone();
    let has_text = !ogp.title.is_empty() || !ogp.description_collapsed.is_empty();
    let base = selectable.as_ref().map_or(0, |value| value.0);
    let selection_context = selectable.as_ref().map(|value| value.1);
    let click_selection = selectable
        .as_ref()
        .map(|(_, _, selection)| selection.clone());

    let text_block = has_text.then(|| {
        let mut cursor = SelectableSectionCursor::new(base);
        let mut block = div().flex().flex_col().gap_0p5();
        if !ogp.title.is_empty() {
            let range = cursor.section(&ogp.title);
            block = block.child(
                div()
                    .id("ogp-title")
                    .cursor_pointer()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(OGP_TITLE_COLOR))
                    .line_clamp(2)
                    .hover(|s| s.text_color(rgb(OGP_TITLE_HOVER_COLOR)))
                    .child(
                        if let (Some(context), Some(range)) = (selection_context, range) {
                            context.text_node(&ogp.title, range).into_any_element()
                        } else {
                            ogp.title.clone().into_any_element()
                        },
                    ),
            );
        }
        if !ogp.description_collapsed.is_empty() {
            let description = ogp.description_collapsed.as_ref();
            let range = cursor.section(description);
            block = block.child(
                div()
                    .cursor(gpui::CursorStyle::IBeam)
                    .text_size(px(12.))
                    .text_color(theme.tokens.text_theme_primary)
                    .opacity(0.9)
                    .line_clamp(2)
                    .child(
                        if let (Some(context), Some(range)) = (selection_context, range) {
                            context.text_node(description, range).into_any_element()
                        } else {
                            ogp.description_collapsed.clone().into_any_element()
                        },
                    ),
            );
        }
        block
    });

    let image_box = div()
        .w_full()
        .h(px(200.))
        .flex_shrink_0()
        .mt_1()
        .rounded(px(4.))
        .overflow_hidden()
        .border_1()
        .border_color(hsla(0., 0., 1., 0.05))
        .bg(theme.tokens.theme_setting_primary)
        .flex()
        .items_center()
        .justify_center()
        .image_cache(og_cache)
        .child(ogp_image(ogp.image_proxied.clone(), theme.text_muted));

    Some(
        div()
            .id(SharedString::from(format!("msg-ogp-{}", message_id.0)))
            .relative()
            .w_full()
            .max_w(px(350.))
            .mt_1()
            .mb_1()
            .rounded(px(8.))
            .overflow_hidden()
            .border_l(px(3.))
            .border_color(theme.tokens.border_left_highlight)
            .bg(theme.tokens.theme_setting_nav)
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                if click_selection
                    .as_ref()
                    .is_some_and(|selection| selection.borrow().has_selection())
                {
                    return;
                }
                super::content::open_message_link(url.clone(), cx);
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .when_some(text_block, |d, block| d.child(block))
                    .child(image_box),
            )
            .when_some(remove_target, |card, (bucket, remove_id)| {
                card.child(ogp_remove_button(bucket, remove_id, theme))
            })
            .into_any_element(),
    )
}

fn ogp_remove_button(bucket: ChannelId, message_id: MessageId, theme: &Theme) -> AnyElement {
    div()
        .id("ogp-remove")
        .absolute()
        .top(px(4.))
        .right(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded(px(4.))
        .cursor_pointer()
        .occlude()
        .bg(theme.tokens.theme_setting_primary)
        .hover(|s| s.bg(theme.bg_hover))
        .child(
            Icon::new(IconName::Close)
                .size(px(12.))
                .text_color(theme.text_muted),
        )
        .on_click(move |_, _, cx| {
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.remove_message_ogp(bucket, message_id, cx);
            });
        })
        .into_any_element()
}

pub(crate) fn selectable_ogp_text(ogp: &OgpPreview) -> String {
    match (ogp.title.is_empty(), ogp.description_collapsed.is_empty()) {
        (false, false) => format!("{}\n{}", ogp.title, ogp.description_collapsed),
        (false, true) => ogp.title.to_string(),
        (true, false) => ogp.description_collapsed.to_string(),
        (true, true) => String::new(),
    }
}

fn ogp_image(src: SharedString, fallback_fg: gpui::Rgba) -> AnyElement {
    if src.is_empty() {
        return ogp_image_fallback(fallback_fg);
    }
    img(src)
        .w_full()
        .max_h(px(200.))
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || ogp_image_fallback(fallback_fg))
        .into_any_element()
}

fn ogp_image_fallback(fallback_fg: gpui::Rgba) -> AnyElement {
    div()
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.3)
        .child(
            Icon::new(IconName::ImageThumbnail)
                .size(px(32.))
                .text_color(fallback_fg),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{ChannelId, Message, MessageId, ogp_remove_target};
    use mezon_store::MessageCode;

    const RAW: &str = r#"{"t":"https://example.com","mk":[{"type":"lk_ogp","s":19,"e":20}]}"#;

    fn own_link(id: MessageId) -> Message {
        let mut msg =
            Message::new(id, "https://example.com", "42", "Me", 100).with_raw_content(RAW);
        msg.channel_id = ChannelId(7);
        msg
    }

    #[test]
    fn remove_targets_the_bucket_and_server_id_of_an_acked_row() {
        let mut acked = own_link(MessageId(99));
        acked.row_anchor_id = MessageId::next_optimistic();

        assert_eq!(
            ogp_remove_target(&acked, "42"),
            Some((ChannelId(7), MessageId(99)))
        );
    }

    #[test]
    fn no_remove_target_for_someone_else() {
        let msg = own_link(MessageId(99));
        assert_eq!(ogp_remove_target(&msg, "7"), None);

        let mut unknown_sender = own_link(MessageId(99));
        unknown_sender.sender_id = String::new();
        assert_eq!(ogp_remove_target(&unknown_sender, ""), None);
    }

    #[test]
    fn no_remove_target_until_the_server_copy_is_in() {
        let pending = own_link(MessageId::next_optimistic());
        assert_eq!(ogp_remove_target(&pending, "42"), None);

        let mut acked_only = own_link(MessageId(99));
        acked_only.raw_content = None;
        assert_eq!(ogp_remove_target(&acked_only, "42"), None);

        let mut unbucketed = own_link(MessageId(99));
        unbucketed.channel_id = ChannelId(0);
        assert_eq!(ogp_remove_target(&unbucketed, "42"), None);
    }

    #[test]
    fn no_remove_target_where_edit_is_refused() {
        let origin = own_link(MessageId(99)).with_code(MessageCode::Topic);
        assert_eq!(ogp_remove_target(&origin, "42"), None);

        let forwarded = own_link(MessageId(99)).with_forwarded(true);
        assert_eq!(ogp_remove_target(&forwarded, "42"), None);
    }
}
