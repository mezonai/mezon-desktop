use std::collections::HashSet;

use gpui::{AnyElement, App, ClickEvent, Entity, FontWeight, SharedString, div, img, prelude::*, px};
use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, ClanMembersStore, DirectKind, DirectMessageStore,
    User, UserId, UsersByUserStore,
};

use crate::theme::Theme;
use crate::SHOW_UNREAD_BADGE_COUNT;

pub(crate) const ROW_PX: f32 = 32.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteItemKind {
    Channel,
    Direct,
    Member,
}

#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub kind: PaletteItemKind,
    pub label: SharedString,
    pub subtext: SharedString,
    pub avatar: SharedString,
    pub unread_count: u32,
    pub last_sent_timestamp: i64,
    pub channel_id: Option<ChannelId>,
    pub user_id: Option<UserId>,
}

fn cmp_items(a: &PaletteItem, b: &PaletteItem) -> std::cmp::Ordering {
    b.last_sent_timestamp
        .cmp(&a.last_sent_timestamp)
        .then_with(|| a.label.cmp(&b.label))
}

pub fn ensure_palette_sources_loaded(cx: &mut App) {
    ChannelList::global(cx).update(cx, |store, cx| store.ensure_user_channels_loaded(cx));
    DirectMessageStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
    if let Some(store) = UsersByUserStore::try_global(cx) {
        store.update(cx, |store, cx| store.ensure_loaded(cx));
    }
}

pub fn build_palette_items(cx: &App) -> Vec<PaletteItem> {
    let mut items = Vec::new();
    let mut dm_user_ids = HashSet::new();

    let dm_store = DirectMessageStore::global(cx);
    for dm in dm_store.read(cx).channels() {
        if dm.kind == DirectKind::Dm
            && let Some(user_id) = dm.peer_user_id
        {
            dm_user_ids.insert(user_id);
        }
        let avatar = avatar_url(cx, &dm.avatar);
        items.push(PaletteItem {
            kind: PaletteItemKind::Direct,
            label: SharedString::from(dm.label.clone()),
            subtext: SharedString::default(),
            avatar,
            unread_count: dm.unread_count,
            last_sent_timestamp: dm.last_sent_timestamp,
            channel_id: Some(dm.id),
            user_id: dm.peer_user_id,
        });
    }

    let channel_list = ChannelList::global(cx);
    let clan_list = ClanList::global(cx);
    for channel in channel_list.read(cx).user_channels() {
        let subtext = clan_list
            .read(cx)
            .clan(channel.clan_id)
            .map(|clan| clan.name.to_uppercase())
            .unwrap_or_default();
        items.push(PaletteItem {
            kind: PaletteItemKind::Channel,
            label: SharedString::from(channel.name.clone()),
            subtext: SharedString::from(subtext),
            avatar: SharedString::default(),
            unread_count: channel.badge_count,
            last_sent_timestamp: channel.last_sent_timestamp,
            channel_id: Some(channel.id),
            user_id: None,
        });
    }

    let Some(users_store) = UsersByUserStore::try_global(cx) else {
        items.sort_by(cmp_items);
        return items;
    };

    let active_clan_id = clan_list.read(cx).active_clan_id;
    let members_store = ClanMembersStore::try_global(cx);
    for user in users_store.read(cx).users() {
        if dm_user_ids.contains(&user.id) {
            continue;
        }
        let label = member_label(user, active_clan_id, members_store.as_ref(), cx);
        let subtext = if user.username.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(user.username.clone())
        };
        items.push(PaletteItem {
            kind: PaletteItemKind::Member,
            label: SharedString::from(label),
            subtext,
            avatar: avatar_url(cx, &user.avatar_url),
            unread_count: 0,
            last_sent_timestamp: 0,
            channel_id: None,
            user_id: Some(user.id),
        });
    }

    items.sort_by(cmp_items);
    items
}

fn member_label(
    user: &User,
    active_clan_id: Option<ClanId>,
    members_store: Option<&Entity<ClanMembersStore>>,
    cx: &App,
) -> String {
    if let (Some(clan_id), Some(store)) = (active_clan_id, members_store)
        && let Some(member) = store.read(cx).member(clan_id, user.id)
    {
        let nick = member.name();
        if !nick.is_empty() {
            return nick.to_string();
        }
    }
    if !user.display_name.is_empty() {
        user.display_name.clone()
    } else {
        user.username.clone()
    }
}

fn avatar_url(cx: &App, raw: &str) -> SharedString {
    if raw.is_empty() {
        SharedString::default()
    } else {
        SharedString::from(crate::util::imgproxy::avatar_url(cx, raw))
    }
}

pub fn render_palette_row(theme: &Theme, item: &PaletteItem) -> AnyElement {
    let unread = item.unread_count;
    let show_badge = SHOW_UNREAD_BADGE_COUNT && unread > 0;
    let badge_label = if unread > 99 {
        SharedString::from("99+")
    } else {
        SharedString::from(unread.to_string())
    };

    let leading: AnyElement = match item.kind {
        PaletteItemKind::Channel => div()
            .size(px(20.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                crate::components::primitives::Icon::new(crate::components::primitives::IconName::Hashtag)
                    .size(px(14.))
                    .text_color(theme.tokens.text_theme_primary),
            )
            .into_any_element(),
        PaletteItemKind::Direct | PaletteItemKind::Member => {
            if item.avatar.is_empty() {
                div()
                    .size(px(20.))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(theme.tokens.bg_active_member_channel)
                    .into_any_element()
            } else {
                img(item.avatar.clone())
                    .size(px(20.))
                    .rounded_full()
                    .flex_shrink_0()
                    .into_any_element()
            }
        }
    };

    let label = div()
        .truncate()
        .text_size(px(15.))
        .text_color(theme.tokens.text_secondary)
        .child(item.label.clone());

    let subtext = (!item.subtext.is_empty()).then(|| {
        div()
            .truncate()
            .text_size(px(10.))
            .text_color(theme.tokens.text_theme_primary)
            .child(item.subtext.clone())
    });

    div()
        .id(format!(
            "palette-item-{}-{}-{}",
            palette_item_kind_id(item.kind),
            item.channel_id.map(|id| id.get()).unwrap_or(0),
            item.user_id.map(|id| id.get()).unwrap_or(0),
        ))
        .h(px(ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
        .child(leading)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(label)
                .children(subtext),
        )
        .when(show_badge, |row| {
            row.child(
                div()
                    .flex_shrink_0()
                    .px_1()
                    .min_w(px(18.))
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme.status_dnd)
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_message)
                    .child(badge_label),
            )
        })
        .on_click(|_: &ClickEvent, _, _| {})
        .into_any_element()
}

fn palette_item_kind_id(kind: PaletteItemKind) -> u8 {
    match kind {
        PaletteItemKind::Channel => 0,
        PaletteItemKind::Direct => 1,
        PaletteItemKind::Member => 2,
    }
}
