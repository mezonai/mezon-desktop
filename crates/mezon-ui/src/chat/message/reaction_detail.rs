use gpui::{
    AnyElement, App, Context, Entity, SharedString, Subscription, Window, div, img, prelude::*, px,
};
use mezon_store::{
    AccountStore, BadgeService, ChannelId, ClanId, ClanList, ClanMembersStore, DirectKind,
    DirectMessageStore, GroupMembersStore, MessageId, MessagesEvent, MessagesStore, UserId,
};

use crate::components::primitives::{Avatar, Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;

pub struct UserReactionPanel {
    message_id: MessageId,
    emoji_id: SharedString,
    emoji: SharedString,
    image_cache: Entity<LruImageCache>,
    _messages_sub: Subscription,
}

impl UserReactionPanel {
    pub fn new(
        message_id: MessageId,
        emoji_id: SharedString,
        emoji: SharedString,
        image_cache: Entity<LruImageCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = MessagesStore::global(cx);
        let messages_sub = cx.subscribe(&store, |this, _store, event: &MessagesEvent, cx| {
            if let MessagesEvent::Updated { message_id } = event
                && (message_id.is_none() || *message_id == Some(this.message_id))
            {
                cx.notify();
            }
        });
        Self {
            message_id,
            emoji_id,
            emoji,
            image_cache,
            _messages_sub: messages_sub,
        }
    }
}

impl Render for UserReactionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (count, senders) = MessagesStore::global(cx)
            .read(cx)
            .reaction_view(self.message_id, &self.emoji_id, &self.emoji)
            .unwrap_or((0, Vec::new()));
        let emoji_src = crate::util::imgproxy::emoji_url(cx, &self.emoji_id);
        let current_uid = BadgeService::global(cx).read(cx).current_user_id(cx);
        let clan_id = ClanList::global(cx).read(cx).active_clan_id;
        let dm_context = clan_id
            .is_none()
            .then(|| {
                let direct = DirectMessageStore::global(cx);
                let direct = direct.read(cx);
                direct
                    .current()
                    .and_then(|(cid, _)| direct.find(cid))
                    .map(|dm| DmContext {
                        id: dm.id,
                        kind: dm.kind,
                        peer: dm.peer_user_id,
                        label: dm.label.clone(),
                        avatar: dm.avatar.clone(),
                    })
            })
            .flatten();
        let self_profile = clan_id
            .is_none()
            .then(|| {
                AccountStore::try_global(cx).and_then(|store| {
                    store.read(cx).account.as_ref().map(|account| {
                        let name = if account.display_name.is_empty() {
                            account.username.clone()
                        } else {
                            account.display_name.clone()
                        };
                        (name, account.avatar_url.clone().unwrap_or_default())
                    })
                })
            })
            .flatten();

        let emoji_glyph = self.emoji.clone();
        let header_emoji = if emoji_src.is_empty() {
            div()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .child(emoji_glyph.clone())
                .into_any_element()
        } else {
            img(SharedString::from(emoji_src))
                .size(px(20.))
                .with_fallback(emoji_error_fallback(px(20.), theme.text_muted))
                .into_any_element()
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(theme.border)
            .child(header_emoji)
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(count.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .truncate()
                    .max_w(px(200.))
                    .text_color(theme.text_secondary)
                    .child(self.emoji.clone()),
            );

        let mut list = div()
            .id("reaction-users")
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .max_h(px(160.))
            .overflow_y_scroll();
        for (index, (sender_id, sender_count)) in senders.iter().enumerate() {
            let uid = sender_id.parse::<i64>().ok().map(UserId::new);
            let (name, avatar) = resolve_reactor(
                uid,
                clan_id,
                current_uid,
                dm_context.as_ref(),
                self_profile.as_ref(),
                cx,
            );
            let display_name = if name.is_empty() {
                SharedString::from(sender_id.clone())
            } else {
                SharedString::from(name)
            };
            let avatar_url = if avatar.is_empty() {
                String::new()
            } else {
                crate::util::imgproxy::avatar_url(cx, &avatar)
            };
            let mut avatar_el = Avatar::new().size_px(px(32.)).name(display_name.clone());
            if !avatar_url.is_empty() {
                avatar_el = avatar_el.src(avatar_url);
            }

            let is_own = current_uid.is_some_and(|u| u.get().to_string() == *sender_id);
            let remove = if is_own {
                let message_id = self.message_id;
                let emoji_id = self.emoji_id.clone();
                let emoji = self.emoji.clone();
                Some(
                    div()
                        .id(("reaction-remove", index))
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(theme.text_secondary),
                        )
                        .on_click(move |_, _, cx| {
                            MessagesStore::global(cx).update(cx, |store, cx| {
                                store.remove_reaction(
                                    message_id,
                                    emoji_id.to_string(),
                                    emoji.to_string(),
                                    cx,
                                );
                            });
                        }),
                )
            } else {
                None
            };

            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_1()
                .child(avatar_el)
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .truncate()
                        .max_w(px(150.))
                        .text_color(theme.tokens.text_theme_primary)
                        .child(display_name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(sender_count.to_string()),
                )
                .children(remove);
            list = list.child(row);
        }

        div()
            .occlude()
            .image_cache(self.image_cache.clone())
            .w(px(288.))
            .max_h(px(400.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .p_1()
            .child(header)
            .child(list)
    }
}

struct DmContext {
    id: ChannelId,
    kind: DirectKind,
    peer: Option<UserId>,
    label: String,
    avatar: String,
}

fn resolve_reactor(
    uid: Option<UserId>,
    clan_id: Option<ClanId>,
    current_uid: Option<UserId>,
    dm: Option<&DmContext>,
    self_profile: Option<&(String, String)>,
    cx: &App,
) -> (String, String) {
    let Some(u) = uid else {
        return (String::new(), String::new());
    };
    if let Some(cid) = clan_id {
        return ClanMembersStore::global(cx)
            .read(cx)
            .member(cid, u)
            .map(|m| (m.name().to_string(), m.avatar().to_string()))
            .unwrap_or_default();
    }
    if current_uid == Some(u)
        && let Some((name, avatar)) = self_profile
    {
        return (name.clone(), avatar.clone());
    }
    if let Some(dm) = dm {
        if dm.kind == DirectKind::Group {
            if let Some(m) = GroupMembersStore::global(cx).read(cx).member(dm.id, u) {
                return (m.name().to_string(), m.avatar().to_string());
            }
        } else if dm.peer == Some(u) {
            return (dm.label.clone(), dm.avatar.clone());
        }
    }
    (String::new(), String::new())
}

pub fn emoji_error_fallback(
    size: gpui::Pixels,
    color: impl Into<gpui::Hsla>,
) -> impl Fn() -> AnyElement + 'static {
    let color = color.into();
    move || {
        div()
            .size(size)
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(IconName::ImageThumbnail)
                    .size(size)
                    .text_color(color),
            )
            .into_any_element()
    }
}
