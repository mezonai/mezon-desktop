use std::sync::Arc;

use chrono::NaiveDate;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Task};
use mezon_client::AppApi;
use mezon_client::transport::{
    ApiMessageContent, ContentToken, detect_markdown, enrich_content_tokens,
    markdown_content_tokens,
};
use mezon_client::{
    build_clan_channel_content_search, build_direct_content_search, parse_search_attachment_field,
    parse_search_mentions_field, search_page_count,
};
use mezon_proto::api::SearchMessageDocument;

use crate::AppConfig;
use crate::cache::KeyedCache;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::message::{
    Embed, MessageAttachment, MessageSpan, OgpPreview, RichLayout, build_rich_layout, parse_spans,
};
use crate::message_time::{format_local_time_hhmm, local_datetime};
use crate::messages::{MessagesStore, build_embeds, build_ogp_preview};

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

#[derive(Debug, Clone)]
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
    pub spans: Arc<[MessageSpan]>,
    pub rich_layout: Option<Arc<RichLayout>>,
    pub ogp: Option<Box<OgpPreview>>,
    pub embeds: Arc<[Embed]>,
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
    _ogp_hydrate_task: Task<()>,
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
            _ogp_hydrate_task: Task::ready(()),
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
                let missing_ogp = {
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
                            enrich_search_results_ogp(&mut state.results, cx);
                            state
                                .results
                                .iter()
                                .filter(|hit| hit.ogp.is_none() && search_hit_may_have_ogp(hit))
                                .map(|hit| (hit.clan_id, hit.channel_id, hit.message_id))
                                .collect::<Vec<_>>()
                        }
                        Err(err) => {
                            tracing::warn!("search_message failed: {err}");
                            state.has_error = true;
                            state.total = 0;
                            state.results.clear();
                            cx.emit(MessageSearchEvent::SearchFailed);
                            Vec::new()
                        }
                    }
                };
                cx.notify();
                if !missing_ogp.is_empty() {
                    this.hydrate_missing_ogp(channel_id, generation, missing_ogp, cx);
                }
            });
        });
    }

    fn hydrate_missing_ogp(
        &mut self,
        channel_id: ChannelId,
        generation: u64,
        missing: Vec<(ClanId, ChannelId, MessageId)>,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        let cfg = AppConfig::try_global(cx).cloned();
        self._ogp_hydrate_task = cx.spawn(async move |this, cx| {
            for (clan_id, hit_channel_id, message_id) in missing {
                let Some(still_needed) = this
                    .update(cx, |this, _| {
                        let state = this.states.get(&channel_id)?;
                        if state.generation != generation {
                            return None;
                        }
                        Some(state.results.iter().any(|hit| {
                            hit.message_id == message_id && hit.ogp.is_none()
                        }))
                    })
                    .ok()
                    .flatten()
                else {
                    return;
                };
                if !still_needed {
                    continue;
                }
                let page = match api
                    .list_channel_messages(
                        clan_id.get(),
                        hit_channel_id.get(),
                        message_id.get(),
                        2,
                        5,
                    )
                    .await
                {
                    Ok(page) => page,
                    Err(err) => {
                        tracing::debug!(
                            channel_id = hit_channel_id.get(),
                            message_id = message_id.get(),
                            "search ogp hydrate failed: {err}"
                        );
                        continue;
                    }
                };
                let ogp = page
                    .messages
                    .into_iter()
                    .find(|msg| msg.message_id == message_id.get())
                    .and_then(|msg| build_ogp_preview(&msg.content_tokens, cfg.as_ref()));
                let Some(ogp) = ogp else {
                    continue;
                };
                let _ = this.update(cx, |this, cx| {
                    let Some(state) = this.states.get_mut(&channel_id) else {
                        return;
                    };
                    if state.generation != generation {
                        return;
                    }
                    if let Some(hit) = state
                        .results
                        .iter_mut()
                        .find(|hit| hit.message_id == message_id && hit.ogp.is_none())
                    {
                        hit.ogp = Some(ogp);
                        cx.notify();
                    }
                });
            }
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
    let parsed = parse_search_hit_content(&doc.content, &doc.mentions, cfg);

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
        content_preview: SharedString::from(parsed.preview),
        spans: parsed.spans,
        rich_layout: parsed.rich_layout,
        ogp: parsed.ogp,
        embeds: parsed.embeds,
        channel_label: SharedString::from(doc.channel_label.clone()),
        create_time_seconds,
        time_hhmm,
        local_date,
        image_attachment,
    })
}

pub fn enrich_search_results_ogp(results: &mut [SearchHit], cx: &App) {
    let Some(messages) = MessagesStore::try_global(cx) else {
        return;
    };
    let messages = messages.read(cx);
    for hit in results.iter_mut() {
        if hit.ogp.is_some() {
            continue;
        }
        if let Some(ogp) = messages
            .message_in_channel(hit.channel_id, hit.message_id)
            .and_then(|msg| msg.ogp.clone())
        {
            hit.ogp = Some(ogp);
        }
    }
}

pub fn resolve_search_hit_ogp(hit: &SearchHit, cx: &App) -> Option<Box<OgpPreview>> {
    if let Some(ogp) = hit.ogp.clone() {
        return Some(ogp);
    }
    MessagesStore::try_global(cx)?
        .read(cx)
        .message_in_channel(hit.channel_id, hit.message_id)
        .and_then(|msg| msg.ogp.clone())
}

fn search_hit_may_have_ogp(hit: &SearchHit) -> bool {
    hit.spans
        .iter()
        .any(|span| matches!(span, MessageSpan::Link { .. }))
        || hit.content_preview.contains("http://")
        || hit.content_preview.contains("https://")
}

struct ParsedSearchContent {
    preview: String,
    spans: Arc<[MessageSpan]>,
    rich_layout: Option<Arc<RichLayout>>,
    ogp: Option<Box<OgpPreview>>,
    embeds: Arc<[Embed]>,
}

fn parse_search_hit_content(
    content_raw: &str,
    mentions_raw: &str,
    cfg: Option<&AppConfig>,
) -> ParsedSearchContent {
    let trimmed = content_raw.trim();
    let mut tokens = parse_search_content_tokens(trimmed);
    if tokens.t.is_empty()
        && let Some(text) = extract_message_text_content(trimmed)
    {
        tokens.t = text;
    }
    let entity_mentions = parse_search_mentions_field(mentions_raw);
    if !entity_mentions.is_empty() {
        enrich_content_tokens(&mut tokens, &entity_mentions);
    }
    merge_detected_markdown_links(&mut tokens);
    let spans = parse_spans(&tokens);
    let rich_layout = build_rich_layout(&spans);
    let ogp = build_ogp_preview(&tokens, cfg);
    let embeds = build_embeds(&tokens, cfg);
    let preview = if tokens.t.is_empty() {
        content_preview_from_raw(content_raw)
    } else {
        crate::message::reply_preview_line(tokens.t.trim())
    };
    ParsedSearchContent {
        preview,
        spans: spans.into(),
        rich_layout,
        ogp,
        embeds,
    }
}

fn parse_search_content_tokens(trimmed: &str) -> ApiMessageContent {
    if trimmed.is_empty() {
        return ApiMessageContent::default();
    }
    if let Ok(tokens) = serde_json::from_str::<ApiMessageContent>(trimmed) {
        return tokens;
    }
    if let Ok(serde_json::Value::String(inner)) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Ok(tokens) = serde_json::from_str::<ApiMessageContent>(&inner)
    {
        return tokens;
    }
    recover_search_content_tokens(trimmed)
}

fn recover_search_content_tokens(trimmed: &str) -> ApiMessageContent {
    let mut fallback = ApiMessageContent::default();
    let root = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::String(inner)) => {
            serde_json::from_str::<serde_json::Value>(&inner).ok()
        }
        Ok(value) => Some(value),
        Err(_) => None,
    };
    if let Some(serde_json::Value::Object(map)) = root {
        if let Some(text) = map.get("t").and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }) {
            fallback.t = text;
        }
        fallback.mentions = recover_content_token_array(map.get("mentions"));
        fallback.mk = recover_content_token_array(map.get("mk"));
        fallback.lk = recover_content_token_array(map.get("lk"));
        fallback.vk = recover_content_token_array(map.get("vk"));
        fallback.lky = recover_content_token_array(map.get("lky"));
    }
    if fallback.t.is_empty() {
        if let Some(text) = extract_message_text_content(trimmed) {
            fallback.t = text;
        } else if !trimmed.starts_with('{') && !trimmed.starts_with('"') {
            fallback.t = trimmed.to_string();
        }
    }
    fallback
}

fn recover_content_token_array(value: Option<&serde_json::Value>) -> Vec<ContentToken> {
    let Some(serde_json::Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| serde_json::from_value::<ContentToken>(item.clone()).ok())
        .collect()
}

fn merge_detected_markdown_links(tokens: &mut ApiMessageContent) {
    if tokens.t.is_empty() {
        return;
    }
    let detected = detect_markdown(&tokens.t);
    if detected.is_empty() {
        return;
    }
    let existing: std::collections::HashSet<(i64, i64)> = tokens
        .lk
        .iter()
        .chain(tokens.mk.iter().filter(|tok| {
            matches!(
                tok.kind.as_deref(),
                Some("lk" | "vk" | "lk_yt" | "lk_fb" | "lk_tt" | "lk_ogp")
            )
        }))
        .filter_map(|tok| Some((tok.s.unwrap_or(0), tok.e?)))
        .filter(|(s, e)| *e > *s)
        .collect();
    for tok in markdown_content_tokens(&detected) {
        let Some(s) = tok.s else {
            continue;
        };
        let Some(e) = tok.e else {
            continue;
        };
        if existing.contains(&(s, e)) {
            continue;
        }
        if tok.kind.as_deref() == Some("lk") {
            tokens.mk.push(tok);
        }
    }
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
                clamp_search_media_size(att.display_width, att.display_height);
            SearchHitImage {
                proxied_src,
                display_width,
                display_height,
            }
        })
}

fn clamp_search_media_size(width: f32, height: f32) -> (f32, f32) {
    const MAX_W: f32 = 280.;
    const MAX_H: f32 = 200.;
    if width <= 0. || height <= 0. {
        return (200., 150.);
    }
    let scale = (MAX_W / width).min(MAX_H / height).min(1.);
    ((width * scale).max(1.), (height * scale).max(1.))
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
        assert_eq!(
            hit.rich_layout.as_ref().map(|layout| layout.text.as_ref()),
            Some("find me")
        );
    }

    #[test]
    fn search_hit_builds_mention_runs_from_content_tokens() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content:
                r#"{"t":"@KOMU hello","mentions":[{"s":0,"e":5,"user_id":"42","username":"KOMU"}]}"#
                    .into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        let layout = hit.rich_layout.expect("rich layout");
        assert_eq!(layout.text.as_ref(), "@KOMU hello");
        assert!(layout.runs.iter().any(|run| {
            run.kind == crate::message::RichRunKind::Mention && run.range == (0..5)
        }));
    }

    #[test]
    fn search_hit_detects_bare_url_as_link() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"see https://github.com/mezonai/mezon/pull/1 please"}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.spans
                .iter()
                .any(|span| matches!(span, crate::message::MessageSpan::Link { .. })),
            "expected link span, got {:?}",
            hit.spans
        );
    }

    #[test]
    fn search_hit_keeps_link_text_and_ogp_from_lk_ogp() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"https://github.com/mezonai/mezon/pull/1","mk":[{"s":0,"e":42,"type":"lk_ogp","url":"https://github.com/mezonai/mezon/pull/1","title":"PR title","description":"Bug:","image":"https://cdn/og.png"}]}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.spans
                .iter()
                .any(|span| matches!(span, crate::message::MessageSpan::Link { .. })),
            "expected link span from lk_ogp, got {:?}",
            hit.spans
        );
        let ogp = hit.ogp.expect("ogp");
        assert_eq!(ogp.title.as_ref(), "PR title");
        assert_eq!(ogp.description.as_ref(), "Bug:");
    }

    #[test]
    fn search_hit_keeps_ogp_marker_past_end_of_text() {
        let text = "https://github.com/mezonai/mezon-desktop/pull/71 @huy.lexuan hi";
        let end = text.encode_utf16().count();
        let content = format!(
            r#"{{"t":"{text}","mk":[{{"s":0,"e":50,"type":"lk"}},{{"s":{end},"e":{},"type":"lk_ogp","url":"https://github.com/mezonai/mezon-desktop/pull/71","title":"PR 71","description":"Bug:","image":"https://cdn/og.png"}}]}}"#,
            end + 1
        );
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content,
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        let ogp = hit.ogp.expect("past-end lk_ogp should still build ogp");
        assert_eq!(ogp.title.as_ref(), "PR 71");
        assert_eq!(
            ogp.url.as_str(),
            "https://github.com/mezonai/mezon-desktop/pull/71"
        );
    }

    #[test]
    fn search_hit_keeps_ogp_when_content_has_null_embed() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"https://github.com/mezonai/mezon/pull/1 hi","mk":[{"s":0,"e":42,"type":"lk"},{"s":45,"e":46,"type":"lk_ogp","url":"https://github.com/mezonai/mezon/pull/1","title":"PR title","description":"Bug:","image":"https://cdn/og.png"}],"embed":null}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.ogp.is_some(),
            "null embed must not drop lk_ogp; preview={}",
            hit.content_preview
        );
    }

    #[test]
    fn search_hit_keeps_ogp_when_embed_fields_null() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"https://github.com/mezonai/mezon/pull/1","mk":[{"s":39,"e":40,"type":"lk_ogp","url":"https://github.com/mezonai/mezon/pull/1","title":"PR title","description":"Bug:","image":"https://cdn/og.png"}],"embed":[{"fields":null}]}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.ogp.is_some(),
            "null embed.fields must not drop lk_ogp; preview={}",
            hit.content_preview
        );
    }

    #[test]
    fn search_hit_may_have_ogp_detects_links() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"see https://example.com/page please"}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            search_hit_may_have_ogp(&hit),
            "bare URL content should be hydrate candidate"
        );
    }

    #[test]
    fn search_hit_keeps_ogp_when_url_missing_on_token() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"https://github.com/mezonai/mezon/pull/1 more","mk":[{"s":0,"e":42,"type":"lk_ogp","title":"PR title","description":"Bug:","image":"https://cdn/og.png"}]}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        let ogp = hit.ogp.expect("ogp url should fall back from text slice");
        assert_eq!(ogp.title.as_ref(), "PR title");
        assert_eq!(ogp.url.as_str(), "https://github.com/mezonai/mezon/pull/1");
    }

    #[test]
    fn search_hit_keeps_ogp_when_cvtt_is_null() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"https://github.com/mezonai/mezon/pull/1","mk":[{"s":39,"e":40,"type":"lk_ogp","url":"https://github.com/mezonai/mezon/pull/1","title":"PR title","description":"Bug:","image":"https://cdn/og.png"}],"cvtt":null}"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(hit.ogp.is_some(), "null cvtt must not drop lk_ogp");
    }

    #[test]
    fn search_hit_keeps_ogp_from_double_encoded_content() {
        let inner = r#"{"t":"https://github.com/mezonai/mezon/pull/1","mk":[{"s":39,"e":40,"type":"lk_ogp","url":"https://github.com/mezonai/mezon/pull/1","title":"PR title","description":"Bug:","image":"https://cdn/og.png"}]}"#;
        let content = serde_json::to_string(inner).expect("encode");
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content,
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.ogp.is_some(),
            "double-encoded content must keep lk_ogp; preview={}",
            hit.content_preview
        );
    }

    #[test]
    fn search_hit_applies_mentions_from_document_field() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"hello @huy.lexuan check"}"#.into(),
            mentions: r#"[{"s":6,"e":17,"user_id":"99","username":"huy.lexuan"}]"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.spans.iter().any(|span| {
                matches!(
                    span,
                    crate::message::MessageSpan::Mention { display, .. }
                        if display.as_ref() == "@huy.lexuan"
                )
            }),
            "expected mention span, got {:?}",
            hit.spans
        );
    }

    #[test]
    fn search_hit_formats_leading_mention_when_s_omitted() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"@gia.chuvan hello"}"#.into(),
            mentions: r#"[{"e":11,"user_id":"7","username":"gia.chuvan"}]"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        assert!(
            hit.spans.iter().any(|span| {
                matches!(
                    span,
                    crate::message::MessageSpan::Mention { display, .. }
                        if display.as_ref() == "@gia.chuvan"
                )
            }),
            "expected leading mention when s is omitted, got {:?}",
            hit.spans
        );
        let layout = hit.rich_layout.expect("rich layout");
        assert!(layout.runs.iter().any(|run| {
            run.kind == crate::message::RichRunKind::Mention && run.range.start == 0
        }));
    }

    #[test]
    fn search_hit_keeps_all_mentions_when_entity_field_partial() {
        let doc = SearchMessageDocument {
            message_id: "100".into(),
            channel_id: "200".into(),
            content: r#"{"t":"@huy.lexuan @gia.chuvan hi","mentions":[{"s":0,"e":11,"user_id":"1","username":"huy.lexuan"},{"s":12,"e":23,"user_id":"7","username":"gia.chuvan"}]}"#.into(),
            mentions: r#"[{"s":12,"e":23,"user_id":"7","username":"gia.chuvan"}]"#.into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let hit = search_hit_from_document(&doc, None).expect("hit");
        let mention_displays: Vec<_> = hit
            .spans
            .iter()
            .filter_map(|span| match span {
                crate::message::MessageSpan::Mention { display, .. } => Some(display.as_ref()),
                _ => None,
            })
            .collect();
        assert!(
            mention_displays.contains(&"@huy.lexuan"),
            "expected leading content mention kept, got {mention_displays:?}"
        );
        assert!(
            mention_displays.contains(&"@gia.chuvan"),
            "expected entity mention present, got {mention_displays:?}"
        );
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
