use std::collections::HashSet;

use std::rc::Rc;

use gpui::{
    AnyElement, App, Entity, FontWeight, Pixels, SharedString, div, img, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, ClanList, ClanMembersStore, DirectChannel,
    DirectKind, DirectMessageStore, User, UserId, UsersByUserStore,
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
    pub last_seen_timestamp: i64,
    pub channel_id: Option<ChannelId>,
    pub clan_id: Option<ClanId>,
    pub user_id: Option<UserId>,
    pub channel_type: Option<ChannelType>,
    pub dm_kind: Option<DirectKind>,
    pub dm_channel_type: Option<i32>,
    pub(crate) filter_prioritize: String,
    pub(crate) filter_name: String,
    pub(crate) filter_display: String,
    pub(crate) filter_blob: String,
}

impl PaletteItem {
    pub fn is_unread(&self) -> bool {
        self.unread_count > 0
            || (self.last_sent_timestamp > 0
                && self.last_seen_timestamp < self.last_sent_timestamp)
    }
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

fn dm_username_subtext(
    dm: &DirectChannel,
    users_store: Option<&Entity<UsersByUserStore>>,
    cx: &App,
) -> SharedString {
    if dm.kind != DirectKind::Dm {
        return SharedString::default();
    }
    if !dm.peer_username.is_empty() {
        return SharedString::from(dm.peer_username.clone());
    }
    if let Some(user_id) = dm.peer_user_id {
        if let Some(store) = users_store {
            if let Some(user) = store.read(cx).user(user_id) {
                if !user.username.is_empty() {
                    return SharedString::from(user.username.clone());
                }
            }
        }
    }
    SharedString::default()
}

fn palette_channel_subtext(
    channel: &mezon_store::Channel,
    channel_list: &ChannelList,
    clan_list: &ClanList,
) -> String {
    let raw = if channel.channel_type == ChannelType::Thread {
        channel
            .parent_id
            .and_then(|parent_id| channel_list.channel_display_name(channel.clan_id, parent_id))
            .unwrap_or_default()
    } else if !channel.clan_name.is_empty() {
        channel.clan_name.clone()
    } else {
        clan_list
            .clan(channel.clan_id)
            .map(|clan| clan.name.clone())
            .unwrap_or_default()
    };
    raw.to_uppercase()
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
        let subtext = dm_username_subtext(dm, users_store.as_ref(), cx);
        let filter_name = normalize_search_string(subtext.as_ref());
        let filter_blob = normalize_search_string(&format!("{label} {}", subtext.as_ref()));
        items.push(PaletteItem {
            kind: PaletteItemKind::Direct,
            label: SharedString::from(label.clone()),
            subtext,
            avatar,
            unread_count: dm.unread_count,
            last_sent_timestamp: dm.last_sent_timestamp,
            last_seen_timestamp: dm.last_seen_timestamp,
            channel_id: Some(dm.id),
            clan_id: None,
            user_id: dm.peer_user_id,
            channel_type: None,
            dm_kind: Some(dm.kind),
            dm_channel_type: Some(dm.kind.channel_type()),
            filter_prioritize: normalize_search_string(&label),
            filter_name,
            filter_display: String::new(),
            filter_blob,
        });
    }

    let channel_list = ChannelList::global(cx);
    let clan_list = ClanList::global(cx);
    let channels = channel_list.read(cx);
    let clans = clan_list.read(cx);
    for channel in channels.user_channels() {
        let subtext = palette_channel_subtext(channel, channels, clans);
        let name = channel.name.clone();
        items.push(PaletteItem {
            kind: PaletteItemKind::Channel,
            label: SharedString::from(name.clone()),
            subtext: SharedString::from(subtext.clone()),
            avatar: SharedString::default(),
            unread_count: channel.badge_count,
            last_sent_timestamp: channel.last_sent_timestamp,
            last_seen_timestamp: channel.last_seen_timestamp,
            channel_id: Some(channel.id),
            clan_id: Some(channel.clan_id),
            user_id: None,
            channel_type: Some(channel.channel_type),
            dm_kind: None,
            dm_channel_type: None,
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
            last_seen_timestamp: 0,
            channel_id: None,
            clan_id: None,
            user_id: Some(user.id),
            channel_type: None,
            dm_kind: None,
            dm_channel_type: None,
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

pub struct PaletteRowActions {
    pub on_hover: Rc<dyn Fn(&mut App)>,
    pub on_click: Rc<dyn Fn(&mut App)>,
}

pub fn render_palette_row(
    theme: &Theme,
    item: &PaletteItem,
    search_query: &str,
    selected: bool,
    actions: Option<PaletteRowActions>,
) -> AnyElement {
    let unread = item.unread_count;
    let show_badge = SHOW_UNREAD_BADGE_COUNT && unread > 0;
    let emphasized = item.is_unread();
    let badge_label = if unread > 99 {
        SharedString::from("99+")
    } else {
        SharedString::from(unread.to_string())
    };
    let highlight = highlight_query(search_query, item.kind);
    let label_weight = if emphasized {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::MEDIUM
    };
    let label_color = if emphasized {
        theme.tokens.text_theme_primary_hover
    } else {
        theme.tokens.text_theme_primary
    };
    let (subtext_size, subtext_uppercase) = match item.kind {
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
                    .text_color(if emphasized {
                        theme.tokens.text_theme_primary_hover
                    } else {
                        theme.tokens.text_theme_primary
                    }),
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
        px(15.),
        label_weight,
        label_color,
        highlight_label,
    );

    let subtext = (!item.subtext.is_empty()).then(|| {
        let text = if subtext_uppercase {
            item.subtext.to_uppercase()
        } else {
            item.subtext.to_string()
        };
        div()
            .flex_shrink_0()
            .max_w(px(240.))
            .child(render_highlighted_text(
                &text,
                highlight,
                subtext_size,
                FontWeight::SEMIBOLD,
                theme.tokens.text_theme_primary,
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

    let mut row = div()
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
        .when(selected, |row| row.bg(theme.tokens.bg_item_theme_hover))
        .when(!selected, |row| row.hover(|s| s.bg(theme.tokens.bg_item_theme_hover)))
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
        });
    if let Some(actions) = actions {
        row = row
            .on_mouse_move({
                let on_hover = actions.on_hover.clone();
                move |_, _, cx| on_hover(cx)
            })
            .on_click({
                let on_click = actions.on_click.clone();
                move |_, _, cx| on_click(cx)
            });
    }
    row.into_any_element()
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
    text_size: Pixels,
    base_weight: FontWeight,
    color: gpui::Rgba,
    highlight: bool,
) -> AnyElement {
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
