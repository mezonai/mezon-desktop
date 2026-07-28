use std::sync::Arc;

use chrono::NaiveDate;
use gpui::{App, AppContext, Context, Entity, Global, Task};
use mezon_client::AppApi;
use mezon_proto::api;

use crate::cache::KeyedCache;
use crate::ids::{ChannelId, ClanId, UserId};

pub const AUDIT_LOG_DATE_FMT: &str = "%d-%m-%Y";
pub const ALL_ACTION_INDEX: usize = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLogEntry {
    pub id: i64,
    pub user_id: UserId,
    pub action_log: String,
    pub entity_name: String,
    pub entity_id: i64,
    pub details: String,
    pub time_log_seconds: u32,
    pub channel_id: ChannelId,
    pub channel_label: String,
}

impl AuditLogEntry {
    pub fn from_proto(entry: api::AuditLog) -> Self {
        Self {
            id: entry.id,
            user_id: UserId(entry.user_id),
            action_log: entry.action_log,
            entity_name: entry.entity_name,
            entity_id: entry.entity_id,
            details: entry.details,
            time_log_seconds: entry.time_log_seconds,
            channel_id: ChannelId(entry.channel_id),
            channel_label: entry.channel_label,
        }
    }
}

pub struct AuditActionOption {
    pub i18n_key: &'static str,
    pub action_key: &'static str,
}

pub const AUDIT_ACTION_OPTIONS: &[AuditActionOption] = &[
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.allActions",
        action_key: "",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateClan",
        action_key: "UPDATE_CLAN_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createChannel",
        action_key: "CREATE_CHANNEL_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateChannel",
        action_key: "UPDATE_CHANNEL_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateChannelPrivate",
        action_key: "UPDATE_CHANNEL_PRIVATE_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteChannel",
        action_key: "DELETE_CHANNE_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createChannelPermission",
        action_key: "CREATE_CHANNEL_PERMISSION_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateChannelPermission",
        action_key: "UPDATE_CHANNEL_PERMISSION_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteChannelPermission",
        action_key: "DELETE_CHANNEL_PERMISSION_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.kickMember",
        action_key: "KICK_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.pruneMember",
        action_key: "PRUNE_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.banMember",
        action_key: "BAN_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.unbanMember",
        action_key: "UNBAN_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateMember",
        action_key: "UPDATE_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateRolesMember",
        action_key: "UPDATE_ROLES_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.moveMember",
        action_key: "MOVE_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.disconnectMember",
        action_key: "DISCONNECT_MEMBER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.addBot",
        action_key: "ADD_BOT_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createThread",
        action_key: "CREATE_THREAD_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateThread",
        action_key: "UPDATE_THREAD_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteThread",
        action_key: "DELETE_THREAD_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createRole",
        action_key: "CREATE_ROLE_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateRole",
        action_key: "UPDATE_ROLE_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteRole",
        action_key: "DELETE_ROLE_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createWebhook",
        action_key: "CREATE_WEBHOOK_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateWebhook",
        action_key: "UPDATE_WEBHOOK_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteWebhook",
        action_key: "DELETE_WEBHOOK_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createEmoji",
        action_key: "CREATE_EMOJI_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateEmoji",
        action_key: "UPDATE_EMOJI_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteEmoji",
        action_key: "DELETE_EMOJI_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createSticker",
        action_key: "CREATE_STICKER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateSticker",
        action_key: "UPDATE_STICKER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteSticker",
        action_key: "DELETE_STICKER_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createEvent",
        action_key: "CREATE_EVENT_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateEvent",
        action_key: "UPDATE_EVENT_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteEvent",
        action_key: "DELETE_EVENT_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createCanvas",
        action_key: "CREATE_CANVAS_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateCanvas",
        action_key: "UPDATE_CANVAS_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteCanvas",
        action_key: "DELETE_CANVAS_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.createCategory",
        action_key: "CREATE_CATEGORY_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.updateCategory",
        action_key: "UPDATE_CATEGORY_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.deleteCategory",
        action_key: "DELETE_CATEGORY_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.addMemberChannel",
        action_key: "ADD_MEMBER_CHANNEL_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.removeMemberChannel",
        action_key: "REMOVE_MEMBER_CHANNEL_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.addRoleChannel",
        action_key: "ADD_ROLE_CHANNEL_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.removeRoleChannel",
        action_key: "REMOVE_ROLE_CHANNEL_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.addMemberThread",
        action_key: "ADD_MEMBER_THREAD_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.removeMemberThread",
        action_key: "REMOVE_MEMBER_THREAD_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.addRoleThread",
        action_key: "ADD_ROLE_THREAD_ACTION_AUDIT",
    },
    AuditActionOption {
        i18n_key: "auditLogSearch.actions.removeRoleThread",
        action_key: "REMOVE_ROLE_THREAD_ACTION_AUDIT",
    },
];

fn title_case_audit_word(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest: String = chars.flat_map(|c| c.to_lowercase()).collect();
    format!("{first}{rest}")
}

pub fn audit_action_api_value_from_key(action_key: &str) -> String {
    if action_key.is_empty() || action_key == "ALL_ACTION_AUDIT" {
        return String::new();
    }
    match action_key {
        "UPDATE_CHANNEL_PRIVATE_ACTION_AUDIT" => "Update Channel private".into(),
        "DELETE_CHANNE_ACTION_AUDIT" => "Delete Channel".into(),
        _ => {
            let stem = action_key
                .strip_suffix("_ACTION_AUDIT")
                .unwrap_or(action_key);
            stem.split('_')
                .map(title_case_audit_word)
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

pub fn audit_action_api_value(index: usize) -> String {
    AUDIT_ACTION_OPTIONS
        .get(index)
        .map(|option| audit_action_api_value_from_key(option.action_key))
        .unwrap_or_default()
}

pub fn audit_action_i18n_key(index: usize) -> &'static str {
    AUDIT_ACTION_OPTIONS
        .get(index)
        .map(|option| option.i18n_key)
        .unwrap_or("auditLogSearch.actions.allActions")
}

pub fn audit_action_index_for_api_log(action_log: &str) -> Option<usize> {
    let normalized = action_log.trim();
    AUDIT_ACTION_OPTIONS
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, option)| {
            audit_action_api_value_from_key(option.action_key)
                .eq_ignore_ascii_case(normalized)
                .then_some(index)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLogQuery {
    pub action_index: usize,
    pub user_id: Option<UserId>,
    pub date: NaiveDate,
}

#[derive(Debug, Clone, Default)]
pub struct AuditLogState {
    pub logs: Vec<AuditLogEntry>,
    pub loading: bool,
    pub fetch_failed: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditLogStateView<'a> {
    pub logs: &'a [AuditLogEntry],
    pub loading: bool,
    pub fetch_failed: bool,
    pub generation: u64,
}

pub struct AuditLogStore {
    states: KeyedCache<ClanId, AuditLogState>,
    api: Arc<AppApi>,
    fetch_generation: u64,
    _fetch_task: Task<()>,
}

struct GlobalAuditLogStore(Entity<AuditLogStore>);
impl Global for GlobalAuditLogStore {}

impl AuditLogStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            states: KeyedCache::new(Some(8)),
            api,
            fetch_generation: 0,
            _fetch_task: Task::ready(()),
        });
        cx.set_global(GlobalAuditLogStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAuditLogStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAuditLogStore>()
            .map(|global| global.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        self.states = KeyedCache::new(Some(8));
        self._fetch_task = Task::ready(());
        cx.notify();
    }

    pub fn state(&self, clan_id: ClanId) -> AuditLogStateView<'_> {
        self.states
            .get(&clan_id)
            .map(|state| AuditLogStateView {
                logs: &state.logs,
                loading: state.loading,
                fetch_failed: state.fetch_failed,
                generation: state.generation,
            })
            .unwrap_or_default()
    }

    pub fn logs(&self, clan_id: ClanId) -> &[AuditLogEntry] {
        self.states
            .get(&clan_id)
            .map(|state| state.logs.as_slice())
            .unwrap_or(&[])
    }

    pub fn cancel(&mut self, clan_id: ClanId) {
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        if let Some(state) = self.states.get_mut(&clan_id) {
            state.loading = false;
            state.generation = self.fetch_generation;
        }
    }

    pub fn fetch(&mut self, clan_id: ClanId, query: AuditLogQuery, cx: &mut Context<Self>) {
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;

        if !self.states.contains(&clan_id) {
            self.states.insert(clan_id, AuditLogState::default(), None);
        }
        if let Some(state) = self.states.get_mut(&clan_id) {
            state.loading = true;
            state.fetch_failed = false;
            state.generation = generation;
        }
        cx.notify();

        let api = self.api.clone();
        let clan_id_raw = clan_id.get();
        let action_log = audit_action_api_value(query.action_index);
        let date_log = query.date.format(AUDIT_LOG_DATE_FMT).to_string();
        let has_action = !action_log.is_empty();
        let has_user = query.user_id.is_some_and(|id| !id.is_zero());
        let api_user_id = if has_action && has_user {
            None
        } else {
            query.user_id.filter(|id| !id.is_zero()).map(UserId::get)
        };
        let client_user_id = if has_action && has_user {
            query.user_id.filter(|id| !id.is_zero())
        } else {
            None
        };

        self._fetch_task = cx.spawn(async move |this, cx| {
            let result = api
                .list_audit_log(clan_id_raw, &action_log, api_user_id, &date_log)
                .await
                .map(|response| {
                    let mut logs: Vec<AuditLogEntry> = response
                        .logs
                        .into_iter()
                        .map(AuditLogEntry::from_proto)
                        .collect();
                    if let Some(user_id) = client_user_id {
                        logs.retain(|entry| entry.user_id == user_id);
                    }
                    logs.sort_by_key(|entry| std::cmp::Reverse(entry.time_log_seconds));
                    logs
                });

            if this
                .update(cx, |this, cx| {
                    let Some(state) = this.states.get_mut(&clan_id) else {
                        return;
                    };
                    if state.generation != generation {
                        return;
                    }
                    state.loading = false;
                    match result {
                        Ok(logs) => {
                            state.logs = logs;
                            state.fetch_failed = false;
                        }
                        Err(error) => {
                            tracing::error!(?error, %clan_id, "audit log fetch failed");
                            state.logs.clear();
                            state.fetch_failed = true;
                        }
                    }
                    cx.notify();
                })
                .is_err()
            {
                tracing::warn!("AuditLogStore dropped before fetch completed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_action_api_value_empty_for_all_actions() {
        assert_eq!(audit_action_api_value_from_key(""), "");
        assert_eq!(audit_action_api_value_from_key("ALL_ACTION_AUDIT"), "");
        assert_eq!(audit_action_api_value(ALL_ACTION_INDEX), "");
    }

    #[test]
    fn audit_action_api_value_overrides() {
        assert_eq!(
            audit_action_api_value_from_key("UPDATE_CHANNEL_PRIVATE_ACTION_AUDIT"),
            "Update Channel private"
        );
        assert_eq!(
            audit_action_api_value_from_key("DELETE_CHANNE_ACTION_AUDIT"),
            "Delete Channel"
        );
    }

    #[test]
    fn audit_action_api_value_default_stem_conversion() {
        assert_eq!(
            audit_action_api_value_from_key("CREATE_CHANNEL_ACTION_AUDIT"),
            "Create Channel"
        );
        assert_eq!(
            audit_action_api_value_from_key("KICK_MEMBER_ACTION_AUDIT"),
            "Kick Member"
        );
    }

    #[test]
    fn audit_action_index_for_api_log_round_trip() {
        let index = 2;
        let api_value = audit_action_api_value(index);
        assert_eq!(audit_action_index_for_api_log(&api_value), Some(index));
        assert_eq!(
            audit_action_index_for_api_log("Update Channel private"),
            Some(4)
        );
        assert_eq!(audit_action_index_for_api_log("Delete Channel"), Some(5));
    }
}
