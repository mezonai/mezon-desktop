use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};
use mezon_client::AppApi;
use mezon_client::{build_clan_channel_content_search, build_direct_content_search};
use mezon_proto::api::SearchMessageDocument;

use crate::AppConfig;
use crate::cache::KeyedCache;
use crate::ids::{ChannelId, ClanId, MessageId};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Default)]
pub struct ChannelSearchState {
    pub query: String,
    pub results: Vec<SearchHit>,
    pub total: i32,
    pub is_searching: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub message_id: MessageId,
    pub channel_id: ChannelId,
    pub clan_id: ClanId,
    pub channel_type: i32,
    pub sender_name: SharedString,
    pub sender_username: SharedString,
    pub avatar_url: SharedString,
    pub avatar_proxied: SharedString,
    pub content_preview: SharedString,
    pub channel_label: SharedString,
    pub create_time: i64,
}

pub struct MessageSearchStore {
    states: KeyedCache<ChannelId, ChannelSearchState>,
    api: Arc<AppApi>,
    search_generation: u64,
    _search_task: Task<()>,
}

struct GlobalMessageSearchStore(Entity<MessageSearchStore>);
impl Global for GlobalMessageSearchStore {}

impl MessageSearchStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            states: KeyedCache::new(Some(32)),
            api,
            search_generation: 0,
            _search_task: Task::ready(()),
        });
        cx.set_global(GlobalMessageSearchStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalMessageSearchStore>().0.clone()
    }

    pub fn state(&self, channel_id: ChannelId) -> ChannelSearchState {
        self.states.get(&channel_id).cloned().unwrap_or_default()
    }

    pub fn clear_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.states
            .insert(channel_id, ChannelSearchState::default(), None);
        self.search_generation = self.search_generation.wrapping_add(1);
        cx.notify();
    }

    pub fn search(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        is_direct: bool,
        query: String,
        cx: &mut Context<Self>,
    ) {
        let trimmed = query.trim().to_string();
        if trimmed.is_empty() {
            self.clear_channel(channel_id, cx);
            return;
        }

        let mut state = self.states.get(&channel_id).cloned().unwrap_or_default();
        state.query = trimmed.clone();
        state.is_searching = true;
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        self.states.insert(channel_id, state, None);
        cx.notify();

        self.search_generation = self.search_generation.wrapping_add(1);
        let store_generation = self.search_generation;
        let api = self.api.clone();

        self._search_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            if this
                .update(cx, |this, _| this.search_generation != store_generation)
                .unwrap_or(true)
            {
                return;
            }

            let request = if is_direct {
                build_direct_content_search(channel_id.get(), &trimmed, 1)
            } else {
                build_clan_channel_content_search(channel_id.get(), clan_id.get(), &trimmed, 1)
            };

            let result = api.search_message(request).await;

            let _ = this.update(cx, |this, cx| {
                let cfg = AppConfig::try_global(cx);
                let Some(state) = this.states.get_mut(&channel_id) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                if state.query.trim() != trimmed {
                    state.is_searching = false;
                    cx.notify();
                    return;
                }
                state.is_searching = false;
                match result {
                    Ok(response) => {
                        state.total = response.total;
                        state.results = response
                            .messages
                            .iter()
                            .filter_map(|doc| search_hit_from_document(doc, cfg))
                            .collect();
                    }
                    Err(err) => {
                        tracing::warn!("search_message failed: {err}");
                        state.total = 0;
                        state.results.clear();
                    }
                }
                cx.notify();
            });
        });
    }
}

pub fn search_hit_from_document(
    doc: &SearchMessageDocument,
    cfg: Option<&AppConfig>,
) -> Option<SearchHit> {
    let message_id = raw_to_message_id(&doc.message_id)?;
    let channel_id = raw_to_channel_id(&doc.channel_id)?;
    let clan_id = raw_to_clan_id(&doc.clan_id).unwrap_or(ClanId(0));
    let create_time = doc.create_time.parse::<i64>().unwrap_or(0);
    let sender_name = if !doc.display_name.is_empty() {
        doc.display_name.clone()
    } else if !doc.username.is_empty() {
        doc.username.clone()
    } else {
        doc.sender_id.clone()
    };
    let avatar_url = doc.avatar_url.clone();
    let avatar_proxied = cfg
        .map(|c| c.avatar_proxy(&avatar_url))
        .unwrap_or_else(|| avatar_url.clone());

    Some(SearchHit {
        message_id,
        channel_id,
        clan_id,
        channel_type: doc.channel_type,
        sender_name: SharedString::from(sender_name),
        sender_username: SharedString::from(doc.username.clone()),
        avatar_url: SharedString::from(avatar_url),
        avatar_proxied: SharedString::from(avatar_proxied),
        content_preview: SharedString::from(content_preview_from_raw(&doc.content)),
        channel_label: SharedString::from(doc.channel_label.clone()),
        create_time,
    })
}

fn raw_to_message_id(raw: &str) -> Option<MessageId> {
    let id = raw.parse::<i64>().ok()?;
    (id != 0).then_some(MessageId(id))
}

fn raw_to_channel_id(raw: &str) -> Option<ChannelId> {
    let id = raw.parse::<i64>().ok()?;
    (id != 0).then_some(ChannelId(id))
}

fn raw_to_clan_id(raw: &str) -> Option<ClanId> {
    raw.parse::<i64>().ok().map(ClanId)
}

fn content_preview_from_raw(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw)
        && let Some(text) = value.get("t").and_then(|v| v.as_str())
    {
        return text.to_string();
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_proto::api::SearchMessageDocument;

    #[test]
    fn content_preview_reads_json_text_field() {
        assert_eq!(
            content_preview_from_raw(r#"{"t":"hello world"}"#),
            "hello world"
        );
    }

    #[test]
    fn content_preview_falls_back_to_raw_string() {
        assert_eq!(content_preview_from_raw("plain"), "plain");
    }

    #[test]
    fn search_hit_skips_invalid_message_id() {
        let doc = SearchMessageDocument {
            message_id: "0".into(),
            channel_id: "42".into(),
            ..Default::default()
        };
        assert!(search_hit_from_document(&doc, None).is_none());
    }

    #[test]
    fn search_hit_maps_document_fields() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            clan_id: "7".into(),
            content: r#"{"t":"find me"}"#.into(),
            display_name: "Alice".into(),
            username: "alice".into(),
            avatar_url: "https://cdn/a.png".into(),
            channel_label: "general".into(),
            channel_type: 1,
            create_time: "1700000000".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert_eq!(hit.message_id, MessageId(100));
        assert_eq!(hit.channel_id, ChannelId(200));
        assert_eq!(hit.clan_id, ClanId(7));
        assert_eq!(hit.content_preview.as_ref(), "find me");
        assert_eq!(hit.sender_name.as_ref(), "Alice");
    }
}
