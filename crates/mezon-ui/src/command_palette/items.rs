use std::collections::HashSet;

use gpui::{
    AnyElement, App, ClickEvent, Entity, FontWeight, Pixels, SharedString, div, img, prelude::*,
    px,
};
use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, ClanMembersStore, DirectKind, DirectMessageStore,
    User, UserId, UsersByUserStore,
};
use unicode_normalization::UnicodeNormalization;

use crate::theme::Theme;
use crate::SHOW_UNREAD_BADGE_COUNT;

pub(crate) const ROW_PX: f32 = 32.;

pub(crate) fn normalize_string(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    value
        .nfd()
        .filter(|ch| !('\u{0300}'..='\u{036f}').contains(ch))
        .collect::<String>()
        .to_uppercase()
}

pub(crate) fn normalize_search_string(value: &str) -> String {
    normalize_string(value)
        .replace('-', " ")
        .replace('_', " ")
        .replace('+', " ")
}

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
    pub(crate) filter_prioritize: String,
    pub(crate) filter_name: String,
    pub(crate) filter_display: String,
    pub(crate) filter_blob: String,
}

impl PaletteItem {
    pub(crate) fn matches_search(&self, search: &str) -> bool {
        if search.is_empty() {
            return true;
        }
        self.filter_prioritize.contains(search)
            || self.filter_name.contains(search)
            || self.filter_display.contains(search)
            || self.filter_blob.contains(search)
    }
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

    let users_store = UsersByUserStore::try_global(cx);
    let dm_store = DirectMessageStore::global(cx);
    for dm in dm_store.read(cx).channels() {
        if dm.kind == DirectKind::Dm
            && let Some(user_id) = dm.peer_user_id
        {
            dm_user_ids.insert(user_id);
        }
        let avatar = avatar_url(cx, &dm.avatar);
        let label = dm.label.clone();
        let username = dm
            .peer_user_id
            .and_then(|user_id| {
                users_store
                    .as_ref()
                    .and_then(|store| store.read(cx).user(user_id))
                    .map(|user| user.username.clone())
            })
            .unwrap_or_default();
        let subtext = if username.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(username.clone())
        };
        items.push(PaletteItem {
            kind: PaletteItemKind::Direct,
            label: SharedString::from(label.clone()),
            subtext,
            avatar,
            unread_count: dm.unread_count,
            last_sent_timestamp: dm.last_sent_timestamp,
            channel_id: Some(dm.id),
            user_id: dm.peer_user_id,
            filter_prioritize: normalize_search_string(&label),
            filter_name: normalize_search_string(&username),
            filter_display: String::new(),
            filter_blob: normalize_search_string(&format!("{label} {username}")),
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
        let name = channel.name.clone();
        items.push(PaletteItem {
            kind: PaletteItemKind::Channel,
            label: SharedString::from(name.clone()),
            subtext: SharedString::from(subtext.clone()),
            avatar: SharedString::default(),
            unread_count: channel.badge_count,
            last_sent_timestamp: channel.last_sent_timestamp,
            channel_id: Some(channel.id),
            user_id: None,
            filter_prioritize: normalize_search_string(&name),
            filter_name: normalize_search_string(&name),
            filter_display: String::new(),
            filter_blob: normalize_search_string(&format!("{name} {subtext}")),
        });
    }

    let Some(users_store) = users_store else {
        items.sort_by(cmp_items);
        return items;
    };

    let active_clan_id = clan_list.read(cx).active_clan_id;
    let members_store = ClanMembersStore::try_global(cx);
    for user in users_store.read(cx).users() {
        if dm_user_ids.contains(&user.id) {
            continue;
        }
        let prioritize = member_label(user, active_clan_id, members_store.as_ref(), cx);
        let username = user.username.clone();
        let display_name = user.display_name.clone();
        let label = if display_name.is_empty() {
            username.clone()
        } else {
            display_name.clone()
        };
        let subtext = if username.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(username.clone())
        };
        let search_blob = [username.as_str(), display_name.as_str(), prioritize.as_str()]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(".");
        items.push(PaletteItem {
            kind: PaletteItemKind::Member,
            label: SharedString::from(label.clone()),
            subtext,
            avatar: avatar_url(cx, &user.avatar_url),
            unread_count: 0,
            last_sent_timestamp: 0,
            channel_id: None,
            user_id: Some(user.id),
            filter_prioritize: normalize_search_string(&prioritize),
            filter_name: normalize_search_string(&username),
            filter_display: normalize_search_string(&display_name),
            filter_blob: normalize_search_string(&search_blob),
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

pub fn render_palette_row(theme: &Theme, item: &PaletteItem, search_query: &str) -> AnyElement {
    let unread = item.unread_count;
    let show_badge = SHOW_UNREAD_BADGE_COUNT && unread > 0;
    let badge_label = if unread > 99 {
        SharedString::from("99+")
    } else {
        SharedString::from(unread.to_string())
    };
    let highlight = highlight_query(search_query, item.kind);
    let label_weight = if unread > 0 {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::MEDIUM
    };
    let (subtext_size, _subtext_uppercase) = match item.kind {
        PaletteItemKind::Channel => (px(10.), true),
        PaletteItemKind::Direct | PaletteItemKind::Member => (px(13.), false),
    };
    let highlight_label =
        !search_query.starts_with('@') || !matches!(item.kind, PaletteItemKind::Member);

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

    let label = render_highlighted_text(
        &item.label,
        highlight,
        theme,
        px(15.),
        label_weight,
        highlight_label,
    );

    let subtext = (!item.subtext.is_empty()).then(|| {
        div()
            .flex_shrink_0()
            .max_w(px(240.))
            .child(render_highlighted_text(
                &item.subtext,
                highlight,
                theme,
                subtext_size,
                FontWeight::SEMIBOLD,
                true,
            ))
    });

    let row_content = match item.kind {
        PaletteItemKind::Channel => div()
            .flex_1()
            .min_w_0()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .overflow_hidden()
                    .child(leading)
                    .child(label),
            )
            .children(subtext)
            .into_any_element(),
        PaletteItemKind::Direct | PaletteItemKind::Member => div()
            .flex_1()
            .min_w_0()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .overflow_hidden()
            .child(leading)
            .child(label)
            .children(subtext)
            .into_any_element(),
    };

    div()
        .id(format!(
            "palette-item-{}-{}-{}",
            palette_item_kind_id(item.kind),
            item.channel_id.map(|id| id.get()).unwrap_or(0),
            item.user_id.map(|id| id.get()).unwrap_or(0),
        ))
        .w_full()
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
        .child(row_content)
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

fn highlight_query(raw_query: &str, kind: PaletteItemKind) -> &str {
    match kind {
        PaletteItemKind::Member if raw_query.starts_with('@') => {
            raw_query.get(1..).unwrap_or_default()
        }
        PaletteItemKind::Channel if raw_query.starts_with('#') => {
            raw_query.get(1..).unwrap_or_default()
        }
        _ => raw_query,
    }
}

fn render_highlighted_text(
    text: &str,
    query: &str,
    theme: &Theme,
    text_size: Pixels,
    base_weight: FontWeight,
    highlight: bool,
) -> AnyElement {
    let color = theme.tokens.text_theme_primary;
    let plain = |value: &str| {
        div()
            .truncate()
            .text_size(text_size)
            .font_weight(base_weight)
            .text_color(color)
            .child(value.to_string())
            .into_any_element()
    };

    if !highlight || query.is_empty() {
        return plain(text);
    }

    let normalized_text = normalize_string(text);
    let normalized_query = normalize_string(query);
    if normalized_query.is_empty() {
        return plain(text);
    }

    let Some(index) = normalized_text.find(normalized_query.as_str()) else {
        return plain(text);
    };

    if normalized_text.len() != text.len() {
        return plain(text);
    }

    let (before, rest) = text.split_at(index);
    let match_len = query.len().min(rest.len());
    let (matched, after) = rest.split_at(match_len);

    div()
        .flex()
        .flex_row()
        .truncate()
        .min_w_0()
        .text_size(text_size)
        .text_color(color)
        .child(
            div()
                .font_weight(base_weight)
                .child(before.to_string()),
        )
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(matched.to_string()),
        )
        .child(
            div()
                .font_weight(base_weight)
                .child(after.to_string()),
        )
        .into_any_element()
}

fn palette_item_kind_id(kind: PaletteItemKind) -> u8 {
    match kind {
        PaletteItemKind::Channel => 0,
        PaletteItemKind::Direct => 1,
        PaletteItemKind::Member => 2,
    }
}
