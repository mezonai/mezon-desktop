use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString};
use mezon_client::AppApi;
use mezon_client::transport::ApiChannelAttachment;

use crate::KeyedCache;
use crate::gallery::resolve_attachment_uploader;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};

pub const FILES_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
pub const FILES_PAGE_SIZE: i32 = 100;
pub const FILES_TYPED_QUERY: &str = "FILE";
pub const FILES_BROAD_QUERY: &str = "";
const FILES_QUERY_GEN: u8 = 3;
const MAX_CACHED_CHANNELS: usize = 32;

#[derive(Debug, Clone)]
pub struct ChannelDocument {
    pub id: i64,
    pub channel_id: ChannelId,
    pub clan_id: ClanId,
    pub message_id: MessageId,
    pub uploader_id: UserId,
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub create_time_seconds: u32,
    pub uploader_name: SharedString,
}

impl ChannelDocument {
    pub fn from_api(api: ApiChannelAttachment, channel_id: ChannelId, clan_id: ClanId) -> Self {
        let filename = if api.filename.is_empty() {
            "File".to_string()
        } else {
            api.filename
        };
        let filetype = if api.filetype.is_empty() {
            "File".to_string()
        } else {
            api.filetype
        };
        Self {
            id: api.id,
            channel_id,
            clan_id,
            message_id: MessageId(api.message_id),
            uploader_id: UserId(api.uploader),
            url: api.url,
            filename,
            filetype,
            create_time_seconds: api.create_time_seconds,
            uploader_name: SharedString::default(),
        }
    }

    pub fn is_failed(&self) -> bool {
        self.filename == "failAttachment"
    }
}

pub fn is_document(filetype: &str) -> bool {
    let ft = filetype.trim();
    if ft.is_empty() {
        return true;
    }
    let lower = ft.to_ascii_lowercase();
    if lower == "sticker" {
        return false;
    }
    if lower.starts_with("image/") || lower.starts_with("video/") {
        return false;
    }
    true
}

#[derive(Default)]
struct FilesChannel {
    documents: Vec<ChannelDocument>,
    is_loading: bool,
    fetch_error: bool,
    fetched_at: Option<Instant>,
    query_gen: u8,
}

impl FilesChannel {
    fn is_fresh(&self) -> bool {
        self.query_gen == FILES_QUERY_GEN
            && self
                .fetched_at
                .is_some_and(|t| t.elapsed() < FILES_CACHE_TTL)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FilesEvent {
    Changed(ChannelId),
}

pub struct FilesStore {
    by_channel: KeyedCache<ChannelId, FilesChannel>,
    api: Arc<AppApi>,
}

struct GlobalFilesStore(Entity<FilesStore>);
impl Global for GlobalFilesStore {}

impl EventEmitter<FilesEvent> for FilesStore {}

impl FilesStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            by_channel: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            api,
        });
        cx.set_global(GlobalFilesStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalFilesStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalFilesStore>().map(|g| g.0.clone())
    }

    pub fn documents(&self, channel_id: ChannelId) -> &[ChannelDocument] {
        self.by_channel
            .get(&channel_id)
            .map(|c| c.documents.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_loading(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.is_loading)
    }

    pub fn fetch_error(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.fetch_error)
    }

    pub fn is_empty(&self, channel_id: ChannelId) -> bool {
        self.documents(channel_id).is_empty()
    }

    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let needs_fetch = match self.by_channel.get(&channel_id) {
            Some(c) => !c.is_loading && (c.documents.is_empty() || !c.is_fresh() || c.fetch_error),
            None => true,
        };
        if needs_fetch {
            self.fetch(clan_id, channel_id, cx);
        } else {
            self.by_channel.touch(&channel_id);
        }
    }

    pub fn refresh(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self
            .by_channel
            .get(&channel_id)
            .is_some_and(|c| c.is_loading)
        {
            return;
        }
        self.fetch(clan_id, channel_id, cx);
    }

    pub fn clear_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.by_channel.remove(&channel_id).is_some() {
            cx.emit(FilesEvent::Changed(channel_id));
            cx.notify();
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.by_channel.is_empty() {
            return;
        }
        let channel_ids: Vec<ChannelId> = self.by_channel.iter().map(|(id, _)| *id).collect();
        self.by_channel.clear();
        for channel_id in channel_ids {
            cx.emit(FilesEvent::Changed(channel_id));
        }
        cx.notify();
    }

    pub fn refresh_uploaders(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if !self.by_channel.contains(&channel_id) {
            return;
        }
        if self.enrich_channel(channel_id, cx) {
            cx.emit(FilesEvent::Changed(channel_id));
            cx.notify();
        }
    }

    fn ensure_channel(&mut self, channel_id: ChannelId) -> &mut FilesChannel {
        if !self.by_channel.contains(&channel_id) {
            self.by_channel
                .insert(channel_id, FilesChannel::default(), Some(&channel_id));
        } else {
            self.by_channel.touch(&channel_id);
        }
        self.by_channel
            .get_mut(&channel_id)
            .expect("channel just ensured")
    }

    fn fetch(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let entry = self.ensure_channel(channel_id);
        entry.is_loading = true;
        entry.fetch_error = false;
        cx.emit(FilesEvent::Changed(channel_id));
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result =
                fetch_channel_documents(&api, clan_id.0, channel_id.0, FILES_PAGE_SIZE).await;
            let mapped = result.map(|list| {
                let mut docs: Vec<ChannelDocument> = list
                    .into_iter()
                    .filter(|a| is_document(&a.filetype))
                    .map(|a| ChannelDocument::from_api(a, channel_id, clan_id))
                    .collect();
                sort_desc_in_place(&mut docs);
                dedupe_by_id(docs)
            });
            let _ = this.update(cx, |this, cx| {
                let succeeded = match mapped {
                    Ok(docs) => {
                        let entry = this.ensure_channel(channel_id);
                        entry.is_loading = false;
                        entry.documents = docs;
                        entry.fetched_at = Some(Instant::now());
                        entry.query_gen = FILES_QUERY_GEN;
                        entry.fetch_error = false;
                        true
                    }
                    Err(e) => {
                        tracing::error!("list_channel_attachments (documents) failed: {e}");
                        let entry = this.ensure_channel(channel_id);
                        entry.is_loading = false;
                        entry.fetch_error = true;
                        false
                    }
                };
                if succeeded {
                    this.enrich_channel(channel_id, cx);
                }
                cx.emit(FilesEvent::Changed(channel_id));
                cx.notify();
            });
        })
        .detach();
    }

    fn enrich_channel(&mut self, channel_id: ChannelId, cx: &App) -> bool {
        let Some(entry) = self.by_channel.get_mut(&channel_id) else {
            return false;
        };
        let mut changed = false;
        for doc in entry.documents.iter_mut() {
            let info = resolve_attachment_uploader(
                doc.clan_id,
                doc.channel_id,
                doc.uploader_id,
                doc.message_id,
                None,
                cx,
            );
            let name = if info.name.is_empty() {
                SharedString::from("Unknown")
            } else {
                info.name.into()
            };
            if doc.uploader_name != name {
                doc.uploader_name = name;
                changed = true;
            }
        }
        changed
    }
}

async fn fetch_channel_documents(
    api: &AppApi,
    clan_id: i64,
    channel_id: i64,
    limit: i32,
) -> anyhow::Result<Vec<ApiChannelAttachment>> {
    let broad = api
        .list_channel_attachments(
            clan_id,
            channel_id,
            FILES_BROAD_QUERY,
            0,
            limit,
            0,
            0,
        )
        .await?;
    let typed = api
        .list_channel_attachments(
            clan_id,
            channel_id,
            FILES_TYPED_QUERY,
            0,
            limit,
            0,
            0,
        )
        .await?;
    Ok(merge_attachments(broad, typed))
}

fn merge_attachments(
    mut broad: Vec<ApiChannelAttachment>,
    typed: Vec<ApiChannelAttachment>,
) -> Vec<ApiChannelAttachment> {
    let mut seen = std::collections::HashSet::new();
    for item in &broad {
        seen.insert(item.id);
    }
    for item in typed {
        if seen.insert(item.id) {
            broad.push(item);
        }
    }
    broad
}

fn sort_desc_in_place(items: &mut [ChannelDocument]) {
    items.sort_by(|a, b| {
        a.create_time_seconds
            .cmp(&b.create_time_seconds)
            .reverse()
            .then_with(|| a.id.cmp(&b.id).reverse())
    });
}

fn dedupe_by_id(docs: Vec<ChannelDocument>) -> Vec<ChannelDocument> {
    let mut seen = std::collections::HashSet::new();
    docs.into_iter().filter(|doc| seen.insert(doc.id)).collect()
}

pub fn short_file_type_label(filetype: &str) -> SharedString {
    short_file_type_label_for(filetype, "")
}

pub fn short_file_type_label_for(filetype: &str, filename: &str) -> SharedString {
    let ft = filetype.trim();
    if !ft.is_empty()
        && !ft.eq_ignore_ascii_case("file")
        && ft != "application/vnd.android.package-archive"
    {
        let lower = ft.to_ascii_lowercase();
        let label = match lower.as_str() {
            "application/pdf" => Some("PDF"),
            "text/csv" | "application/csv" => Some("CSV"),
            "text/plain" => Some("TXT"),
            "text/markdown" => Some("MD"),
            "application/json" => Some("JSON"),
            "application/zip" | "application/x-zip-compressed" => Some("ZIP"),
            "application/vnd.rar" | "application/x-rar-compressed" => Some("RAR"),
            "application/x-7z-compressed" => Some("7Z"),
            "application/msword"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "docx" => Some("DOC"),
            "application/vnd.ms-excel"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "xlsx" => Some("XLS"),
            "application/vnd.ms-powerpoint"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "pptx" => Some("PPT"),
            _ => {
                if let Some(ext) = lower.rsplit('/').next()
                    && ext.len() <= 8
                    && !ext.is_empty()
                {
                    return SharedString::from(ext.to_ascii_uppercase());
                }
                None
            }
        };
        if let Some(label) = label {
            return label.into();
        }
    }
    if let Some(ext) = filename.rsplit_once('.').map(|(_, e)| e) {
        let ext = ext.trim();
        if !ext.is_empty() && ext.len() <= 8 && !ext.contains('/') {
            return SharedString::from(ext.to_ascii_uppercase());
        }
    }
    "FILE".into()
}

pub fn filename_matches_query(filename: &str, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    filename
        .to_ascii_lowercase()
        .contains(&q.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_document_excludes_media_and_sticker() {
        assert!(!is_document("image/png"));
        assert!(!is_document("video/mp4"));
        assert!(!is_document("sticker"));
        assert!(is_document("application/mp4"));
        assert!(is_document("application/pdf"));
        assert!(is_document("text/csv"));
        assert!(is_document("text/plain"));
        assert!(is_document("FILE"));
        assert!(is_document(""));
        assert!(is_document("application/zip"));
        assert!(is_document("audio/mpeg"));
        assert!(is_document("audio/mp4"));
    }

    #[test]
    fn from_api_defaults_empty_names() {
        let api = ApiChannelAttachment {
            id: 1,
            filename: String::new(),
            filetype: String::new(),
            ..ApiChannelAttachment::default()
        };
        let doc = ChannelDocument::from_api(api, ChannelId(2), ClanId(3));
        assert_eq!(doc.filename, "File");
        assert_eq!(doc.filetype, "File");
        assert_eq!(doc.channel_id, ChannelId(2));
        assert_eq!(doc.clan_id, ClanId(3));
    }

    #[test]
    fn sort_desc_orders_by_time_then_id() {
        let mut items = vec![
            ChannelDocument {
                id: 1,
                channel_id: ChannelId(1),
                clan_id: ClanId(1),
                message_id: MessageId(0),
                uploader_id: UserId(0),
                url: String::new(),
                filename: "a".into(),
                filetype: "FILE".into(),
                create_time_seconds: 10,
                uploader_name: SharedString::default(),
            },
            ChannelDocument {
                id: 3,
                channel_id: ChannelId(1),
                clan_id: ClanId(1),
                message_id: MessageId(0),
                uploader_id: UserId(0),
                url: String::new(),
                filename: "b".into(),
                filetype: "FILE".into(),
                create_time_seconds: 20,
                uploader_name: SharedString::default(),
            },
            ChannelDocument {
                id: 2,
                channel_id: ChannelId(1),
                clan_id: ClanId(1),
                message_id: MessageId(0),
                uploader_id: UserId(0),
                url: String::new(),
                filename: "c".into(),
                filetype: "FILE".into(),
                create_time_seconds: 20,
                uploader_name: SharedString::default(),
            },
        ];
        sort_desc_in_place(&mut items);
        assert_eq!(items[0].id, 3);
        assert_eq!(items[1].id, 2);
        assert_eq!(items[2].id, 1);
    }

    #[test]
    fn short_file_type_label_maps_common_mimes() {
        assert_eq!(short_file_type_label("application/pdf").as_ref(), "PDF");
        assert_eq!(short_file_type_label("text/csv").as_ref(), "CSV");
        assert_eq!(short_file_type_label("FILE").as_ref(), "FILE");
        assert_eq!(
            short_file_type_label("application/vnd.android.package-archive").as_ref(),
            "FILE"
        );
        assert_eq!(
            short_file_type_label_for("FILE", "Mezon AI Summary.pdf").as_ref(),
            "PDF"
        );
        assert_eq!(short_file_type_label_for("pdf", "x.bin").as_ref(), "PDF");
    }

    #[test]
    fn merge_attachments_dedupes_by_id() {
        let broad = vec![
            ApiChannelAttachment {
                id: 1,
                filetype: "text/plain".into(),
                ..ApiChannelAttachment::default()
            },
            ApiChannelAttachment {
                id: 2,
                filetype: "image/png".into(),
                ..ApiChannelAttachment::default()
            },
        ];
        let typed = vec![
            ApiChannelAttachment {
                id: 2,
                filetype: "application/pdf".into(),
                ..ApiChannelAttachment::default()
            },
            ApiChannelAttachment {
                id: 3,
                filetype: "application/pdf".into(),
                ..ApiChannelAttachment::default()
            },
        ];
        let merged = merge_attachments(broad, typed);
        assert_eq!(merged.len(), 3);
        assert!(merged.iter().any(|a| a.id == 1));
        assert!(merged.iter().any(|a| a.id == 2));
        assert!(merged.iter().any(|a| a.id == 3));
    }

    #[test]
    fn filename_matches_query_is_case_insensitive() {
        assert!(filename_matches_query("Report.PDF", ""));
        assert!(filename_matches_query("Report.PDF", "  "));
        assert!(filename_matches_query("Report.PDF", "report"));
        assert!(filename_matches_query("Mezon AI Summary.pdf", "ai"));
        assert!(!filename_matches_query("Mezon AI Summary.pdf", "docx"));
    }

    #[test]
    fn dedupe_by_id_keeps_first() {
        let docs = vec![
            ChannelDocument {
                id: 1,
                channel_id: ChannelId(1),
                clan_id: ClanId(1),
                message_id: MessageId(0),
                uploader_id: UserId(0),
                url: String::new(),
                filename: "a".into(),
                filetype: "FILE".into(),
                create_time_seconds: 20,
                uploader_name: SharedString::default(),
            },
            ChannelDocument {
                id: 1,
                channel_id: ChannelId(1),
                clan_id: ClanId(1),
                message_id: MessageId(0),
                uploader_id: UserId(0),
                url: String::new(),
                filename: "b".into(),
                filetype: "FILE".into(),
                create_time_seconds: 10,
                uploader_name: SharedString::default(),
            },
            ChannelDocument {
                id: 2,
                channel_id: ChannelId(1),
                clan_id: ClanId(1),
                message_id: MessageId(0),
                uploader_id: UserId(0),
                url: String::new(),
                filename: "c".into(),
                filetype: "FILE".into(),
                create_time_seconds: 5,
                uploader_name: SharedString::default(),
            },
        ];
        let deduped = dedupe_by_id(docs);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].id, 1);
        assert_eq!(deduped[0].filename, "a");
        assert_eq!(deduped[1].id, 2);
    }
}
