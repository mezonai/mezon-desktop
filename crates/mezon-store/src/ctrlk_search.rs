use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::AppApi;

use crate::ids::{ChannelId, ClanId, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CtrlKSearchType {
    #[default]
    Both = 0,
    Users = 1,
    Channels = 2,
}

impl CtrlKSearchType {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Users,
            2 => Self::Channels,
            _ => Self::Both,
        }
    }

    pub fn as_raw(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CtrlKUser {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
    pub nicknames: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CtrlKChannel {
    pub clan_id: ClanId,
    pub parent_id: Option<ChannelId>,
    pub channel_id: ChannelId,
    pub category_id: Option<ChannelId>,
    pub channel_type: i32,
    pub label: String,
    pub private: bool,
    pub avatar: String,
    pub topic: String,
    pub age_restricted: i32,
    pub e2ee: i32,
}

#[derive(Debug, Clone, Default)]
pub struct CtrlKSearchState {
    pub users: Vec<CtrlKUser>,
    pub channels: Vec<CtrlKChannel>,
    pub is_searching: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlKSearchEvent {
    Changed,
}

pub struct CtrlKSearchStore {
    state: CtrlKSearchState,
    api: Arc<AppApi>,
    search_generation: u64,
    last_response_query: Option<String>,
    _search_task: Task<()>,
}

struct GlobalCtrlKSearchStore(Entity<CtrlKSearchStore>);
impl Global for GlobalCtrlKSearchStore {}

impl EventEmitter<CtrlKSearchEvent> for CtrlKSearchStore {}

impl CtrlKSearchStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            state: CtrlKSearchState::default(),
            api,
            search_generation: 0,
            last_response_query: None,
            _search_task: Task::ready(()),
        });
        cx.set_global(GlobalCtrlKSearchStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCtrlKSearchStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalCtrlKSearchStore>()
            .map(|g| g.0.clone())
    }

    pub fn state(&self) -> &CtrlKSearchState {
        &self.state
    }

    pub fn has_settled_response(&self) -> bool {
        self.last_response_query.is_some()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.cancel_pending();
        self.state = CtrlKSearchState::default();
        self.last_response_query = None;
        cx.emit(CtrlKSearchEvent::Changed);
        cx.notify();
    }

    pub fn search(&mut self, query: String, search_type: CtrlKSearchType, cx: &mut Context<Self>) {
        let trimmed = query.trim().to_string();
        if trimmed.is_empty() {
            self.clear(cx);
            return;
        }

        self.cancel_pending();
        mark_search_in_flight(&mut self.state);

        let api = self.api.clone();
        let store_generation = self.search_generation;
        self._search_task = cx.spawn(async move |this, cx| {
            let result = api.search_ctrl_k(&trimmed, search_type.as_raw()).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != store_generation {
                    return;
                }
                this.state.is_searching = false;
                match result {
                    Ok(response) => {
                        this.state.users = map_users(response.users);
                        this.state.channels = map_channels(response.channels);
                    }
                    Err(err) => {
                        tracing::warn!("SearchCtrlK failed: {err}");
                        this.state.users.clear();
                        this.state.channels.clear();
                    }
                }
                this.last_response_query = Some(trimmed);
                cx.emit(CtrlKSearchEvent::Changed);
                cx.notify();
            });
        });
    }

    fn cancel_pending(&mut self) {
        self.search_generation = self.search_generation.wrapping_add(1);
    }
}

fn mark_search_in_flight(state: &mut CtrlKSearchState) {
    state.is_searching = true;
}

fn map_users(users: Vec<mezon_proto::api::User>) -> Vec<CtrlKUser> {
    users
        .into_iter()
        .filter_map(|user| {
            if user.id == 0 {
                return None;
            }
            Some(CtrlKUser {
                id: UserId(user.id),
                username: user.username,
                display_name: user.display_name,
                avatar_url: user.avatar_url,
                nicknames: user.list_nick_names,
            })
        })
        .collect()
}

fn map_channels(channels: Vec<mezon_proto::api::ChannelDescription>) -> Vec<CtrlKChannel> {
    channels
        .into_iter()
        .filter_map(|channel| {
            if channel.channel_id == 0 {
                return None;
            }
            Some(CtrlKChannel {
                clan_id: ClanId(channel.clan_id),
                parent_id: (channel.parent_id != 0).then_some(ChannelId(channel.parent_id)),
                channel_id: ChannelId(channel.channel_id),
                category_id: (channel.category_id != 0).then_some(ChannelId(channel.category_id)),
                channel_type: channel.r#type,
                label: channel.channel_label,
                private: channel.channel_private != 0,
                avatar: channel.channel_avatar,
                topic: channel.topic,
                age_restricted: channel.age_restricted,
                e2ee: channel.e2ee,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_type_from_raw() {
        assert_eq!(CtrlKSearchType::from_raw(0), CtrlKSearchType::Both);
        assert_eq!(CtrlKSearchType::from_raw(1), CtrlKSearchType::Users);
        assert_eq!(CtrlKSearchType::from_raw(2), CtrlKSearchType::Channels);
        assert_eq!(CtrlKSearchType::from_raw(99), CtrlKSearchType::Both);
    }

    #[test]
    fn map_users_skips_zero_id() {
        let users = map_users(vec![
            mezon_proto::api::User {
                id: 0,
                username: "bad".into(),
                ..Default::default()
            },
            mezon_proto::api::User {
                id: 42,
                username: "good".into(),
                display_name: "Good".into(),
                list_nick_names: vec!["nick".into()],
                ..Default::default()
            },
        ]);
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].id, UserId(42));
        assert_eq!(users[0].nicknames, vec!["nick".to_string()]);
    }

    #[test]
    fn beginning_a_search_keeps_previous_hits() {
        let mut state = CtrlKSearchState {
            users: vec![CtrlKUser {
                id: UserId(7),
                username: "gia".into(),
                ..Default::default()
            }],
            channels: vec![CtrlKChannel {
                channel_id: ChannelId(3),
                label: "general".into(),
                ..Default::default()
            }],
            is_searching: false,
        };
        mark_search_in_flight(&mut state);
        assert!(state.is_searching);
        assert_eq!(state.users.len(), 1);
        assert_eq!(state.channels.len(), 1);
        assert_eq!(state.users[0].id, UserId(7));
        assert_eq!(state.channels[0].channel_id, ChannelId(3));
    }

    #[test]
    fn map_channels_skips_zero_id() {
        let channels = map_channels(vec![
            mezon_proto::api::ChannelDescription {
                channel_id: 0,
                channel_label: "bad".into(),
                ..Default::default()
            },
            mezon_proto::api::ChannelDescription {
                clan_id: 1,
                channel_id: 9,
                channel_label: "general".into(),
                r#type: 1,
                ..Default::default()
            },
        ]);
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].channel_id, ChannelId(9));
        assert_eq!(channels[0].clan_id, ClanId(1));
        assert!(!channels[0].private);
    }
}
