use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, FontWeight, MouseButton, Rgba,
    SharedString, Subscription, UniformListScrollHandle, Window, deferred, div, prelude::*, px,
    uniform_list,
};
use mezon_store::{BreakoutInvite, BreakoutRule, ChannelId, ClanId, UserId, VoiceStore};
use mezon_widgets::{Icon, IconName};

use crate::Shell;
use crate::components::primitives::{Button, ButtonVariants, Checkbox, Radio, h_flex, v_flex};
use crate::theme::ActiveTheme;

const MAX_BREAKOUT_ROOMS: u32 = 50;
const MENU_ROW_HEIGHT: f32 = 28.;
const MENU_MAX_HEIGHT: f32 = 224.;
const MENU_PADDING: f32 = 8.;
const MENU_MESSAGE_HEIGHT: f32 = 48.;
const ROOM_ROW_HEIGHT: f32 = 36.;
const HOSTED_ROW_HEIGHT: f32 = 30.;
const HOSTED_LIST_MAX_HEIGHT: f32 = 220.;

fn locale(cx: &App) -> String {
    mezon_store::Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default()
}

fn close(cx: &mut App) {
    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
}

fn room_label(locale: &str, index: usize) -> String {
    mezon_i18n::t(locale, "channelVoice.breakout.room")
        .replace("{{number}}", &(index + 1).to_string())
}

fn modal_title(locale: &str, theme: &crate::theme::Theme) -> gpui::Div {
    h_flex()
        .items_center()
        .gap_2()
        .child(
            Icon::new(IconName::VoiceBreakoutIcon)
                .size(px(20.))
                .text_color(theme.tokens.text_theme_primary),
        )
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(locale, "channelVoice.breakout.title")),
        )
}

fn toggle_participant_room(rooms_of: &mut HashMap<UserId, u32>, user_id: UserId, room: u32) {
    if rooms_of.get(&user_id).copied() == Some(room) {
        rooms_of.remove(&user_id);
    } else {
        rooms_of.insert(user_id, room);
    }
}

fn redistribute_automatic<S>(
    rooms_of: &mut HashMap<UserId, u32>,
    participants: &[(UserId, S)],
    room_count: usize,
) {
    rooms_of.clear();
    if room_count == 0 {
        return;
    }
    let mut seen = HashSet::with_capacity(participants.len());
    let mut idx = 0usize;
    for (uid, _) in participants {
        if !seen.insert(*uid) {
            continue;
        }
        rooms_of.insert(*uid, (idx % room_count) as u32);
        idx += 1;
    }
}

fn place_unassigned_participants<S>(
    rooms_of: &mut HashMap<UserId, u32>,
    participants: &[(UserId, S)],
    room_count: usize,
) {
    if room_count == 0 {
        return;
    }
    let mut counts = vec![0usize; room_count];
    for (user_id, _) in participants {
        if let Some(count) = rooms_of
            .get(user_id)
            .and_then(|&room| counts.get_mut(room as usize))
        {
            *count += 1;
        }
    }
    for (user_id, _) in participants {
        if rooms_of
            .get(user_id)
            .is_some_and(|&room| (room as usize) < room_count)
        {
            continue;
        }
        let Some((room, count)) = counts
            .iter_mut()
            .enumerate()
            .min_by_key(|(_, count)| **count)
        else {
            return;
        };
        *count += 1;
        rooms_of.insert(*user_id, room as u32);
    }
}

fn rebind_rooms_to_idle_prefix(
    rooms: &mut [BreakoutRoomItem],
    idle_channels: &[(ChannelId, SharedString)],
) {
    if idle_channels.len() < rooms.len() {
        return;
    }
    for (room, (channel_id, name)) in rooms.iter_mut().zip(idle_channels.iter()) {
        room.channel_id = *channel_id;
        room.name = name.clone();
    }
}

fn delete_last_room<S>(
    rooms: &mut Vec<BreakoutRoomItem>,
    rooms_of: &mut HashMap<UserId, u32>,
    participants: &[(UserId, S)],
    redistribute: bool,
) -> bool {
    if rooms.len() <= 1 {
        return false;
    }
    rooms.pop();
    if redistribute {
        redistribute_automatic(rooms_of, participants, rooms.len());
    } else {
        let remaining = rooms.len() as u32;
        rooms_of.retain(|_, room| *room < remaining);
    }
    true
}

fn build_manual_assignments<S>(
    room_count: u32,
    rooms_of: &HashMap<UserId, u32>,
    participants: &[(UserId, S)],
) -> Vec<Vec<i64>> {
    let mut assignments = vec![Vec::new(); room_count as usize];
    let mut seen = HashSet::with_capacity(participants.len());
    for (user_id, _) in participants {
        if !seen.insert(*user_id) {
            continue;
        }
        if let Some(&room) = rooms_of.get(user_id).filter(|&&room| room < room_count) {
            assignments[room as usize].push(user_id.0);
        }
    }
    assignments
}

fn can_create_room_count(count: u32, available_idle: u32) -> bool {
    (1..=MAX_BREAKOUT_ROOMS).contains(&count) && count <= available_idle
}

fn can_open_selected_rooms(
    rooms: &[BreakoutRoomItem],
    idle_channels: &[(ChannelId, SharedString)],
) -> bool {
    !rooms.is_empty()
        && rooms.len() <= idle_channels.len()
        && rooms
            .iter()
            .all(|room| idle_channels.iter().any(|(cid, _)| *cid == room.channel_id))
}

fn can_open_breakout(
    rooms: &[BreakoutRoomItem],
    idle_channels: &[(ChannelId, SharedString)],
    participant_count: usize,
    rule: BreakoutRule,
    assigned_count: usize,
) -> bool {
    can_open_selected_rooms(rooms, idle_channels)
        && participant_count > 0
        && (rule == BreakoutRule::SelfSelect || assigned_count > 0)
}

fn pick_next_unused_idle_channel(
    rooms: &[BreakoutRoomItem],
    idle_channels: &[(ChannelId, SharedString)],
) -> Option<(ChannelId, SharedString)> {
    if (rooms.len() as u32) >= MAX_BREAKOUT_ROOMS {
        return None;
    }
    idle_channels
        .iter()
        .find(|(cid, _)| !rooms.iter().any(|r| r.channel_id == *cid))
        .cloned()
}

fn resolve_participants(voice: &Entity<VoiceStore>, cx: &App) -> Vec<(UserId, SharedString)> {
    let store = voice.read(cx);
    let clan_id = store
        .connection()
        .connected_channel()
        .and_then(|(_, clan)| clan.parse::<i64>().ok())
        .map(ClanId);
    let me = mezon_store::BadgeService::try_global(cx)
        .and_then(|badges| badges.read(cx).current_user_id(cx));
    let mut seen = HashSet::new();
    store
        .participants()
        .iter()
        .filter(|p| !p.is_local && !p.is_agent)
        .filter_map(|p| {
            let user_id = p.identity.parse::<UserId>().ok()?;
            if Some(user_id) == me || !seen.insert(user_id) {
                return None;
            }
            let fallback = if p.metadata_name.is_empty() {
                &p.name
            } else {
                &p.metadata_name
            };
            let name =
                crate::util::voice_member::resolve_user_display(cx, clan_id, user_id, fallback, "")
                    .name;
            let label = if name.is_empty() {
                fallback.clone()
            } else {
                name
            };
            Some((user_id, SharedString::from(label)))
        })
        .collect()
}

fn group_members_by_room(
    room_count: usize,
    rooms_of: &HashMap<UserId, u32>,
    participants: &[(UserId, SharedString)],
) -> Vec<Vec<(UserId, SharedString)>> {
    let mut members = vec![Vec::new(); room_count];
    for (user_id, name) in participants {
        if let Some(slot) = rooms_of
            .get(user_id)
            .and_then(|&room| members.get_mut(room as usize))
        {
            slot.push((*user_id, name.clone()));
        }
    }
    members
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListRow {
    Room(usize),
    Member(usize, usize),
}

fn build_list_rows(member_counts: impl IntoIterator<Item = usize>) -> Vec<ListRow> {
    let mut rows = Vec::new();
    for (room, count) in member_counts.into_iter().enumerate() {
        rows.push(ListRow::Room(room));
        rows.extend((0..count).map(|member| ListRow::Member(room, member)));
    }
    rows
}

fn is_eligible_idle_voice_channel(
    channel: &mezon_store::Channel,
    parent_id: ChannelId,
    locally_removed: bool,
) -> bool {
    channel.channel_type == mezon_store::ChannelType::Voice
        && !channel.private
        && !channel.is_archived()
        && channel.active != mezon_store::CHANNEL_ACTIVE_ARCHIVED
        && !locally_removed
        && channel.id != parent_id
        && !channel.voice_members.iter().any(|m| m.user_id.0 > 0)
}

fn sort_and_label_idle_channels(
    mut raw: Vec<(ChannelId, String)>,
    locale: &str,
) -> Vec<(ChannelId, SharedString)> {
    raw.sort_by_key(|(id, _)| id.0);
    raw.into_iter()
        .enumerate()
        .map(|(idx, (id, name))| {
            let label = if name.trim().is_empty() {
                room_label(locale, idx)
            } else {
                name
            };
            (id, SharedString::from(label))
        })
        .collect()
}

fn resolve_idle_voice_channels(
    voice: &Entity<VoiceStore>,
    locale: &str,
    cx: &App,
) -> Vec<(ChannelId, SharedString)> {
    let Some((parent_str, clan_str)) = voice.read(cx).connection().connected_channel() else {
        return Vec::new();
    };
    let (Ok(parent_raw), Ok(clan_raw)) = (parent_str.parse::<i64>(), clan_str.parse::<i64>())
    else {
        return Vec::new();
    };
    let parent_id = ChannelId(parent_raw);
    let clan_id = ClanId(clan_raw);
    let Some(channel_list) = mezon_store::ChannelList::try_global(cx) else {
        return Vec::new();
    };
    let list = channel_list.read(cx);
    let mut seen = HashSet::new();
    let mut raw = Vec::new();
    for category in list.categories_for_clan(clan_id) {
        if category.id == mezon_store::FAVOR_CATE_ID {
            continue;
        }
        for channel in &category.channels {
            if !is_eligible_idle_voice_channel(
                channel,
                parent_id,
                list.is_locally_removed(channel.id),
            ) {
                continue;
            }
            if !seen.insert(channel.id) {
                continue;
            }
            raw.push((channel.id, channel.name.clone()));
        }
    }
    sort_and_label_idle_channels(raw, locale)
}

fn hosted_room_channel(
    list: &mezon_store::ChannelList,
    clan_id: Option<ClanId>,
    room_id: ChannelId,
) -> Option<&mezon_store::Channel> {
    clan_id
        .and_then(|clan| list.channel(clan, room_id))
        .or_else(|| list.linkable_channel(room_id))
}

fn hosted_rooms_signature(voice: &Entity<VoiceStore>, cx: &App) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let store = voice.read(cx);
    let Some((channel_id, clan_str)) = store.connection().connected_channel() else {
        return hasher.finish();
    };
    let Some(hosted) = store.hosted_breakout(channel_id) else {
        return hasher.finish();
    };
    channel_id.hash(&mut hasher);
    clan_str.hash(&mut hasher);
    let clan_id = clan_str.parse::<i64>().ok().map(ClanId);
    let channel_list = mezon_store::ChannelList::try_global(cx);
    let list = channel_list.as_ref().map(|channels| channels.read(cx));
    for &room_id in &hosted.rooms {
        room_id.hash(&mut hasher);
        let Some(channel) =
            list.and_then(|list| hosted_room_channel(list, clan_id, ChannelId(room_id)))
        else {
            continue;
        };
        channel.name.hash(&mut hasher);
        for member in &channel.voice_members {
            member.user_id.0.hash(&mut hasher);
            member.display_name.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn resolve_hosted_room_statuses(
    voice: &Entity<VoiceStore>,
    locale: &str,
    cx: &App,
) -> Vec<(ChannelId, SharedString, Vec<(UserId, SharedString)>)> {
    let store = voice.read(cx);
    let Some((channel_id, clan_str)) = store.connection().connected_channel() else {
        return Vec::new();
    };
    let Some(hosted) = store.hosted_breakout(channel_id) else {
        return Vec::new();
    };
    let clan_id = clan_str.parse::<i64>().ok().map(ClanId);
    let channel_list = mezon_store::ChannelList::try_global(cx);
    let list = channel_list.as_ref().map(|channels| channels.read(cx));
    let structure_loaded = list
        .zip(clan_id)
        .is_some_and(|(list, clan)| !list.categories_for_clan(clan).is_empty());
    let invited: HashSet<i64> = hosted.invitees.iter().copied().collect();
    hosted
        .rooms
        .iter()
        .enumerate()
        .filter_map(|(idx, &raw_room_id)| {
            let room_cid = ChannelId(raw_room_id);
            let channel = list.and_then(|list| hosted_room_channel(list, clan_id, room_cid));
            if channel.is_none() && structure_loaded {
                return None;
            }
            let mut room_name = SharedString::from(room_label(locale, idx));
            let mut members = Vec::new();
            if let Some(channel) = channel {
                if !channel.name.trim().is_empty() {
                    room_name = SharedString::from(channel.name.clone());
                }
                let mut seen = HashSet::new();
                for member in &channel.voice_members {
                    let uninvited = !invited.is_empty() && !invited.contains(&member.user_id.0);
                    if member.user_id.0 <= 0 || uninvited || !seen.insert(member.user_id) {
                        continue;
                    }
                    let resolved =
                        crate::util::voice_member::resolve_display(cx, clan_id, member).name;
                    let label = if resolved.is_empty() {
                        member.display_name.clone()
                    } else {
                        resolved
                    };
                    members.push((member.user_id, SharedString::from(label)));
                }
            }
            Some((room_cid, room_name, members))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BreakoutRoomItem {
    channel_id: ChannelId,
    name: SharedString,
    collapsed: bool,
}

type HostedRoomStatus = (ChannelId, SharedString, Vec<(UserId, SharedString)>);

#[derive(Clone, Copy)]
struct RowColors {
    text_primary: Rgba,
    text_secondary: Rgba,
    bg_secondary: Rgba,
    bg_hover: Rgba,
    border: Rgba,
    accent: Rgba,
    danger: Rgba,
    online: Rgba,
}

impl RowColors {
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            text_primary: theme.tokens.text_theme_primary,
            text_secondary: theme.tokens.text_secondary,
            bg_secondary: theme.tokens.bg_secondary,
            bg_hover: theme.tokens.bg_item_hover,
            border: theme.border,
            accent: theme.brand,
            danger: theme.danger,
            online: theme.status_online,
        }
    }
}

struct RoomRowLabels {
    assign: SharedString,
    delete: SharedString,
    move_to: SharedString,
    exchange: SharedString,
}

impl RoomRowLabels {
    fn new(locale: &str) -> Self {
        Self {
            assign: mezon_i18n::t(locale, "channelVoice.breakout.assign").into(),
            delete: mezon_i18n::t(locale, "channelVoice.breakout.delete").into(),
            move_to: mezon_i18n::t(locale, "channelVoice.breakout.moveTo").into(),
            exchange: mezon_i18n::t(locale, "channelVoice.breakout.exchange").into(),
        }
    }
}

pub struct BreakoutRoomsModal {
    focus_handle: FocusHandle,
    voice: Entity<VoiceStore>,
    rooms: Vec<BreakoutRoomItem>,
    room_members: Vec<Vec<(UserId, SharedString)>>,
    room_rows: Vec<ListRow>,
    rooms_scroll_handle: UniformListScrollHandle,
    recreate_count: u32,
    rule: BreakoutRule,
    draft_rule: BreakoutRule,
    recreating: bool,
    initial_create: bool,
    manual_dirty: bool,
    rooms_of: HashMap<UserId, u32>,
    assigning_room: Option<u32>,
    menu_items: Vec<usize>,
    menu_scroll_handle: UniformListScrollHandle,
    hovered_room: Option<usize>,
    hovered_user: Option<UserId>,
    deleting_room: Option<usize>,
    moving_user: Option<UserId>,
    exchanging_user: Option<UserId>,
    connected_channel: Option<(String, String)>,
    hosted_room_ids: Vec<i64>,
    hosted_rooms_sig: Option<u64>,
    hosted_room_statuses: Vec<HostedRoomStatus>,
    hosted_rows: Vec<ListRow>,
    hosted_scroll_handle: UniformListScrollHandle,
    pending: bool,
    opening: bool,
    participants_revision: u64,
    participants: Vec<(UserId, SharedString)>,
    idle_channels: Vec<(ChannelId, SharedString)>,
    _observe: Subscription,
    _channel_observe: Option<Subscription>,
    _members_observe: Option<Subscription>,
}

impl BreakoutRoomsModal {
    pub fn show(voice: Entity<VoiceStore>, window: &mut Window, cx: &mut App) {
        let view = cx.new(|cx| {
            let (connected_channel, hosted_room_ids, pending) = Self::read_voice_status(&voice, cx);
            let loc = locale(cx);
            let participants_revision = voice.read(cx).participant_roster_revision();
            let participants = resolve_participants(&voice, cx);
            let idle_channels = resolve_idle_voice_channels(&voice, &loc, cx);
            let hosted_sig = hosted_rooms_signature(&voice, cx);
            let hosted_room_statuses = if !hosted_room_ids.is_empty() {
                resolve_hosted_room_statuses(&voice, &loc, cx)
            } else {
                Vec::new()
            };
            let hosted_rows = build_list_rows(
                hosted_room_statuses
                    .iter()
                    .map(|(_, _, members)| members.len()),
            );
            let initial_rooms = idle_channels
                .first()
                .map(|(channel_id, name)| {
                    vec![BreakoutRoomItem {
                        channel_id: *channel_id,
                        name: name.clone(),
                        collapsed: false,
                    }]
                })
                .unwrap_or_default();
            let channel_observe = mezon_store::ChannelList::try_global(cx).map(|channels| {
                cx.subscribe(&channels, |this: &mut Self, _, event, cx| {
                    this.channels_changed(event, cx)
                })
            });
            let members_observe = mezon_store::ClanMembersStore::try_global(cx).map(|members| {
                cx.subscribe(
                    &members,
                    |this: &mut Self, _, event: &mezon_store::ClanMembersEvent, cx| {
                        this.members_changed(event.clan_id(), cx)
                    },
                )
            });
            let mut modal = Self {
                focus_handle: cx.focus_handle(),
                _observe: cx.observe(&voice, |this: &mut Self, voice, cx| {
                    if voice.read(cx).connection().connected_channel().is_none() {
                        let owner = cx.entity_id();
                        Shell::global(cx)
                            .update(cx, |shell, cx| shell.close_modal_if_current(owner, cx));
                        return;
                    }
                    if this.sync_from_voice(cx) {
                        cx.notify();
                    }
                    if this.opening && !this.pending {
                        this.opening = false;
                        if this.hosted_rooms().is_some() {
                            let owner = cx.entity_id();
                            Shell::global(cx)
                                .update(cx, |shell, cx| shell.close_modal_if_current(owner, cx));
                        }
                    }
                }),
                _channel_observe: channel_observe,
                _members_observe: members_observe,
                voice,
                rooms: initial_rooms,
                room_members: Vec::new(),
                room_rows: Vec::new(),
                rooms_scroll_handle: UniformListScrollHandle::new(),
                recreate_count: 1,
                rule: BreakoutRule::Manual,
                draft_rule: BreakoutRule::Manual,
                recreating: true,
                initial_create: true,
                manual_dirty: false,
                rooms_of: HashMap::new(),
                assigning_room: None,
                menu_items: Vec::new(),
                menu_scroll_handle: UniformListScrollHandle::new(),
                hovered_room: None,
                hovered_user: None,
                deleting_room: None,
                moving_user: None,
                exchanging_user: None,
                connected_channel,
                hosted_room_ids,
                hosted_rooms_sig: Some(hosted_sig),
                hosted_room_statuses,
                hosted_rows,
                hosted_scroll_handle: UniformListScrollHandle::new(),
                pending,
                opening: false,
                participants_revision,
                participants,
                idle_channels,
            };
            modal.refresh_room_rows();
            modal
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        focus_handle.focus(window, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn read_voice_status(
        voice: &Entity<VoiceStore>,
        cx: &App,
    ) -> (Option<(String, String)>, Vec<i64>, bool) {
        let store = voice.read(cx);
        let connected = store
            .connection()
            .connected_channel()
            .map(|(cid, clan)| (cid.to_string(), clan.to_string()));
        let hosted_room_ids = connected
            .as_ref()
            .and_then(|(channel_id, _)| store.hosted_breakout(channel_id))
            .map(|hosted| hosted.rooms.clone())
            .unwrap_or_default();
        (connected, hosted_room_ids, store.breakout_pending())
    }

    fn hosted_rooms(&self) -> Option<usize> {
        (!self.hosted_room_ids.is_empty()).then_some(self.hosted_room_ids.len())
    }

    fn clan_id(&self) -> Option<ClanId> {
        self.connected_channel
            .as_ref()
            .and_then(|(_, clan_id)| clan_id.parse::<i64>().ok())
            .map(ClanId)
    }

    fn sync_from_voice(&mut self, cx: &App) -> bool {
        let participants_changed = self.sync_participants(cx);
        let store = self.voice.read(cx);
        let connected = store.connection().connected_channel();
        let hosted_room_ids = connected
            .and_then(|(channel_id, _)| store.hosted_breakout(channel_id))
            .map_or(&[][..], |hosted| hosted.rooms.as_slice());
        let pending = store.breakout_pending();
        let connected_changed = self
            .connected_channel
            .as_ref()
            .map(|(channel_id, clan_id)| (channel_id.as_str(), clan_id.as_str()))
            != connected;
        let rooms_changed = self.hosted_room_ids.as_slice() != hosted_room_ids;
        if connected_changed {
            self.connected_channel =
                connected.map(|(channel_id, clan_id)| (channel_id.to_owned(), clan_id.to_owned()));
        }
        if rooms_changed {
            self.hosted_room_ids = hosted_room_ids.to_vec();
        }
        let status_changed = rooms_changed || self.pending != pending;
        self.pending = pending;
        let mut channels_changed = false;
        if connected_changed || rooms_changed {
            channels_changed |= self.refresh_idle_channels(cx);
            channels_changed |= self.refresh_hosted_statuses(cx);
        }
        status_changed || participants_changed || channels_changed
    }

    fn sync_participants(&mut self, cx: &App) -> bool {
        let revision = self.voice.read(cx).participant_roster_revision();
        if self.participants_revision == revision {
            return false;
        }
        self.participants_revision = revision;
        self.apply_participants(cx)
    }

    fn members_changed(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.clan_id() != Some(clan_id) {
            return;
        }
        let mut changed = self.apply_participants(cx);
        if self.hosted_rooms().is_some() {
            self.hosted_rooms_sig = None;
            changed |= self.refresh_hosted_statuses(cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_participants(&mut self, cx: &App) -> bool {
        let participants = resolve_participants(&self.voice, cx);
        if self.participants == participants {
            return false;
        }
        self.participants = participants;
        let valid_ids: HashSet<UserId> = self.participants.iter().map(|(uid, _)| *uid).collect();
        if self
            .hovered_user
            .is_some_and(|uid| !valid_ids.contains(&uid))
        {
            self.hovered_user = None;
        }
        if self
            .moving_user
            .is_some_and(|uid| !valid_ids.contains(&uid))
        {
            self.moving_user = None;
        }
        if self
            .exchanging_user
            .is_some_and(|uid| !valid_ids.contains(&uid))
        {
            self.exchanging_user = None;
        }
        if self.rule == BreakoutRule::Automatic && !self.manual_dirty && !self.rooms.is_empty() {
            place_unassigned_participants(&mut self.rooms_of, &self.participants, self.rooms.len());
        }
        self.refresh_room_rows();
        true
    }

    fn channels_changed(&mut self, event: &mezon_store::ChannelEvent, cx: &mut Context<Self>) {
        let clan_id = self.clan_id();
        let relevant = match event {
            mezon_store::ChannelEvent::VoiceMembersChanged(event_clan)
            | mezon_store::ChannelEvent::ClanChannelsLoaded(event_clan) => {
                Some(*event_clan) == clan_id
            }
            mezon_store::ChannelEvent::PrivacyChanged {
                clan_id: event_clan,
                ..
            } => Some(*event_clan) == clan_id,
            mezon_store::ChannelEvent::AccessLost(_) => true,
            _ => false,
        };
        if !relevant {
            return;
        }
        let changed = if self.hosted_rooms().is_some() {
            self.refresh_hosted_statuses(cx)
        } else {
            self.refresh_idle_channels(cx)
        };
        if changed {
            cx.notify();
        }
    }

    fn refresh_idle_channels(&mut self, cx: &App) -> bool {
        let idle_channels = resolve_idle_voice_channels(&self.voice, &locale(cx), cx);
        if self.idle_channels == idle_channels {
            return false;
        }
        self.idle_channels = idle_channels;
        rebind_rooms_to_idle_prefix(&mut self.rooms, &self.idle_channels);
        true
    }

    fn refresh_hosted_statuses(&mut self, cx: &App) -> bool {
        let hosted_sig = hosted_rooms_signature(&self.voice, cx);
        if self.hosted_rooms_sig == Some(hosted_sig) {
            return false;
        }
        self.hosted_rooms_sig = Some(hosted_sig);
        let statuses = if self.hosted_room_ids.is_empty() {
            Vec::new()
        } else {
            resolve_hosted_room_statuses(&self.voice, &locale(cx), cx)
        };
        if self.hosted_room_statuses == statuses {
            return false;
        }
        self.hosted_rows = build_list_rows(statuses.iter().map(|(_, _, members)| members.len()));
        self.hosted_room_statuses = statuses;
        true
    }

    fn refresh_room_rows(&mut self) {
        self.room_members =
            group_members_by_room(self.rooms.len(), &self.rooms_of, &self.participants);
        self.room_rows = build_list_rows(self.rooms.iter().zip(&self.room_members).map(
            |(room, members)| {
                if room.collapsed { 0 } else { members.len() }
            },
        ));
        self.refresh_menu_items();
    }

    fn room_count(&self) -> u32 {
        self.rooms.len() as u32
    }

    fn available_idle_count(&self) -> u32 {
        self.idle_channels.len() as u32
    }

    fn next_unused_idle_channel(&self) -> Option<(ChannelId, SharedString)> {
        pick_next_unused_idle_channel(&self.rooms, &self.idle_channels)
    }

    fn assigned_room(&self, user_id: UserId) -> Option<u32> {
        let count = self.room_count();
        self.rooms_of
            .get(&user_id)
            .copied()
            .filter(|&room| room < count)
    }

    fn assigned_participant_count(&self) -> usize {
        self.room_members.iter().map(Vec::len).sum()
    }

    fn set_recreate_count(&mut self, count: u32, cx: &mut Context<Self>) {
        self.recreate_count = count.clamp(1, MAX_BREAKOUT_ROOMS);
        cx.notify();
    }

    fn begin_recreate(&mut self, cx: &mut Context<Self>) {
        self.refresh_idle_channels(cx);
        self.clear_popovers();
        self.recreate_count = self.room_count().max(1);
        self.draft_rule = self.rule;
        self.recreating = true;
        cx.notify();
    }

    fn cancel_recreate(&mut self, cx: &mut Context<Self>) {
        self.draft_rule = self.rule;
        self.recreating = false;
        cx.notify();
    }

    fn apply_recreate(&mut self, cx: &mut Context<Self>) {
        if !can_create_room_count(self.recreate_count, self.available_idle_count()) {
            return;
        }
        let count = self.recreate_count as usize;
        self.rule = self.draft_rule;
        self.rooms = self
            .idle_channels
            .iter()
            .take(count)
            .map(|(channel_id, name)| BreakoutRoomItem {
                channel_id: *channel_id,
                name: name.clone(),
                collapsed: false,
            })
            .collect();
        self.rooms_of.clear();
        self.manual_dirty = false;
        if self.rule == BreakoutRule::Automatic && count > 0 {
            redistribute_automatic(&mut self.rooms_of, &self.participants, count);
        }
        self.recreating = false;
        self.initial_create = false;
        self.clear_popovers();
        self.refresh_room_rows();
        cx.notify();
    }

    fn clear_popovers(&mut self) {
        self.dismiss_menus();
        self.deleting_room = None;
    }

    fn dismiss_menus(&mut self) {
        self.assigning_room = None;
        self.moving_user = None;
        self.exchanging_user = None;
        self.menu_items.clear();
    }

    fn open_assign_menu(&mut self, room: u32, cx: &mut Context<Self>) {
        self.clear_popovers();
        self.assigning_room = Some(room);
        self.refresh_menu_items();
        cx.notify();
    }

    fn open_move_menu(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        self.clear_popovers();
        self.moving_user = Some(user_id);
        self.refresh_menu_items();
        cx.notify();
    }

    fn open_exchange_menu(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        self.clear_popovers();
        self.exchanging_user = Some(user_id);
        self.refresh_menu_items();
        cx.notify();
    }

    fn refresh_menu_items(&mut self) {
        let items = if let Some(room) = self.assigning_room {
            self.participants
                .iter()
                .enumerate()
                .filter(|(_, (uid, _))| {
                    self.assigned_room(*uid)
                        .is_none_or(|assigned| assigned == room)
                })
                .map(|(ix, _)| ix)
                .collect()
        } else if let Some(user_id) = self.moving_user {
            let current = self.assigned_room(user_id);
            (0..self.rooms.len())
                .filter(|&ix| Some(ix as u32) != current)
                .collect()
        } else if let Some(current) = self
            .exchanging_user
            .and_then(|user_id| self.assigned_room(user_id))
        {
            self.participants
                .iter()
                .enumerate()
                .filter(|(_, (uid, _))| {
                    self.assigned_room(*uid).is_some_and(|room| room != current)
                })
                .map(|(ix, _)| ix)
                .collect()
        } else {
            Vec::new()
        };
        self.menu_items = items;
    }

    fn add_room(&mut self, cx: &mut Context<Self>) {
        self.refresh_idle_channels(cx);
        let Some((channel_id, name)) = self.next_unused_idle_channel() else {
            return;
        };
        self.rooms.push(BreakoutRoomItem {
            channel_id,
            name,
            collapsed: false,
        });
        if self.rule == BreakoutRule::Automatic && !self.manual_dirty {
            redistribute_automatic(&mut self.rooms_of, &self.participants, self.rooms.len());
        }
        self.clear_popovers();
        self.refresh_room_rows();
        cx.notify();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let is_last_room = self
            .deleting_room
            .is_some_and(|idx| idx + 1 == self.rooms.len());
        if is_last_room {
            let redistribute = self.rule == BreakoutRule::Automatic && !self.manual_dirty;
            delete_last_room(
                &mut self.rooms,
                &mut self.rooms_of,
                &self.participants,
                redistribute,
            );
        }
        self.clear_popovers();
        self.refresh_room_rows();
        cx.notify();
    }

    fn toggle_room_collapsed(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(room) = self.rooms.get_mut(idx) {
            room.collapsed = !room.collapsed;
            self.refresh_room_rows();
            cx.notify();
        }
    }

    fn toggle_user_room(&mut self, user_id: UserId, room: u32, cx: &mut Context<Self>) {
        toggle_participant_room(&mut self.rooms_of, user_id, room);
        self.manual_dirty = true;
        self.refresh_room_rows();
        cx.notify();
    }

    fn move_user(&mut self, user_id: UserId, room: u32, cx: &mut Context<Self>) {
        self.rooms_of.insert(user_id, room);
        self.manual_dirty = true;
        self.moving_user = None;
        self.refresh_room_rows();
        cx.notify();
    }

    fn exchange_users(&mut self, user_id: UserId, other_id: UserId, cx: &mut Context<Self>) {
        if let (Some(room), Some(other_room)) =
            (self.assigned_room(user_id), self.assigned_room(other_id))
        {
            self.rooms_of.insert(user_id, other_room);
            self.rooms_of.insert(other_id, room);
            self.manual_dirty = true;
        }
        self.exchanging_user = None;
        self.refresh_room_rows();
        cx.notify();
    }

    fn effective_rule(&self) -> BreakoutRule {
        match self.rule {
            BreakoutRule::SelfSelect => BreakoutRule::SelfSelect,
            BreakoutRule::Automatic if !self.manual_dirty => BreakoutRule::Automatic,
            _ => BreakoutRule::Manual,
        }
    }

    fn open_all_rooms(&mut self, cx: &mut Context<Self>) {
        self.refresh_idle_channels(cx);
        let rule = self.effective_rule();
        let room_count = self.room_count();
        let assignments = if rule == BreakoutRule::SelfSelect {
            Vec::new()
        } else {
            build_manual_assignments(room_count, &self.rooms_of, &self.participants)
        };
        let assigned_count = assignments.iter().map(Vec::len).sum();
        if !can_open_breakout(
            &self.rooms,
            &self.idle_channels,
            self.participants.len(),
            rule,
            assigned_count,
        ) {
            return;
        }
        self.voice.update(cx, |store, cx| {
            store.open_breakout_rooms(room_count, rule, assignments, cx)
        });
        self.opening = self.voice.read(cx).breakout_pending();
        cx.notify();
    }

    fn popover_backdrop(
        id: &'static str,
        popover: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dismiss_menus();
                    cx.notify();
                }),
            )
            .child(popover)
            .into_any_element()
    }

    fn menu_anchor_row(&self) -> Option<usize> {
        if let Some(room) = self.assigning_room {
            return self
                .room_rows
                .iter()
                .position(|row| *row == ListRow::Room(room as usize));
        }
        let user_id = self.moving_user.or(self.exchanging_user)?;
        self.room_rows.iter().position(|row| match *row {
            ListRow::Member(room, member) => self
                .room_members
                .get(room)
                .and_then(|members| members.get(member))
                .is_some_and(|(uid, _)| *uid == user_id),
            ListRow::Room(_) => false,
        })
    }

    fn menu_top(&self, menu_height: f32) -> f32 {
        let Some(row_ix) = self.menu_anchor_row() else {
            return 0.;
        };
        let state = self.rooms_scroll_handle.0.borrow();
        let scroll_y = f32::from(state.base_handle.offset().y);
        let viewport = f32::from(state.base_handle.bounds().size.height);
        let row_top = row_ix as f32 * ROOM_ROW_HEIGHT + scroll_y;
        let below = row_top + ROOM_ROW_HEIGHT;
        if viewport > 0. && below + menu_height > viewport {
            (row_top - menu_height).max(0.)
        } else {
            below.max(0.)
        }
    }

    fn render_menu(&self, locale: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, width) = if self.assigning_room.is_some() {
            ("breakout-assign-popup", 220.)
        } else if self.moving_user.is_some() {
            ("breakout-move-menu", 160.)
        } else if self.exchanging_user.is_some() {
            ("breakout-exchange-menu", 200.)
        } else {
            return None;
        };
        let colors = RowColors::new(cx);
        let bg_floating = cx.theme().bg_floating;
        let empty_message =
            (self.assigning_room.is_some() && self.menu_items.is_empty()).then(|| {
                if self.participants.is_empty() {
                    mezon_i18n::t(locale, "channelVoice.breakout.noParticipants")
                } else {
                    mezon_i18n::t(locale, "channelVoice.breakout.allAssigned")
                }
            });
        let list_height = (self.menu_items.len() as f32 * MENU_ROW_HEIGHT).min(MENU_MAX_HEIGHT);
        let body_height = if empty_message.is_some() {
            MENU_MESSAGE_HEIGHT
        } else {
            list_height
        };
        let top = self.menu_top(body_height + 2. * MENU_PADDING + 2.);
        let body = match empty_message {
            Some(message) => div()
                .px_2()
                .py_2()
                .text_xs()
                .text_color(colors.text_secondary)
                .child(message)
                .into_any_element(),
            None => uniform_list(
                "breakout-menu-list",
                self.menu_items.len(),
                cx.processor(|this: &mut Self, range: Range<usize>, _window, cx| {
                    this.render_menu_rows(range, cx)
                }),
            )
            .track_scroll(&self.menu_scroll_handle)
            .w_full()
            .h(px(list_height))
            .into_any_element(),
        };
        let popup = v_flex()
            .id(id)
            .occlude()
            .absolute()
            .right(px(14.))
            .top(px(top))
            .w(px(width))
            .p(px(MENU_PADDING))
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(bg_floating)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(body)
            .into_any_element();
        Some(Self::popover_backdrop("breakout-menu-backdrop", popup, cx))
    }

    fn render_menu_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = RowColors::new(cx);
        range
            .map(|ix| {
                self.menu_items
                    .get(ix)
                    .copied()
                    .and_then(|item| self.render_menu_row(item, colors, cx))
                    .unwrap_or_else(|| div().h(px(MENU_ROW_HEIGHT)).into_any_element())
            })
            .collect()
    }

    fn render_menu_row(
        &self,
        item: usize,
        colors: RowColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = |id: ElementId| {
            h_flex()
                .id(id)
                .w_full()
                .h(px(MENU_ROW_HEIGHT))
                .px_2()
                .gap_2()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |style| style.bg(colors.bg_hover))
        };
        if let Some(room) = self.assigning_room {
            let (user_id, name) = self.participants.get(item)?.clone();
            let checked = self.assigned_room(user_id) == Some(room);
            return Some(
                row(("breakout-assign-user", user_id.0 as u64).into())
                    .child(
                        Checkbox::new(("breakout-assign-check", user_id.0 as u64)).checked(checked),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(colors.text_primary)
                            .child(name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_user_room(user_id, room, cx);
                    }))
                    .into_any_element(),
            );
        }
        if let Some(user_id) = self.moving_user {
            let target = self.rooms.get(item)?;
            let room = item as u32;
            return Some(
                row(("breakout-move-target", item).into())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(colors.text_primary)
                            .child(target.name.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.move_user(user_id, room, cx);
                    }))
                    .into_any_element(),
            );
        }
        let user_id = self.exchanging_user?;
        let (other_id, other_name) = self.participants.get(item)?.clone();
        let room_name = self
            .rooms
            .get(self.assigned_room(other_id)? as usize)?
            .name
            .clone();
        Some(
            row(("breakout-exchange-target", other_id.0 as u64).into())
                .justify_between()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(colors.text_primary)
                        .child(other_name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.text_secondary)
                        .child(room_name),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.exchange_users(user_id, other_id, cx);
                }))
                .into_any_element(),
        )
    }

    fn render_recreate_overlay(&self, locale: &str, cx: &mut Context<Self>) -> AnyElement {
        let (text_primary, danger_color, border_color, bg_card, bg_floating) = {
            let theme = cx.theme();
            (
                theme.tokens.text_theme_primary,
                theme.danger,
                theme.border,
                theme.tokens.bg_secondary,
                theme.bg_floating,
            )
        };
        let rule_row =
            |id: &'static str, rule: BreakoutRule, key: &'static str, cx: &mut Context<Self>| {
                Radio::new(id)
                    .selected(self.draft_rule == rule)
                    .label(mezon_i18n::t(locale, key))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft_rule = rule;
                        cx.notify();
                    }))
            };
        let count = self.recreate_count;
        let available_rooms = self.available_idle_count();
        let not_enough_rooms = !can_create_room_count(count, available_rooms);
        let header_template = mezon_i18n::t(locale, "channelVoice.breakout.createHeader");
        let (prefix, suffix) = header_template
            .split_once("{{count}}")
            .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
            .unwrap_or_else(|| {
                (
                    mezon_i18n::t(locale, "channelVoice.breakout.roomCount").to_string(),
                    String::new(),
                )
            });

        let confirm_key = if self.initial_create {
            "channelVoice.breakout.create"
        } else {
            "channelVoice.breakout.recreate"
        };

        div()
            .id("breakout-recreate-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(gpui::Hsla::from(bg_floating).opacity(0.65))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                v_flex()
                    .w(px(360.))
                    .p(px(24.))
                    .gap_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(border_color)
                    .bg(bg_card)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_primary)
                                    .child(prefix),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .px_1()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(border_color)
                                    .child(
                                        Button::new("breakout-count-down")
                                            .label("-")
                                            .ghost()
                                            .disabled(count <= 1)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_recreate_count(count - 1, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .w(px(28.))
                                            .text_center()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(text_primary)
                                            .child(count.to_string()),
                                    )
                                    .child(
                                        Button::new("breakout-count-up")
                                            .label("+")
                                            .ghost()
                                            .disabled(count >= MAX_BREAKOUT_ROOMS)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_recreate_count(count + 1, cx);
                                            })),
                                    ),
                            )
                            .when(!suffix.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_base()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text_primary)
                                        .child(suffix),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(rule_row(
                                "breakout-rule-auto",
                                BreakoutRule::Automatic,
                                "channelVoice.breakout.ruleAutomatic",
                                cx,
                            ))
                            .child(rule_row(
                                "breakout-rule-manual",
                                BreakoutRule::Manual,
                                "channelVoice.breakout.ruleManual",
                                cx,
                            ))
                            .child(rule_row(
                                "breakout-rule-self",
                                BreakoutRule::SelfSelect,
                                "channelVoice.breakout.ruleSelfSelect",
                                cx,
                            )),
                    )
                    .when(not_enough_rooms, |el| {
                        el.child(div().text_xs().text_color(danger_color).child(format!(
                            "{} ({})",
                            mezon_i18n::t(locale, "channelVoice.breakout.notEnoughRooms"),
                            available_rooms
                        )))
                    })
                    .child(
                        h_flex()
                            .pt_2()
                            .justify_end()
                            .gap_2()
                            .when(!self.initial_create, |el| {
                                el.child(
                                    Button::new("breakout-recreate-cancel")
                                        .label(mezon_i18n::t(locale, "common.cancel"))
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_recreate(cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new("breakout-recreate-confirm")
                                    .label(mezon_i18n::t(locale, confirm_key))
                                    .primary()
                                    .disabled(not_enough_rooms)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.apply_recreate(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_delete_overlay(&self, locale: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let idx = self.deleting_room?;
        let room_name = self.rooms.get(idx)?.name.to_string();
        let (text_primary, border_color, bg_card, bg_floating) = {
            let theme = cx.theme();
            (
                theme.tokens.text_theme_primary,
                theme.border,
                theme.tokens.bg_secondary,
                theme.bg_floating,
            )
        };
        let prompt = mezon_i18n::t(locale, "channelVoice.breakout.deleteConfirm")
            .replace("{{name}}", &room_name);
        Some(
            div()
                .id("breakout-delete-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .bg(gpui::Hsla::from(bg_floating).opacity(0.45))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    v_flex()
                        .w(px(240.))
                        .p(px(16.))
                        .gap_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(border_color)
                        .bg(bg_card)
                        .shadow_lg()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_center()
                                .text_color(text_primary)
                                .child(prompt),
                        )
                        .child(
                            h_flex()
                                .justify_center()
                                .gap_2()
                                .child(
                                    Button::new("breakout-delete-yes")
                                        .label(mezon_i18n::t(locale, "channelVoice.breakout.yes"))
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_delete(cx);
                                        })),
                                )
                                .child(
                                    Button::new("breakout-delete-no")
                                        .label(mezon_i18n::t(locale, "channelVoice.breakout.no"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deleting_room = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_room_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = RowColors::new(cx);
        let labels = RoomRowLabels::new(&locale(cx));
        let total_assigned = self.assigned_participant_count();
        range
            .map(|ix| match self.room_rows.get(ix).copied() {
                Some(ListRow::Room(idx)) => self.render_room_header(idx, colors, &labels, cx),
                Some(ListRow::Member(idx, member)) => {
                    self.render_room_member(idx, member, total_assigned, colors, &labels, cx)
                }
                None => div().h(px(ROOM_ROW_HEIGHT)).into_any_element(),
            })
            .collect()
    }

    fn render_room_header(
        &self,
        idx: usize,
        colors: RowColors,
        labels: &RoomRowLabels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(room) = self.rooms.get(idx) else {
            return div().h(px(ROOM_ROW_HEIGHT)).into_any_element();
        };
        let room_u32 = idx as u32;
        let count = self.room_members.get(idx).map_or(0, Vec::len);
        let is_hovered = self.hovered_room == Some(idx);
        let chevron = if !room.collapsed && count > 0 {
            IconName::CaretDown
        } else {
            IconName::CaretRight
        };
        let can_delete = self.rooms.len() > 1 && idx + 1 == self.rooms.len();
        let can_assign = self.rule != BreakoutRule::SelfSelect;
        let assign_text = if count == 0 {
            labels.assign.clone()
        } else {
            SharedString::from(count.to_string())
        };
        let delete_label = labels.delete.clone();
        h_flex()
            .id(("breakout-room-header", idx))
            .w_full()
            .h(px(ROOM_ROW_HEIGHT))
            .px(px(14.))
            .justify_between()
            .items_center()
            .bg(colors.bg_secondary)
            .border_b_1()
            .border_color(colors.border)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if this.hovered_room != Some(idx) || this.hovered_user.is_some() {
                        this.hovered_room = Some(idx);
                        this.hovered_user = None;
                        cx.notify();
                    }
                } else if this.hovered_room == Some(idx) {
                    this.hovered_room = None;
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        h_flex()
                            .id(("breakout-room-toggle", idx))
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_room_collapsed(idx, cx);
                            }))
                            .child(
                                Icon::new(chevron)
                                    .size(px(12.))
                                    .text_color(colors.text_secondary),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .child(room.name.clone()),
                            ),
                    )
                    .when(is_hovered && can_delete, |el| {
                        el.child(
                            h_flex()
                                .id(("breakout-room-delete", idx))
                                .items_center()
                                .gap_1()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(move |s| s.bg(colors.bg_hover))
                                .child(
                                    Icon::new(IconName::Close)
                                        .size(px(12.))
                                        .text_color(colors.danger),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.text_secondary)
                                        .child(delete_label),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_popovers();
                                    this.deleting_room = Some(idx);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(can_assign, |el| {
                el.child(
                    div()
                        .id(("breakout-assign", idx))
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(colors.bg_hover))
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.accent)
                        .child(assign_text)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_assign_menu(room_u32, cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn render_room_member(
        &self,
        room_idx: usize,
        member_idx: usize,
        total_assigned: usize,
        colors: RowColors,
        labels: &RoomRowLabels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((members, (uid, uname))) = self.room_members.get(room_idx).and_then(|members| {
            members
                .get(member_idx)
                .cloned()
                .map(|member| (members, member))
        }) else {
            return div().h(px(ROOM_ROW_HEIGHT)).into_any_element();
        };
        let user_hovered = self.hovered_user == Some(uid)
            || self.moving_user == Some(uid)
            || self.exchanging_user == Some(uid);
        let can_move = self.rooms.len() > 1;
        let can_exchange = total_assigned > members.len();
        let move_to_label = labels.move_to.clone();
        let exchange_label = labels.exchange.clone();
        h_flex()
            .id(("breakout-assigned-member", uid.0 as u64))
            .w_full()
            .h(px(ROOM_ROW_HEIGHT))
            .pl(px(30.))
            .pr(px(14.))
            .justify_between()
            .items_center()
            .border_b_1()
            .border_color(gpui::Hsla::from(colors.border).opacity(0.4))
            .when(user_hovered, |el| el.bg(colors.bg_hover))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if this.hovered_user != Some(uid) || this.hovered_room.is_some() {
                        this.hovered_user = Some(uid);
                        this.hovered_room = None;
                        cx.notify();
                    }
                } else if this.hovered_user == Some(uid) {
                    this.hovered_user = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(colors.text_primary)
                    .child(uname),
            )
            .when(user_hovered, |el| {
                el.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .when(can_move, |el| {
                            el.child(
                                h_flex()
                                    .id(("breakout-user-move", uid.0 as u64))
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(colors.bg_secondary)
                                    .cursor_pointer()
                                    .child(
                                        Icon::new(IconName::ArrowRight)
                                            .size(px(12.))
                                            .text_color(colors.text_secondary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.text_primary)
                                            .child(move_to_label),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_move_menu(uid, cx);
                                    })),
                            )
                        })
                        .when(can_exchange, |el| {
                            el.child(
                                h_flex()
                                    .id(("breakout-user-exchange", uid.0 as u64))
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(colors.bg_secondary)
                                    .cursor_pointer()
                                    .child(
                                        Icon::new(IconName::ConvertAccount)
                                            .size(px(12.))
                                            .text_color(colors.text_secondary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.text_primary)
                                            .child(exchange_label),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_exchange_menu(uid, cx);
                                    })),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn render_open(&self, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let danger_color = cx.theme().danger;
        let border_color = cx.theme().border;
        let pending = self.pending;
        let can_add_room = self.next_unused_idle_channel().is_some();
        let rooms_still_idle = can_open_selected_rooms(&self.rooms, &self.idle_channels);
        let can_open_rooms = can_open_breakout(
            &self.rooms,
            &self.idle_channels,
            self.participants.len(),
            self.effective_rule(),
            self.assigned_participant_count(),
        );
        let footer_warning = if self.recreating {
            None
        } else if !rooms_still_idle {
            Some(mezon_i18n::t(
                locale,
                "channelVoice.breakout.notEnoughRooms",
            ))
        } else if self.participants.is_empty() {
            Some(mezon_i18n::t(
                locale,
                "channelVoice.breakout.noParticipants",
            ))
        } else {
            None
        };
        let menu = self.render_menu(locale, cx).map(deferred);
        let recreate_overlay = self
            .recreating
            .then(|| deferred(self.render_recreate_overlay(locale, cx)));
        let delete_overlay = self.render_delete_overlay(locale, cx).map(deferred);
        let room_list = uniform_list(
            "breakout-rooms-list",
            self.room_rows.len(),
            cx.processor(|this: &mut Self, range: Range<usize>, _window, cx| {
                this.render_room_rows(range, cx)
            }),
        )
        .track_scroll(&self.rooms_scroll_handle)
        .flex_1()
        .min_h_0()
        .w_full();

        v_flex()
            .flex_1()
            .w_full()
            .justify_between()
            .child(room_list)
            .when_some(footer_warning, |el, warning| {
                el.child(
                    div()
                        .w_full()
                        .px(px(16.))
                        .py(px(6.))
                        .text_xs()
                        .text_color(danger_color)
                        .child(warning),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(border_color)
                    .justify_end()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("breakout-recreate")
                            .label(mezon_i18n::t(locale, "channelVoice.breakout.recreate"))
                            .on_click(cx.listener(|this, _, _, cx| this.begin_recreate(cx))),
                    )
                    .child(
                        Button::new("breakout-add-room")
                            .label(mezon_i18n::t(locale, "channelVoice.breakout.addRoom"))
                            .disabled(!can_add_room)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_room(cx);
                            })),
                    )
                    .child(
                        Button::new("breakout-create")
                            .label(mezon_i18n::t(locale, "channelVoice.breakout.openAllRooms"))
                            .primary()
                            .loading(pending)
                            .disabled(pending || !can_open_rooms)
                            .on_click(cx.listener(|this, _, _, cx| this.open_all_rooms(cx))),
                    ),
            )
            .children(menu)
            .children(recreate_overlay)
            .children(delete_overlay)
    }

    fn render_hosted_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = RowColors::new(cx);
        range
            .map(|ix| {
                self.hosted_rows
                    .get(ix)
                    .copied()
                    .and_then(|row| self.render_hosted_row(row, colors))
                    .unwrap_or_else(|| div().h(px(HOSTED_ROW_HEIGHT)).into_any_element())
            })
            .collect()
    }

    fn render_hosted_row(&self, row: ListRow, colors: RowColors) -> Option<AnyElement> {
        match row {
            ListRow::Room(idx) => {
                let (_, room_name, members) = self.hosted_room_statuses.get(idx)?;
                Some(
                    h_flex()
                        .id(("breakout-hosted-room", idx))
                        .w_full()
                        .h(px(HOSTED_ROW_HEIGHT))
                        .px_3()
                        .justify_between()
                        .items_center()
                        .bg(colors.bg_secondary)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    Icon::new(IconName::VoiceBreakoutIcon)
                                        .size(px(14.))
                                        .text_color(colors.text_secondary),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(colors.text_primary)
                                        .truncate()
                                        .child(room_name.clone()),
                                ),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .bg(colors.border)
                                .text_xs()
                                .text_color(colors.text_secondary)
                                .child(members.len().to_string()),
                        )
                        .into_any_element(),
                )
            }
            ListRow::Member(idx, member) => {
                let (uid, name) = self.hosted_room_statuses.get(idx)?.2.get(member)?;
                Some(
                    h_flex()
                        .id(("breakout-hosted-member", uid.0 as u64))
                        .w_full()
                        .h(px(HOSTED_ROW_HEIGHT))
                        .px_3()
                        .gap_2()
                        .items_center()
                        .child(div().size(px(8.)).rounded_full().bg(colors.online))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(colors.text_primary)
                                .child(name.clone()),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    fn render_hosted(
        &self,
        locale: &str,
        rooms: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let text_secondary = theme.tokens.text_secondary;
        let border_color = theme.border;
        let pending = self.pending;
        let voice = self.voice.clone();
        let rooms = if self.hosted_room_statuses.is_empty() {
            rooms
        } else {
            self.hosted_room_statuses.len()
        };
        let list_height =
            (self.hosted_rows.len() as f32 * HOSTED_ROW_HEIGHT).min(HOSTED_LIST_MAX_HEIGHT);
        let room_list = (!self.hosted_rows.is_empty()).then(|| {
            v_flex()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(border_color)
                .overflow_hidden()
                .child(
                    uniform_list(
                        "breakout-hosted-rooms-list",
                        self.hosted_rows.len(),
                        cx.processor(|this: &mut Self, range: Range<usize>, _window, cx| {
                            this.render_hosted_rows(range, cx)
                        }),
                    )
                    .track_scroll(&self.hosted_scroll_handle)
                    .w_full()
                    .h(px(list_height)),
                )
        });
        v_flex()
            .p(px(24.))
            .gap_4()
            .child(
                div().text_sm().text_color(text_secondary).child(
                    mezon_i18n::t(locale, "channelVoice.breakout.openRooms")
                        .replace("{{count}}", &rooms.to_string()),
                ),
            )
            .children(room_list)
            .child(
                h_flex()
                    .pt(px(8.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("breakout-cancel")
                            .label(mezon_i18n::t(locale, "common.cancel"))
                            .ghost()
                            .on_click(|_, _, cx| close(cx)),
                    )
                    .child(
                        Button::new("breakout-close")
                            .label(mezon_i18n::t(locale, "channelVoice.breakout.close"))
                            .danger()
                            .loading(pending)
                            .disabled(pending)
                            .on_click(move |_, _, cx| {
                                voice.update(cx, |store, cx| store.close_breakout_rooms(cx));
                                close(cx);
                            }),
                    ),
            )
    }
}

impl Render for BreakoutRoomsModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = locale(cx);
        if let Some(rooms) = self.hosted_rooms() {
            let theme = cx.theme();
            return modal_card(theme, self.focus_handle.clone())
                .on_action(|_: &::menu::Cancel, _, cx| close(cx))
                .child(modal_title(&locale, theme))
                .child(self.render_hosted(&locale, rooms, cx))
                .into_any_element();
        }

        let (text_primary, text_secondary, bg_hover, border_color, bg_floating, accent_color) = {
            let theme = cx.theme();
            (
                theme.tokens.text_theme_primary,
                theme.tokens.text_secondary,
                theme.tokens.bg_item_hover,
                theme.border,
                theme.bg_floating,
                theme.brand,
            )
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .relative()
            .w(px(520.))
            .h(px(460.))
            .rounded_xl()
            .border_1()
            .border_color(border_color)
            .bg(bg_floating)
            .shadow_lg()
            .on_action(cx.listener(|this, _: &::menu::Cancel, _, cx| {
                if this.assigning_room.is_some()
                    || this.deleting_room.is_some()
                    || this.moving_user.is_some()
                    || this.exchanging_user.is_some()
                {
                    this.clear_popovers();
                    cx.notify();
                } else if this.recreating && !this.initial_create {
                    this.cancel_recreate(cx);
                } else {
                    close(cx);
                }
            }))
            .child(
                h_flex()
                    .w_full()
                    .h(px(44.))
                    .px(px(16.))
                    .justify_between()
                    .items_center()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::VoiceBreakoutIcon)
                                    .size(px(18.))
                                    .text_color(accent_color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_primary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "channelVoice.breakout.notStarted",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("breakout-modal-close")
                            .size(px(28.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |s| s.bg(bg_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(14.))
                                    .text_color(text_secondary),
                            )
                            .on_click(|_, _, cx| close(cx)),
                    ),
            )
            .child(self.render_open(&locale, cx))
            .into_any_element()
    }
}

pub struct BreakoutInviteModal {
    focus_handle: FocusHandle,
    voice: Entity<VoiceStore>,
    invite: Option<BreakoutInvite>,
    room_names: Vec<Option<String>>,
    pending: bool,
    _observe: Subscription,
    _channel_observe: Option<Subscription>,
}

fn invite_room_names(invite: Option<&BreakoutInvite>, cx: &App) -> Vec<Option<String>> {
    match invite {
        Some(BreakoutInvite::Assigned { room_id }) => vec![voice_channel_name(room_id, cx)],
        Some(BreakoutInvite::Choose { rooms }) => rooms
            .iter()
            .map(|room_id| voice_channel_name(room_id, cx))
            .collect(),
        Some(BreakoutInvite::Return) | None => Vec::new(),
    }
}

impl BreakoutInviteModal {
    pub fn show(voice: Entity<VoiceStore>, cx: &mut App) {
        let view = cx.new(|cx| {
            let (invite, pending) = {
                let store = voice.read(cx);
                (store.breakout_invite().cloned(), store.breakout_pending())
            };
            let room_names = invite_room_names(invite.as_ref(), cx);
            let channel_observe = mezon_store::ChannelList::try_global(cx).map(|channels| {
                cx.subscribe(&channels, |this: &mut Self, _, event, cx| {
                    if !matches!(
                        event,
                        mezon_store::ChannelEvent::ClanChannelsLoaded(_)
                            | mezon_store::ChannelEvent::UserChannelsLoaded
                            | mezon_store::ChannelEvent::LinkedChannelResolved(_)
                    ) {
                        return;
                    }
                    let room_names = invite_room_names(this.invite.as_ref(), cx);
                    if this.room_names != room_names {
                        this.room_names = room_names;
                        cx.notify();
                    }
                })
            });
            Self {
                focus_handle: cx.focus_handle(),
                _channel_observe: channel_observe,
                _observe: cx.observe(&voice, |this: &mut Self, voice, cx| {
                    let store = voice.read(cx);
                    let pending = store.breakout_pending();
                    let Some(invite) = store.breakout_invite() else {
                        this.invite = None;
                        this.pending = pending;
                        let owner = cx.entity_id();
                        Shell::global(cx)
                            .update(cx, |shell, cx| shell.close_modal_if_current(owner, cx));
                        return;
                    };
                    if this.invite.as_ref() == Some(invite) && this.pending == pending {
                        return;
                    }
                    let invite = invite.clone();
                    this.room_names = invite_room_names(Some(&invite), cx);
                    this.invite = Some(invite);
                    this.pending = pending;
                    cx.notify();
                }),
                voice,
                invite,
                room_names,
                pending,
            }
        });
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn accept(&self, room_id: Option<String>, window: &mut Window, cx: &mut App) {
        let target = self.voice.update(cx, |store, cx| {
            store.accept_breakout_invite(room_id, window, cx)
        });
        open_voice_channel(target, cx);
    }
}

impl Render for BreakoutInviteModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = locale(cx);
        let invite = &self.invite;
        let pending = self.pending;
        let theme = cx.theme();
        let (message_key, join_key) = match &invite {
            Some(BreakoutInvite::Assigned { .. }) => (
                "channelVoice.breakout.inviteAssigned",
                Some("channelVoice.breakout.join"),
            ),
            Some(BreakoutInvite::Choose { .. }) => ("channelVoice.breakout.inviteChoose", None),
            Some(BreakoutInvite::Return) | None => (
                "channelVoice.breakout.inviteReturn",
                Some("channelVoice.breakout.returnToMain"),
            ),
        };
        let assigned_room_name = match &invite {
            Some(BreakoutInvite::Assigned { .. }) => self.room_names.first().cloned().flatten(),
            _ => None,
        };
        let rooms = match &invite {
            Some(BreakoutInvite::Choose { rooms }) => rooms.clone(),
            _ => Vec::new(),
        };
        let room_names: Vec<String> = (0..rooms.len())
            .map(|index| {
                self.room_names
                    .get(index)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| room_label(&locale, index))
            })
            .collect();
        let voice = self.voice.clone();
        let accent = theme.brand;
        let text_primary = theme.tokens.text_theme_primary;
        let bg_secondary = theme.bg_secondary;
        let border_color = theme.border;

        modal_card(theme, self.focus_handle.clone())
            .on_action(|_: &::menu::Cancel, _, cx| close(cx))
            .child(modal_title(&locale, theme))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(mezon_i18n::t(&locale, message_key)),
            )
            .when_some(assigned_room_name, |el, room_name| {
                el.child(
                    h_flex()
                        .id("breakout-assigned-room-badge")
                        .px(px(12.))
                        .py(px(8.))
                        .gap_2()
                        .items_center()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(border_color)
                        .bg(bg_secondary)
                        .child(
                            Icon::new(IconName::VoiceBreakoutIcon)
                                .size(px(15.))
                                .text_color(accent),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(room_name),
                        ),
                )
            })
            .when(!rooms.is_empty(), |el| {
                el.child(
                    h_flex()
                        .id("breakout-rooms")
                        .flex_wrap()
                        .gap_2()
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .children(rooms.into_iter().zip(room_names).enumerate().map(
                            |(index, (room_id, name))| {
                                Button::new(("breakout-room", index))
                                    .label(name)
                                    .disabled(pending)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.accept(Some(room_id.clone()), window, cx)
                                    }))
                            },
                        )),
                )
            })
            .child(
                h_flex()
                    .pt(px(8.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("breakout-later")
                            .label(mezon_i18n::t(&locale, "channelVoice.breakout.later"))
                            .ghost()
                            .on_click(move |_, _, cx| {
                                voice.update(cx, |store, cx| store.dismiss_breakout_invite(cx))
                            }),
                    )
                    .children(join_key.map(|key| {
                        Button::new("breakout-join")
                            .label(mezon_i18n::t(&locale, key))
                            .primary()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.accept(None, window, cx)),
                            )
                    })),
            )
    }
}

fn modal_card(theme: &crate::theme::Theme, focus_handle: FocusHandle) -> gpui::Div {
    v_flex()
        .track_focus(&focus_handle)
        .key_context("menu")
        .w(px(440.))
        .gap_3()
        .p(px(24.))
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg_floating)
        .shadow_lg()
}

fn voice_channel_name(channel_id: &str, cx: &App) -> Option<String> {
    let channel_id = mezon_store::ChannelId(channel_id.parse::<i64>().ok()?);
    let channels = mezon_store::ChannelList::try_global(cx)?;
    let name = channels.read(cx).linkable_channel(channel_id)?.name.clone();
    (!name.is_empty()).then_some(name)
}

pub(crate) fn open_voice_channel(target: Option<(String, String)>, cx: &mut App) {
    let Some((clan_id, channel_id)) = target else {
        return;
    };
    let (Ok(clan_id), Ok(channel_id)) = (clan_id.parse::<i64>(), channel_id.parse::<i64>()) else {
        return;
    };
    let (clan_id, channel_id) = (ClanId(clan_id), mezon_store::ChannelId(channel_id));
    mezon_store::ChannelList::global(cx).update(cx, |list, cx| list.select_channel(channel_id, cx));
    mezon_store::MessagesStore::global(cx).update(cx, |store, cx| {
        store.open_channel_in_clan(clan_id, channel_id, cx)
    });
    crate::router::navigate(
        cx,
        crate::router::Route::Channel {
            clan_id,
            channel_id,
        },
    );
}

pub(crate) fn can_open_breakout_rooms(store: &VoiceStore, cx: &App) -> bool {
    if store.external_meeting().is_some() || store.breakout_room_id().is_some() {
        return false;
    }
    let Some(clan_id) = store
        .connection()
        .connected_channel()
        .and_then(|(_, clan)| clan.parse::<i64>().ok())
        .filter(|clan| *clan != 0)
        .map(ClanId)
    else {
        return false;
    };
    mezon_store::PermissionStore::try_global(cx).is_some_and(|permissions| {
        permissions
            .read(cx)
            .check_permission(clan_id, mezon_store::PERMISSION_MANAGE_CLAN, cx)
    })
}

pub(crate) fn notice_text(
    locale: &str,
    notice: &mezon_store::BreakoutNotice,
) -> Option<SharedString> {
    Some(match notice {
        mezon_store::BreakoutNotice::Invited => return None,
        mezon_store::BreakoutNotice::Opened(_) => {
            mezon_i18n::t(locale, "channelVoice.breakout.opened").into()
        }
        mezon_store::BreakoutNotice::Closed => {
            mezon_i18n::t(locale, "channelVoice.breakout.closed").into()
        }
        mezon_store::BreakoutNotice::Failed(message) => message.clone().into(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gpui::SharedString;
    use mezon_store::{BreakoutRule, ChannelId, ClanId, UserId};

    use super::{
        BreakoutRoomItem, ListRow, MAX_BREAKOUT_ROOMS, build_list_rows, build_manual_assignments,
        can_create_room_count, can_open_breakout, can_open_selected_rooms, delete_last_room,
        group_members_by_room, is_eligible_idle_voice_channel, pick_next_unused_idle_channel,
        place_unassigned_participants, rebind_rooms_to_idle_prefix, sort_and_label_idle_channels,
        toggle_participant_room,
    };

    fn room_counts(
        room_count: u32,
        rooms_of: &HashMap<UserId, u32>,
        participants: &[(UserId, String)],
    ) -> Vec<usize> {
        build_manual_assignments(room_count, rooms_of, participants)
            .iter()
            .map(Vec::len)
            .collect()
    }

    fn rooms_from(idle: &[(ChannelId, SharedString)]) -> Vec<BreakoutRoomItem> {
        idle.iter()
            .map(|(cid, name)| BreakoutRoomItem {
                channel_id: *cid,
                name: name.clone(),
                collapsed: false,
            })
            .collect()
    }

    #[test]
    fn toggle_participant_room_moves_between_rooms_and_unassigns_when_reclicked() {
        let mut rooms_of = HashMap::new();
        let alice = UserId(101);
        let bob = UserId(202);
        let participants = vec![
            (alice, "Alice".to_string()),
            (bob, "Bob".to_string()),
            (alice, "Alice duplicate".to_string()),
        ];

        assert_eq!(room_counts(2, &rooms_of, &participants), vec![0, 0]);
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![Vec::<i64>::new(), Vec::<i64>::new()]
        );

        toggle_participant_room(&mut rooms_of, alice, 0);
        toggle_participant_room(&mut rooms_of, bob, 1);
        assert_eq!(room_counts(2, &rooms_of, &participants), vec![1, 1]);
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![vec![101], vec![202]]
        );

        toggle_participant_room(&mut rooms_of, alice, 1);
        assert_eq!(room_counts(2, &rooms_of, &participants), vec![0, 2]);
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![Vec::<i64>::new(), vec![101, 202]]
        );

        toggle_participant_room(&mut rooms_of, alice, 1);
        assert_eq!(room_counts(2, &rooms_of, &participants), vec![0, 1]);
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![Vec::<i64>::new(), vec![202]]
        );
    }

    #[test]
    fn delete_last_room_unassigns_only_its_members() {
        let idle = vec![
            (ChannelId(1001), SharedString::from("room-a")),
            (ChannelId(1002), SharedString::from("room-b")),
            (ChannelId(1003), SharedString::from("room-c")),
        ];
        let mut rooms = rooms_from(&idle);
        let alice = UserId(101);
        let bob = UserId(202);
        let carol = UserId(303);
        let participants = vec![
            (alice, "Alice".to_string()),
            (bob, "Bob".to_string()),
            (carol, "Carol".to_string()),
        ];
        let mut rooms_of = HashMap::from([(alice, 0), (bob, 1), (carol, 2)]);

        assert!(delete_last_room(
            &mut rooms,
            &mut rooms_of,
            &participants,
            false
        ));
        assert_eq!(
            rooms.iter().map(|r| r.channel_id).collect::<Vec<_>>(),
            vec![ChannelId(1001), ChannelId(1002)]
        );
        assert_eq!(rooms[1].name, SharedString::from("room-b"));
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![vec![101], vec![202]]
        );

        assert!(delete_last_room(
            &mut rooms,
            &mut rooms_of,
            &participants,
            true
        ));
        assert_eq!(
            build_manual_assignments(1, &rooms_of, &participants),
            vec![vec![101, 202, 303]]
        );
        assert!(!delete_last_room(
            &mut rooms,
            &mut rooms_of,
            &participants,
            true
        ));
        assert_eq!(rooms.len(), 1);
    }

    #[test]
    fn out_of_range_rooms_are_ignored_in_counts_and_assignments() {
        let mut rooms_of = HashMap::new();
        let alice = UserId(101);
        rooms_of.insert(alice, 2);
        let participants = vec![(alice, "Alice".to_string())];

        assert_eq!(room_counts(2, &rooms_of, &participants), vec![0, 0]);
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![Vec::<i64>::new(), Vec::<i64>::new()]
        );
    }

    #[test]
    fn validate_room_count_against_available_idle_channels() {
        assert!(!can_create_room_count(0, 2));
        assert!(!can_create_room_count(1, 0));
        assert!(can_create_room_count(1, 2));
        assert!(can_create_room_count(2, 2));
        assert!(!can_create_room_count(3, 2));
        assert!(can_create_room_count(
            MAX_BREAKOUT_ROOMS,
            MAX_BREAKOUT_ROOMS
        ));
        assert!(!can_create_room_count(
            MAX_BREAKOUT_ROOMS + 1,
            MAX_BREAKOUT_ROOMS + 5
        ));
    }

    #[test]
    fn pick_next_unused_idle_channel_and_readd_after_delete() {
        let idle = vec![
            (ChannelId(1001), SharedString::from("voice-1")),
            (ChannelId(1002), SharedString::from("voice-2")),
        ];
        let mut rooms = vec![BreakoutRoomItem {
            channel_id: ChannelId(1001),
            name: SharedString::from("voice-1"),
            collapsed: false,
        }];

        let next = pick_next_unused_idle_channel(&rooms, &idle);
        assert_eq!(next, Some((ChannelId(1002), SharedString::from("voice-2"))));

        if let Some((cid, name)) = next {
            rooms.push(BreakoutRoomItem {
                channel_id: cid,
                name,
                collapsed: false,
            });
        }
        assert_eq!(pick_next_unused_idle_channel(&rooms, &idle), None);

        rooms.remove(0);
        assert_eq!(
            pick_next_unused_idle_channel(&rooms, &idle),
            Some((ChannelId(1001), SharedString::from("voice-1")))
        );
    }

    #[test]
    fn can_open_selected_rooms_rejects_when_idle_channel_becomes_busy() {
        let rooms = vec![
            BreakoutRoomItem {
                channel_id: ChannelId(1001),
                name: SharedString::from("voice-1"),
                collapsed: false,
            },
            BreakoutRoomItem {
                channel_id: ChannelId(1002),
                name: SharedString::from("voice-2"),
                collapsed: false,
            },
        ];
        let both_idle = vec![
            (ChannelId(1001), SharedString::from("voice-1")),
            (ChannelId(1002), SharedString::from("voice-2")),
        ];
        assert!(can_open_selected_rooms(&rooms, &both_idle));

        let only_one_idle = vec![(ChannelId(1001), SharedString::from("voice-1"))];
        assert!(!can_open_selected_rooms(&rooms, &only_one_idle));

        let different_idle = vec![
            (ChannelId(1002), SharedString::from("voice-2")),
            (ChannelId(1003), SharedString::from("voice-3")),
        ];
        assert!(!can_open_selected_rooms(&rooms, &different_idle));
        assert!(!can_open_selected_rooms(&[], &both_idle));
    }

    #[test]
    fn can_open_breakout_blocks_when_participants_not_returned_or_unassigned() {
        let rooms = vec![BreakoutRoomItem {
            channel_id: ChannelId(1001),
            name: SharedString::from("bo-dev-a"),
            collapsed: false,
        }];
        let idle = vec![(ChannelId(1001), SharedString::from("bo-dev-a"))];

        assert!(!can_open_breakout(
            &rooms,
            &idle,
            0,
            BreakoutRule::Automatic,
            0
        ));
        assert!(!can_open_breakout(
            &rooms,
            &idle,
            0,
            BreakoutRule::SelfSelect,
            0
        ));
        assert!(!can_open_breakout(
            &rooms,
            &idle,
            0,
            BreakoutRule::Manual,
            0
        ));
        assert!(!can_open_breakout(
            &rooms,
            &idle,
            2,
            BreakoutRule::Manual,
            0
        ));
        assert!(can_open_breakout(&rooms, &idle, 2, BreakoutRule::Manual, 1));
        assert!(!can_open_breakout(
            &rooms,
            &idle,
            2,
            BreakoutRule::Automatic,
            0
        ));
        assert!(can_open_breakout(
            &rooms,
            &idle,
            2,
            BreakoutRule::Automatic,
            2
        ));
        assert!(can_open_breakout(
            &rooms,
            &idle,
            2,
            BreakoutRule::SelfSelect,
            0
        ));
    }

    #[test]
    fn grouped_members_and_rows_follow_room_order() {
        let alice = UserId(101);
        let bob = UserId(202);
        let charlie = UserId(303);
        let participants = vec![
            (alice, SharedString::from("Alice")),
            (bob, SharedString::from("Bob")),
            (charlie, SharedString::from("Charlie")),
        ];
        let rooms_of = HashMap::from([(alice, 1), (bob, 0), (charlie, 1)]);

        let members = group_members_by_room(2, &rooms_of, &participants);
        assert_eq!(
            members,
            vec![
                vec![(bob, SharedString::from("Bob"))],
                vec![
                    (alice, SharedString::from("Alice")),
                    (charlie, SharedString::from("Charlie")),
                ],
            ]
        );
        assert_eq!(
            build_manual_assignments(2, &rooms_of, &participants),
            vec![vec![202], vec![101, 303]]
        );
        assert_eq!(
            build_list_rows(members.iter().map(Vec::len)),
            vec![
                ListRow::Room(0),
                ListRow::Member(0, 0),
                ListRow::Room(1),
                ListRow::Member(1, 0),
                ListRow::Member(1, 1),
            ]
        );
        assert_eq!(
            build_list_rows([0, 2]),
            vec![
                ListRow::Room(0),
                ListRow::Room(1),
                ListRow::Member(1, 0),
                ListRow::Member(1, 1),
            ]
        );
    }

    #[test]
    fn breakout_manual_assign_keys_resolve_in_all_locales() {
        for locale in [
            "en", "vi", "ru", "ukr", "es", "tt", "de", "it", "pt", "jpn", "pl", "kr", "swe", "blr",
            "fr", "nl",
        ] {
            for key in [
                "channelVoice.breakout.assign",
                "channelVoice.breakout.noParticipants",
                "channelVoice.breakout.notStarted",
                "channelVoice.breakout.recreate",
                "channelVoice.breakout.addRoom",
                "channelVoice.breakout.openAllRooms",
                "channelVoice.breakout.delete",
                "channelVoice.breakout.deleteConfirm",
                "channelVoice.breakout.moveTo",
                "channelVoice.breakout.exchange",
                "channelVoice.breakout.notEnoughRooms",
            ] {
                let text = mezon_i18n::t(locale, key);
                assert_ne!(text, key, "{locale} is missing {key}");
                assert!(!text.is_empty(), "{locale} has empty {key}");
            }
        }
    }

    #[test]
    fn manual_assignment_scales_to_thousand_participants() {
        let mut rooms_of = HashMap::with_capacity(1000);
        let participants: Vec<(UserId, String)> = (0..1000_i64)
            .map(|i| {
                let uid = UserId(1_000_000 + i);
                toggle_participant_room(&mut rooms_of, uid, (i as u32) % 10);
                (uid, format!("User {i}"))
            })
            .collect();

        let counts = room_counts(10, &rooms_of, &participants);
        assert_eq!(counts, vec![100; 10]);

        let assignments = build_manual_assignments(10, &rooms_of, &participants);
        assert_eq!(assignments.len(), 10);
        assert!(assignments.iter().all(|room| room.len() == 100));
    }

    #[test]
    fn idle_voice_channel_eligibility_filters_private_archived_parent_and_occupied() {
        let parent = ChannelId(100);
        let mut ch = mezon_store::Channel {
            id: ChannelId(201),
            name: "bo-dev-a".to_string(),
            channel_type: mezon_store::ChannelType::Voice,
            private: false,
            clan_id: ClanId(1),
            clan_name: String::new(),
            category_name: String::new(),
            category_id: None,
            member_count: 0,
            badge_count: 0,
            muted: false,
            parent_id: None,
            last_seen_message_id: mezon_store::MessageId(0),
            last_seen_timestamp: 0,
            last_sent_message_id: mezon_store::MessageId(0),
            last_sent_timestamp: 0,
            voice_members: Vec::new(),
            is_favorite: false,
            creator_id: UserId(1),
            active: mezon_store::CHANNEL_ACTIVE_JOINED,
            avatar_url: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        };

        assert!(is_eligible_idle_voice_channel(&ch, parent, false));
        assert!(!is_eligible_idle_voice_channel(&ch, parent, true));
        assert!(!is_eligible_idle_voice_channel(&ch, ChannelId(201), false));

        ch.private = true;
        assert!(!is_eligible_idle_voice_channel(&ch, parent, false));
        ch.private = false;

        ch.active = mezon_store::CHANNEL_ACTIVE_ARCHIVED;
        assert!(!is_eligible_idle_voice_channel(&ch, parent, false));
        ch.active = mezon_store::CHANNEL_ACTIVE_JOINED;

        ch.channel_type = mezon_store::ChannelType::Text;
        assert!(!is_eligible_idle_voice_channel(&ch, parent, false));
        ch.channel_type = mezon_store::ChannelType::Voice;

        ch.voice_members.push(mezon_store::VoiceMember {
            user_id: UserId(0),
            display_name: String::new(),
            avatar_url: String::new(),
            sharing_screen: false,
        });
        assert!(is_eligible_idle_voice_channel(&ch, parent, false));

        ch.voice_members.push(mezon_store::VoiceMember {
            user_id: UserId(999),
            display_name: "Alice".to_string(),
            avatar_url: String::new(),
            sharing_screen: false,
        });
        assert!(!is_eligible_idle_voice_channel(&ch, parent, false));
    }

    #[test]
    fn sort_and_label_idle_channels_orders_by_channel_id() {
        let raw = vec![
            (ChannelId(300), "Room C".to_string()),
            (ChannelId(100), String::new()),
            (ChannelId(200), "Room B".to_string()),
        ];
        let sorted = sort_and_label_idle_channels(raw, "en");
        assert_eq!(
            sorted,
            vec![
                (ChannelId(100), SharedString::from("Room 1")),
                (ChannelId(200), SharedString::from("Room B")),
                (ChannelId(300), SharedString::from("Room C")),
            ]
        );
    }

    #[test]
    fn newcomers_fill_the_emptiest_rooms_without_moving_anyone() {
        let alice = UserId(1);
        let bob = UserId(2);
        let carol = UserId(3);
        let dave = UserId(4);
        let mut rooms_of = HashMap::from([(alice, 0), (bob, 0), (carol, 1)]);
        let participants = vec![
            (alice, "Alice".to_string()),
            (bob, "Bob".to_string()),
            (carol, "Carol".to_string()),
            (dave, "Dave".to_string()),
        ];

        place_unassigned_participants(&mut rooms_of, &participants, 3);
        assert_eq!(rooms_of[&alice], 0);
        assert_eq!(rooms_of[&bob], 0);
        assert_eq!(rooms_of[&carol], 1);
        assert_eq!(rooms_of[&dave], 2);

        rooms_of.insert(dave, 7);
        place_unassigned_participants(&mut rooms_of, &participants, 3);
        assert_eq!(rooms_of[&dave], 2);
        assert_eq!(room_counts(3, &rooms_of, &participants), vec![2, 1, 1]);
    }

    #[test]
    fn rooms_rebind_to_the_idle_prefix_and_keep_their_groups() {
        let idle = [
            (ChannelId(1001), SharedString::from("room-a")),
            (ChannelId(1002), SharedString::from("room-b")),
            (ChannelId(1003), SharedString::from("room-c")),
        ];
        let mut rooms = rooms_from(&idle[..2]);
        let busy_b = vec![idle[0].clone(), idle[2].clone()];

        rebind_rooms_to_idle_prefix(&mut rooms, &busy_b);
        assert_eq!(
            rooms.iter().map(|r| r.channel_id).collect::<Vec<_>>(),
            vec![ChannelId(1001), ChannelId(1003)]
        );
        assert_eq!(rooms[1].name, SharedString::from("room-c"));
        assert!(can_open_selected_rooms(&rooms, &busy_b));

        rebind_rooms_to_idle_prefix(&mut rooms, &idle[..1]);
        assert_eq!(
            rooms.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
            vec![SharedString::from("room-a"), SharedString::from("room-c")]
        );
        assert!(!can_open_selected_rooms(&rooms, &idle[..1]));
    }
}
