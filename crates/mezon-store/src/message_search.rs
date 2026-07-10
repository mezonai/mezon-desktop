use std::sync::Arc;

use chrono::NaiveDate;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Task};
use mezon_client::AppApi;
use mezon_client::{
    build_clan_channel_content_search, build_direct_content_search, parse_search_attachment_field,
    search_page_count,
};
use mezon_proto::api::SearchMessageDocument;

use crate::AppConfig;
use crate::cache::KeyedCache;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::message::MessageAttachment;
use crate::message_time::{format_local_time_hhmm, local_datetime};

#[derive(Debug, Clone, Default)]
pub struct ChannelSearchState {
    pub query: String,
    pub results: Vec<SearchHit>,
    pub total: i32,
    pub current_page: i32,
    pub is_searching: bool,
    pub has_error: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHitImage {
    pub proxied_src: SharedString,
    pub display_width: f32,
    pub display_height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub message_id: MessageId,
    pub channel_id: ChannelId,
    pub clan_id: ClanId,
    pub channel_type: i32,
    pub sender_id: Option<UserId>,
    pub sender_name: SharedString,
    pub sender_username: SharedString,
    pub avatar_url: SharedString,
    pub avatar_proxied: SharedString,
    pub content_preview: SharedString,
    pub channel_label: SharedString,
    pub create_time_seconds: i64,
    pub time_hhmm: SharedString,
    pub local_date: Option<NaiveDate>,
    pub image_attachment: Option<SearchHitImage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageSearchEvent {
    SearchFailed,
}

pub struct MessageSearchStore {
    states: KeyedCache<ChannelId, ChannelSearchState>,
    api: Arc<AppApi>,
    search_generation: u64,
    _search_task: Task<()>,
}

struct GlobalMessageSearchStore(Entity<MessageSearchStore>);
impl Global for GlobalMessageSearchStore {}

impl EventEmitter<MessageSearchEvent> for MessageSearchStore {}

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

    pub fn cancel_pending(&mut self) {
        self.search_generation = self.search_generation.wrapping_add(1);
    }

    pub fn clear_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.cancel_pending();
        self.states
            .insert(channel_id, ChannelSearchState::default(), None);
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
        state.query = trimmed;
        state.current_page = 1;
        state.is_searching = true;
        state.has_error = false;
        state.results.clear();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        self.states.insert(channel_id, state, None);
        cx.notify();

        self.run_search(channel_id, clan_id, is_direct, 1, generation, cx);
    }

    pub fn set_page(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        is_direct: bool,
        page: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(mut state) = self.states.get(&channel_id).cloned() else {
            return;
        };
        if state.query.trim().is_empty() {
            return;
        }
        let page_count = search_page_count(state.total);
        if page < 1 || page > page_count {
            return;
        }
        if state.current_page == page {
            return;
        }

        state.current_page = page;
        state.is_searching = true;
        state.has_error = false;
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        self.states.insert(channel_id, state, None);
        cx.notify();

        self.run_search(channel_id, clan_id, is_direct, page, generation, cx);
    }

    fn run_search(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        is_direct: bool,
        page: i32,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        self.cancel_pending();
        let store_generation = self.search_generation;
        let api = self.api.clone();
        let query = self
            .states
            .get(&channel_id)
            .map(|state| state.query.clone())
            .unwrap_or_default();

        self._search_task = cx.spawn(async move |this, cx| {
            if this
                .update(cx, |this, _| this.search_generation != store_generation)
                .unwrap_or(true)
            {
                return;
            }

            let request = if is_direct {
                build_direct_content_search(channel_id.get(), &query, page)
            } else {
                build_clan_channel_content_search(channel_id.get(), clan_id.get(), &query, page)
            };

            let result = api.search_message(request).await;

            let _ = this.update(cx, |this, cx| {
                let cfg = AppConfig::try_global(cx);
                let Some(state) = this.states.get_mut(&channel_id) else {
                    return;
                };
                if !search_response_matches(state, generation, &query, page) {
                    if state.generation == generation {
                        state.is_searching = false;
                        cx.notify();
                    }
                    return;
                }
                state.is_searching = false;
                match result {
                    Ok(response) => {
                        state.has_error = false;
                        state.total = response.total;
                        state.current_page = page;
                        state.results = response
                            .messages
                            .iter()
                            .filter_map(|doc| search_hit_from_document(doc, cfg))
                            .collect();
                    }
                    Err(err) => {
                        tracing::warn!("search_message failed: {err}");
                        state.has_error = true;
                        state.total = 0;
                        state.results.clear();
                        cx.emit(MessageSearchEvent::SearchFailed);
                    }
                }
                cx.notify();
            });
        });
    }
}

pub(crate) fn search_response_matches(
    state: &ChannelSearchState,
    generation: u64,
    query: &str,
    page: i32,
) -> bool {
    state.generation == generation
        && state.query.trim() == query.trim()
        && state.current_page == page
}

pub fn search_hit_from_document(
    doc: &SearchMessageDocument,
    cfg: Option<&AppConfig>,
) -> Option<SearchHit> {
    let message_id = raw_to_message_id(&doc.message_id)?;
    let channel_id = raw_to_channel_id(&doc.channel_id)?;
    let clan_id = raw_to_clan_id(&doc.clan_id).unwrap_or(ClanId(0));
    let create_time_seconds = parse_create_time_seconds(&doc.create_time);
    let local_date = local_datetime(create_time_seconds).map(|dt| dt.date_naive());
    let time_hhmm = format_local_time_hhmm(create_time_seconds).into();
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
    let image_attachment = first_search_media(&doc.attachments, cfg);
    let sender_id = raw_to_user_id(&doc.sender_id);

    Some(SearchHit {
        message_id,
        channel_id,
        clan_id,
        channel_type: doc.channel_type,
        sender_id,
        sender_name: SharedString::from(sender_name),
        sender_username: SharedString::from(doc.username.clone()),
        avatar_url: SharedString::from(avatar_url),
        avatar_proxied: SharedString::from(avatar_proxied),
        content_preview: SharedString::from(content_preview_from_raw(&doc.content)),
        channel_label: SharedString::from(doc.channel_label.clone()),
        create_time_seconds,
        time_hhmm,
        local_date,
        image_attachment,
    })
}

fn first_search_media(raw: &str, cfg: Option<&AppConfig>) -> Option<SearchHitImage> {
    parse_search_attachment_field(raw)
        .into_iter()
        .map(|api| MessageAttachment::from_api(api, cfg))
        .find(|att| att.is_image() || att.is_video())
        .map(|att| {
            let proxied_src = if att.is_video() && !att.thumbnail_proxied.is_empty() {
                att.thumbnail_proxied.clone()
            } else {
                att.proxied_src
            };
            let (display_width, display_height) =
                if att.display_width > 0. && att.display_height > 0. {
                    (att.display_width.min(280.), att.display_height.min(200.))
                } else {
                    (200., 150.)
                };
            SearchHitImage {
                proxied_src,
                display_width,
                display_height,
            }
        })
}

fn parse_create_time_seconds(raw: &str) -> i64 {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return 0;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        if value > 1_000_000_000_000 {
            return value / 1000;
        }
        return value;
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        let secs = value as i64;
        if secs > 1_000_000_000_000 {
            return secs / 1000;
        }
        return secs;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return dt.timestamp();
    }
    chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.fZ")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S"))
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
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

fn raw_to_user_id(raw: &str) -> Option<UserId> {
    let id = raw.parse::<i64>().ok()?;
    (id != 0).then_some(UserId(id))
}

fn content_preview_from_raw(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(text) = extract_message_text_content(trimmed) {
        let text = text.trim();
        if !text.is_empty() {
            return crate::message::reply_preview_line(text);
        }
        return String::new();
    }
    if trimmed.starts_with('{') {
        return String::new();
    }
    crate::message::reply_preview_line(trimmed)
}

fn extract_message_text_content(trimmed: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return value
            .get("t")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    extract_json_t_field_loose(trimmed)
}

fn extract_json_t_field_loose(raw: &str) -> Option<String> {
    const MARKERS: &[&str] = &[r#"{"t":""#, r#"{"t": ""#, r#"{"t" : ""#];
    for marker in MARKERS {
        if let Some(pos) = raw.find(marker) {
            let text = parse_json_string_tail(&raw[pos + marker.len()..]);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn parse_json_string_tail(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    match next {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        other => {
                            out.push('\\');
                            out.push(other);
                        }
                    }
                }
            }
            '"' => break,
            other => out.push(other),
        }
    }
    out
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
    fn content_preview_ignores_metadata_only_json() {
        assert_eq!(content_preview_from_raw(r#"{"presign_finish":[]}"#), "");
    }

    #[test]
    fn content_preview_ignores_json_with_empty_text_field() {
        assert_eq!(content_preview_from_raw(r#"{"t":""}"#), "");
    }

    #[test]
    fn content_preview_extracts_truncated_json_text() {
        assert_eq!(
            content_preview_from_raw(r#"{"t":"*daily Yesterday: Add pagination"#),
            "*daily Yesterday: Add pagination"
        );
    }

    #[test]
    fn content_preview_hides_unparseable_json_without_text() {
        assert_eq!(content_preview_from_raw(r#"{"broken":true"#), "");
    }

    #[test]
    fn parse_create_time_accepts_unix_seconds_string() {
        assert_eq!(parse_create_time_seconds("1700000000"), 1_700_000_000);
    }

    #[test]
    fn parse_create_time_accepts_iso_string() {
        assert_eq!(
            parse_create_time_seconds("2026-07-07T21:51:00Z"),
            chrono::DateTime::parse_from_rfc3339("2026-07-07T21:51:00Z")
                .expect("valid")
                .timestamp()
        );
    }

    #[test]
    fn parse_create_time_converts_millis() {
        assert_eq!(parse_create_time_seconds("1700000000000"), 1_700_000_000);
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
    fn search_hit_skips_invalid_channel_id() {
        let doc = SearchMessageDocument {
            message_id: "1".into(),
            channel_id: "0".into(),
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
        assert_eq!(hit.create_time_seconds, 1_700_000_000);
        assert!(!hit.time_hhmm.is_empty());
    }

    #[test]
    fn search_response_matches_same_generation_and_query() {
        let state = ChannelSearchState {
            query: "hello".into(),
            current_page: 1,
            generation: 3,
            ..Default::default()
        };
        assert!(search_response_matches(&state, 3, "hello", 1));
    }

    #[test]
    fn search_response_rejects_stale_page() {
        let state = ChannelSearchState {
            query: "hello".into(),
            current_page: 2,
            generation: 3,
            ..Default::default()
        };
        assert!(!search_response_matches(&state, 3, "hello", 1));
    }

    #[test]
    fn search_response_rejects_stale_generation() {
        let state = ChannelSearchState {
            query: "hello".into(),
            current_page: 1,
            generation: 4,
            ..Default::default()
        };
        assert!(!search_response_matches(&state, 3, "hello", 1));
    }

    #[test]
    fn search_response_rejects_changed_query() {
        let state = ChannelSearchState {
            query: "world".into(),
            current_page: 1,
            generation: 3,
            ..Default::default()
        };
        assert!(!search_response_matches(&state, 3, "hello", 1));
    }
}
