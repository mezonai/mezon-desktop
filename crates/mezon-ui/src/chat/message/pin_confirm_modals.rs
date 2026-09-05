use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use mezon_store::{
    AccountStore, ChannelId, ClanId, ClanList, ClanMembersStore, DirectMessageStore, Message,
    MessageId, MessagesStore, PinnedMessage, PinnedMessagesStore, UsersByUserStore,
};

use super::parts::{
    effective_clan_id, resolve_pin_avatar_url, resolve_pin_sender_label_with_message,
};
use super::time::format_message_time;
use crate::app::shell::Shell;
use crate::chat::pinned_popover::{render_pin_message_preview, render_pinned_message_preview};
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Sizable, Size, h_flex, v_flex,
};
use crate::image_cache::{LruImageCache, PREVIEW_ENTRY_MAX_BYTES};

const MODAL_PREVIEW_IMAGE_CACHE_CAPACITY: usize = 8;
const MODAL_PREVIEW_IMAGE_CACHE_BYTES: u64 = 8 * 1024 * 1024;
use crate::image_viewer::resolve_channel_label;
use crate::theme::ActiveTheme;

struct MessagePreview {
    sender_label: SharedString,
    avatar_src: Option<SharedString>,
    avatar_fallback: Option<SharedString>,
    body: gpui::AnyElement,
    timestamp: Option<SharedString>,
}

pub struct ConfirmPinMessageModal {
    focus_handle: FocusHandle,
    message_id: MessageId,
    message: Message,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
    locale: SharedString,
    title: SharedString,
    description: SharedString,
    cancel_label: SharedString,
    confirm_label: SharedString,
    avatar_image_cache: Entity<LruImageCache>,
    message_image_cache: Entity<LruImageCache>,
    ogp_image_cache: Entity<LruImageCache>,
    _subs: Vec<Subscription>,
}

pub struct ConfirmUnpinMessageModal {
    focus_handle: FocusHandle,
    pin_id: SharedString,
    message_id: SharedString,
    fallback_sender_label: SharedString,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
    locale: SharedString,
    title: SharedString,
    description: SharedString,
    cancel_label: SharedString,
    confirm_label: SharedString,
    avatar_image_cache: Entity<LruImageCache>,
    message_image_cache: Entity<LruImageCache>,
    ogp_image_cache: Entity<LruImageCache>,
    _subs: Vec<Subscription>,
}

impl Focusable for ConfirmPinMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Focusable for ConfirmUnpinMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn modal_preview_image_cache(cx: &mut App, label: &'static str) -> Entity<LruImageCache> {
    cx.new(|cx| {
        LruImageCache::message(
            label,
            MODAL_PREVIEW_IMAGE_CACHE_CAPACITY,
            MODAL_PREVIEW_IMAGE_CACHE_BYTES,
            PREVIEW_ENTRY_MAX_BYTES,
            cx,
        )
    })
}

fn member_subscriptions(cx: &mut Context<ConfirmPinMessageModal>) -> Vec<Subscription> {
    vec![
        cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&UsersByUserStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&AccountStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&DirectMessageStore::global(cx), |_, _, cx| cx.notify()),
    ]
}

fn member_subscriptions_unpin(cx: &mut Context<ConfirmUnpinMessageModal>) -> Vec<Subscription> {
    vec![
        cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&UsersByUserStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&AccountStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&DirectMessageStore::global(cx), |_, _, cx| cx.notify()),
        cx.observe(&PinnedMessagesStore::global(cx), |_, _, cx| cx.notify()),
    ]
}

impl ConfirmPinMessageModal {
    pub fn open(message_id: MessageId, locale: SharedString, window: &mut Window, cx: &mut App) {
        if PinnedMessagesStore::global(cx)
            .read(cx)
            .is_pinned(&message_id.to_string())
        {
            return;
        }

        let (is_dm, mut clan_id, channel_id) = {
            let messages = MessagesStore::global(cx).read(cx);
            (
                messages.is_dm(),
                messages.active_clan_id(),
                messages.active_channel_id(),
            )
        };
        let message = match find_channel_message(message_id, channel_id, cx) {
            Some(message) => message,
            None => {
                tracing::warn!(
                    message_id = message_id.get(),
                    "confirm pin modal: message not found in active or channel cache"
                );
                Message::new(message_id, String::new(), "", "", 0)
            }
        };
        if clan_id.is_none() {
            clan_id = ClanList::global(cx).read(cx).active_clan_id;
        }

        if let Some(clan_id) = clan_id {
            ClanMembersStore::global(cx).update(cx, |members, cx| {
                members.ensure_loaded(clan_id, cx);
            });
        }

        let description = if is_dm {
            mezon_i18n::t(&locale, "pinMessage.modal.descriptionDM").to_string()
        } else {
            let channel_label = match (clan_id, channel_id) {
                (Some(clan_id), Some(channel_id)) => {
                    resolve_channel_label(clan_id, channel_id, SharedString::default(), cx)
                }
                _ => SharedString::default(),
            };
            mezon_i18n::t(&locale, "pinMessage.modal.description")
                .replace("{{channelLabel}}", channel_label.as_ref())
        };

        let locale_for_view = locale.clone();
        let view = cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            message_id,
            message,
            clan_id,
            channel_id,
            locale: locale_for_view,
            title: mezon_i18n::t(&locale, "pinMessage.modal.title")
                .to_string()
                .into(),
            description: description.into(),
            cancel_label: mezon_i18n::t(&locale, "pinMessage.modal.cancel")
                .to_string()
                .into(),
            confirm_label: mezon_i18n::t(&locale, "pinMessage.modal.confirm")
                .to_string()
                .into(),
            avatar_image_cache: crate::image_cache::shared_avatar_cache(cx),
            message_image_cache: modal_preview_image_cache(cx, "pin-confirm-image"),
            ogp_image_cache: crate::image_cache::ogp_aux_cache("pin-confirm-ogp", cx),
            _subs: member_subscriptions(cx),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn confirm(&self, cx: &mut App) {
        let message_id = self.message_id.to_string();
        PinnedMessagesStore::global(cx).update(cx, |store, cx| store.pin(&message_id, cx));
        Self::close(cx);
    }

    fn preview(&self, cx: &App) -> MessagePreview {
        preview_from_message(
            &self.message,
            self.clan_id,
            self.channel_id,
            &self.locale,
            self.message_image_cache.clone(),
            self.ogp_image_cache.clone(),
            cx,
        )
    }
}

impl ConfirmUnpinMessageModal {
    pub fn open(
        pin_id: SharedString,
        message_id: SharedString,
        sender_label: SharedString,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (clan_id, channel_id) = {
            let store = PinnedMessagesStore::global(cx).read(cx);
            (store.clan_id(), store.channel_id())
        };
        if let Some(clan_id) = clan_id {
            ClanMembersStore::global(cx).update(cx, |members, cx| {
                members.ensure_loaded(clan_id, cx);
            });
        }

        let view = cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            pin_id,
            message_id,
            fallback_sender_label: sender_label,
            clan_id,
            channel_id,
            locale: locale.clone(),
            title: mezon_i18n::t(&locale, "channelTopbar.pinnedMessages.unpinMessage")
                .to_string()
                .into(),
            description: mezon_i18n::t(&locale, "channelTopbar.pinnedMessages.unpinConfirmation")
                .to_string()
                .into(),
            cancel_label: mezon_i18n::t(&locale, "channelTopbar.pinnedMessages.cancel")
                .to_string()
                .into(),
            confirm_label: mezon_i18n::t(&locale, "channelTopbar.pinnedMessages.unpinIt")
                .to_string()
                .into(),
            avatar_image_cache: crate::image_cache::shared_avatar_cache(cx),
            message_image_cache: modal_preview_image_cache(cx, "unpin-confirm-image"),
            ogp_image_cache: crate::image_cache::ogp_aux_cache("unpin-confirm-ogp", cx),
            _subs: member_subscriptions_unpin(cx),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn confirm(&self, cx: &mut App) {
        let pin_id = self.pin_id.to_string();
        let message_id = self.message_id.to_string();
        PinnedMessagesStore::global(cx).update(cx, |store, cx| {
            store.unpin(&pin_id, &message_id, cx);
        });
        Self::close(cx);
    }

    fn preview(&self, cx: &App) -> Option<MessagePreview> {
        let pin = PinnedMessagesStore::global(cx)
            .read(cx)
            .pinned()
            .iter()
            .find(|p| p.id == self.pin_id.as_ref())
            .cloned()?;
        Some(preview_from_pin(
            &pin,
            self.clan_id,
            self.channel_id,
            &self.fallback_sender_label,
            &self.locale,
            self.message_image_cache.clone(),
            self.ogp_image_cache.clone(),
            cx,
        ))
    }
}

fn find_channel_message(
    message_id: MessageId,
    channel_id: Option<ChannelId>,
    cx: &App,
) -> Option<Message> {
    let store = MessagesStore::global(cx).read(cx);
    if let Some(msg) = store.messages().iter().find(|m| m.id == message_id) {
        return Some(msg.clone());
    }
    channel_id.and_then(|channel_id| store.message_in_channel(channel_id, message_id).cloned())
}

fn preview_from_message(
    msg: &Message,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
    locale: &str,
    image_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
    cx: &App,
) -> MessagePreview {
    let theme = cx.theme();
    let clan_id = effective_clan_id(clan_id, cx);
    let sender_label = resolve_pin_sender_label_with_message(
        &msg.sender_id,
        msg.sender_name.as_ref(),
        Some(&msg.id.to_string()),
        clan_id,
        channel_id,
        cx,
    );

    let mut avatar_raw = resolve_pin_avatar_url(
        &msg.sender_id,
        msg.avatar_url.as_ref(),
        clan_id,
        channel_id,
        cx,
    );
    if avatar_raw.is_empty() && !msg.avatar_proxied.is_empty() {
        avatar_raw = msg.avatar_proxied.to_string();
    }

    let (avatar_src, avatar_fallback) = resolve_avatar_urls(&avatar_raw, &msg.avatar_proxied, cx);

    let timestamp = Some(format_message_time(
        &msg.time_hhmm,
        msg.local_date,
        locale,
        chrono::Local::now(),
    ));

    MessagePreview {
        sender_label,
        avatar_src,
        avatar_fallback,
        body: render_pin_message_preview(msg, theme, locale, image_cache, ogp_cache),
        timestamp,
    }
}

fn preview_from_pin(
    pin: &PinnedMessage,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
    fallback_sender_label: &SharedString,
    locale: &str,
    image_cache: Entity<LruImageCache>,
    ogp_cache: Entity<LruImageCache>,
    cx: &App,
) -> MessagePreview {
    let theme = cx.theme();
    let clan_id = effective_clan_id(clan_id, cx);
    let mut sender_label = resolve_pin_sender_label_with_message(
        &pin.sender_id,
        &pin.sender_name,
        Some(pin.message_id.as_str()),
        clan_id,
        channel_id,
        cx,
    );
    if sender_label.is_empty() {
        sender_label = fallback_sender_label.clone();
    }

    let avatar_raw = {
        let from_member =
            resolve_pin_avatar_url(&pin.sender_id, &pin.avatar_url, clan_id, channel_id, cx);
        if !from_member.is_empty() {
            from_member
        } else if let Ok(message_id) = pin.message_id.parse::<MessageId>()
            && let Some(msg) = find_channel_message(message_id, channel_id, cx)
            && !msg.avatar_url.is_empty()
        {
            msg.avatar_url.to_string()
        } else {
            pin.avatar_url.clone()
        }
    };

    let (avatar_src, avatar_fallback) = resolve_avatar_urls(&avatar_raw, &pin.avatar_proxied, cx);

    MessagePreview {
        sender_label,
        avatar_src,
        avatar_fallback,
        body: render_pinned_message_preview(pin, theme, locale, image_cache, ogp_cache),
        timestamp: None,
    }
}

fn resolve_avatar_urls(
    avatar_raw: &str,
    avatar_proxied: &SharedString,
    cx: &App,
) -> (Option<SharedString>, Option<SharedString>) {
    if !avatar_raw.is_empty() {
        let proxied = crate::util::imgproxy::avatar_url(cx, avatar_raw);
        (Some(proxied.into()), Some(avatar_raw.into()))
    } else if !avatar_proxied.is_empty() {
        (Some(avatar_proxied.clone()), None)
    } else {
        (None, None)
    }
}

fn render_preview(
    preview: MessagePreview,
    avatar_cache: Entity<LruImageCache>,
    theme: &crate::theme::Theme,
) -> impl IntoElement {
    let tokens = &theme.tokens;
    let sender_color = tokens.text_theme_primary;

    let mut avatar = Avatar::new()
        .name(&preview.sender_label)
        .with_size(Size::Small)
        .image_cache(avatar_cache);
    if let Some(src) = &preview.avatar_src {
        avatar = avatar.src(src.clone());
    }
    if let Some(fallback) = &preview.avatar_fallback {
        avatar = avatar.fallback_src(fallback.clone());
    }

    let name_row = h_flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(sender_color)
                .overflow_hidden()
                .text_ellipsis()
                .child(preview.sender_label.clone()),
        )
        .when_some(preview.timestamp.clone(), |row, ts| {
            row.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tokens.text_secondary)
                    .child(ts),
            )
        });

    h_flex()
        .w_full()
        .items_start()
        .gap_2()
        .p(px(8.))
        .rounded(px(4.))
        .bg(tokens.bg_item_theme_hover)
        .child(avatar)
        .child(
            v_flex().flex_1().min_w_0().gap_1().child(name_row).child(
                div()
                    .id("pin-confirm-preview")
                    .w_full()
                    .min_w_0()
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .child(preview.body),
            ),
        )
}

fn modal_shell(
    focus_handle: &FocusHandle,
    close: fn(&mut App),
    shell_bg: gpui::Rgba,
    body: impl IntoElement,
    footer: impl IntoElement,
) -> impl IntoElement {
    div()
        .rounded(px(12.))
        .overflow_hidden()
        .bg(shell_bg)
        .shadow_lg()
        .child(
            v_flex()
                .track_focus(focus_handle)
                .key_context("menu")
                .on_action(move |_: &::menu::Cancel, _window, cx| {
                    close(cx);
                })
                .occlude()
                .w(px(440.))
                .max_w(px(440.))
                .child(body)
                .child(footer),
        )
}

impl Render for ConfirmPinMessageModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tokens = &theme.tokens;
        let preview = self.preview(cx);

        let body = v_flex()
            .w_full()
            .rounded_t(px(12.))
            .bg(tokens.theme_setting_primary)
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(0.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tokens.text_theme_message)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(tokens.text_theme_primary)
                            .child(self.description.clone()),
                    ),
            )
            .child(div().px(px(16.)).py(px(16.)).child(render_preview(
                preview,
                self.avatar_image_cache.clone(),
                &theme,
            )));

        let footer = h_flex()
            .w_full()
            .justify_end()
            .gap_4()
            .p(px(16.))
            .rounded_b(px(12.))
            .bg(tokens.theme_setting_nav)
            .child(
                Button::new("pin-confirm-cancel")
                    .label(self.cancel_label.clone())
                    .ghost()
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
            )
            .child(
                Button::new("pin-confirm-ok")
                    .label(self.confirm_label.clone())
                    .primary()
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.confirm(cx);
                    })),
            );

        modal_shell(
            &self.focus_handle,
            Self::close,
            tokens.theme_setting_primary,
            body,
            footer,
        )
    }
}

impl Render for ConfirmUnpinMessageModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tokens = &theme.tokens;
        let preview = self.preview(cx);

        let body = v_flex()
            .w_full()
            .rounded_t(px(12.))
            .bg(tokens.theme_setting_primary)
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(0.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tokens.text_theme_message)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(tokens.text_theme_primary)
                            .child(self.description.clone()),
                    ),
            )
            .children(preview.map(|preview| {
                div()
                    .px(px(16.))
                    .pb(px(8.))
                    .pt(px(12.))
                    .child(render_preview(
                        preview,
                        self.avatar_image_cache.clone(),
                        &theme,
                    ))
                    .into_any_element()
            }));

        let footer = h_flex()
            .w_full()
            .justify_end()
            .gap_4()
            .p(px(16.))
            .rounded_b(px(12.))
            .bg(tokens.theme_setting_nav)
            .child(
                Button::new("unpin-confirm-cancel")
                    .label(self.cancel_label.clone())
                    .ghost()
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
            )
            .child(
                Button::new("unpin-confirm-ok")
                    .label(self.confirm_label.clone())
                    .danger()
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.confirm(cx);
                    })),
            );

        modal_shell(
            &self.focus_handle,
            Self::close,
            tokens.theme_setting_primary,
            body,
            footer,
        )
    }
}
