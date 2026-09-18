use std::collections::HashSet;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use mezon_client::transport::{
    ApiCanvas, ApiCanvasDetail, CANVAS_LIST_LIMIT, CANVAS_STATUS_CREATED, CANVAS_STATUS_UPDATE,
};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::realtime;

use crate::KeyedCache;
use crate::channel::{ChannelEvent, ChannelList};
use crate::config::AppConfig;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_CHANNELS: usize = 32;

#[derive(Debug, Clone)]
pub struct CanvasSummary {
    pub id: String,
    pub title: String,
    pub is_default: bool,
    pub creator_id: UserId,
    pub update_time: i64,
    pub create_time: i64,
}

#[derive(Debug, Clone)]
pub struct UploadedCanvasImage {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct CanvasDetail {
    pub id: String,
    pub title: String,
    pub content: String,
    pub creator_id: UserId,
    pub editor_id: UserId,
    pub is_default: bool,
}

pub struct CanvasStore {
    channel_id: Option<String>,
    clan_id: Option<String>,
    cache: KeyedCache<String, Vec<CanvasSummary>>,
    loading: HashSet<String>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalCanvasStore(Entity<CanvasStore>);
impl Global for GlobalCanvasStore {}

impl CanvasStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalCanvasStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCanvasStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalCanvasStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.channel_id = None;
        self.clan_id = None;
        self.cache.clear();
        self.loading.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _list, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(channel_id) = event {
                this.on_active_channel_changed(*channel_id, cx);
            }
        });
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::CanvasEvent, &entity, |this, event, cx| {
                this.handle_canvas_event(event, cx);
            });
        });
        let mut store = Self {
            channel_id: None,
            clan_id: None,
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            loading: HashSet::new(),
            api,
            _channel_sub: channel_sub,
            _conn_watch: conn_watch,
        };
        if let Some(channel) = ChannelList::global(cx).read(cx).active_channel() {
            store.channel_id = Some(channel.id.to_string());
            store.clan_id = Some(channel.clan_id.to_string());
        }
        store
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    if this
                        .update(cx, |this, cx| {
                            this.cache.mark_all_stale();
                            this.refresh(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn handle_canvas_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(realtime::envelope::Message::CanvasEvent(canvas)) = event
        else {
            return;
        };
        if canvas.id == 0 {
            return;
        }
        let channel_id = canvas.channel_id.to_string();
        if self.channel_id.as_deref() != Some(channel_id.as_str()) {
            self.cache.mark_stale(&channel_id);
            return;
        }
        let canvas_id = canvas.id.to_string();
        if canvas.status == CANVAS_STATUS_CREATED || canvas.status == CANVAS_STATUS_UPDATE {
            let known = self
                .cache
                .get_mut(&channel_id)
                .and_then(|list| list.iter_mut().find(|item| item.id == canvas_id))
                .map(|item| {
                    item.title = canvas.title.clone();
                    item.is_default = canvas.is_default;
                    if canvas.status == CANVAS_STATUS_CREATED {
                        item.creator_id = UserId(canvas.editor_id);
                    }
                })
                .is_some();
            if known {
                cx.notify();
            } else if canvas.status == CANVAS_STATUS_CREATED {
                self.refresh(cx);
            }
            return;
        }
        let Some(list) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let before = list.len();
        list.retain(|item| item.id != canvas_id);
        if list.len() != before {
            cx.notify();
        }
    }

    pub fn canvases(&self) -> &[CanvasSummary] {
        self.channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_loading(&self) -> bool {
        self.channel_id
            .as_ref()
            .is_some_and(|id| self.loading.contains(id))
    }

    pub fn channel_id(&self) -> Option<&str> {
        self.channel_id.as_deref()
    }

    pub fn clan_id(&self) -> Option<ClanId> {
        self.clan_id
            .as_ref()
            .and_then(|id| id.parse::<ClanId>().ok())
    }

    fn on_active_channel_changed(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        match channel_id {
            None => {
                self.channel_id = None;
                self.clan_id = None;
            }
            Some(id) => {
                let clan_id = ChannelList::global(cx)
                    .read(cx)
                    .find_channel_in_active_clan(id)
                    .map(|c| c.clan_id)
                    .filter(|clan_id| !clan_id.is_zero())
                    .unwrap_or(ClanId(0));
                self.channel_id = Some(id.to_string());
                self.clan_id = Some(clan_id.to_string());
            }
        }
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        self.cache.touch(&channel_id);
        if self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
            return;
        }
        self.fetch(cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(channel_id) = self.channel_id.clone() {
            self.cache.mark_stale(&channel_id);
        }
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        let (Ok(channel_id_i64), Ok(clan_id_i64)) =
            (channel_id.parse::<i64>(), clan_id.parse::<i64>())
        else {
            return;
        };
        if !self.loading.insert(channel_id.clone()) {
            return;
        }
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .get_channel_canvas_list(channel_id_i64, clan_id_i64, CANVAS_LIST_LIMIT, 1)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&channel_id);
                match result {
                    Ok(list) => {
                        let mut canvases: Vec<_> = list.into_iter().map(summary_from_api).collect();
                        sort_canvases(&mut canvases);
                        let protect = this.channel_id.clone();
                        this.cache.insert(channel_id, canvases, protect.as_ref());
                    }
                    Err(e) => tracing::error!("get_channel_canvas_list failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn create(
        &mut self,
        title: String,
        content: String,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<String>> {
        let Some(channel_id) = self.channel_id.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active channel")));
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active clan")));
        };
        let (Ok(channel_id_i64), Ok(clan_id_i64)) =
            (channel_id.parse::<i64>(), clan_id.parse::<i64>())
        else {
            return Task::ready(Err(anyhow::anyhow!("invalid channel or clan id")));
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let id = api
                .edit_channel_canvas(
                    0,
                    channel_id_i64,
                    clan_id_i64,
                    &title,
                    &content,
                    false,
                    CANVAS_STATUS_CREATED,
                )
                .await?;
            let _ = this.update(cx, |this, cx| {
                let summary = CanvasSummary {
                    id: id.clone(),
                    title,
                    is_default: false,
                    creator_id: UserId(0),
                    update_time: chrono::Utc::now().timestamp(),
                    create_time: chrono::Utc::now().timestamp(),
                };
                if let Some(list) = this.cache.get_mut(&channel_id) {
                    list.insert(0, summary);
                    this.cache.mark_fetched(&channel_id);
                } else {
                    this.cache
                        .insert(channel_id, vec![summary], this.channel_id.as_ref());
                }
                cx.notify();
            });
            Ok(id)
        })
    }

    pub fn save(
        &mut self,
        canvas_id: &str,
        title: String,
        content: String,
        is_default: bool,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<String>> {
        let Some(channel_id) = self.channel_id.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active channel")));
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active clan")));
        };
        let (Ok(id_i64), Ok(channel_id_i64), Ok(clan_id_i64)) = (
            canvas_id.parse::<i64>(),
            channel_id.parse::<i64>(),
            clan_id.parse::<i64>(),
        ) else {
            return Task::ready(Err(anyhow::anyhow!("invalid canvas or channel id")));
        };
        let api = self.api.clone();
        let canvas_id = canvas_id.to_string();
        cx.spawn(async move |this, cx| {
            let id = api
                .edit_channel_canvas(
                    id_i64,
                    channel_id_i64,
                    clan_id_i64,
                    &title,
                    &content,
                    is_default,
                    CANVAS_STATUS_UPDATE,
                )
                .await?;
            let _ = this.update(cx, |this, cx| {
                if let Some(list) = this.cache.get_mut(&channel_id)
                    && let Some(item) = list.iter_mut().find(|c| c.id == canvas_id)
                {
                    item.title = title;
                    item.update_time = chrono::Utc::now().timestamp();
                }
                cx.notify();
            });
            Ok(id)
        })
    }

    pub fn upload_canvas_image(
        &self,
        data: Vec<u8>,
        filetype: String,
        filename: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<UploadedCanvasImage, String>> {
        let api = self.api.clone();
        let base_img_url = AppConfig::global(cx).base_img_url.clone();
        cx.background_executor().spawn(async move {
            upload_canvas_image_bytes(&api, &base_img_url, data, &filetype, &filename).await
        })
    }

    pub fn delete(&mut self, canvas_id: &str, cx: &mut Context<Self>) {
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        let (Ok(canvas_id_i64), Ok(channel_id_i64), Ok(clan_id_i64)) = (
            canvas_id.parse::<i64>(),
            channel_id.parse::<i64>(),
            clan_id.parse::<i64>(),
        ) else {
            return;
        };
        if let Some(list) = self.cache.get_mut(&channel_id) {
            list.retain(|c| c.id != canvas_id);
        }
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .delete_channel_canvas(canvas_id_i64, clan_id_i64, channel_id_i64)
                .await
            {
                tracing::error!("delete_channel_canvas failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    this.cache.mark_stale(&channel_id);
                    this.refresh(cx);
                });
            }
        })
        .detach();
    }

    pub fn fetch_detail(
        &self,
        canvas_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<CanvasDetail>> {
        let Some(channel_id) = self.channel_id.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active channel")));
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active clan")));
        };
        let (Ok(id_i64), Ok(channel_id_i64), Ok(clan_id_i64)) = (
            canvas_id.parse::<i64>(),
            channel_id.parse::<i64>(),
            clan_id.parse::<i64>(),
        ) else {
            return Task::ready(Err(anyhow::anyhow!("invalid canvas or channel id")));
        };
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            let detail = api
                .get_channel_canvas_detail(id_i64, clan_id_i64, channel_id_i64)
                .await?;
            Ok(detail_from_api(detail))
        })
    }
}

pub fn canvas_web_link(
    redirect_uri: &str,
    clan_id: &str,
    channel_id: &str,
    canvas_id: &str,
) -> String {
    let base = redirect_uri.trim_end_matches('/');
    format!("{base}/chat/clans/{clan_id}/channels/{channel_id}/canvas/{canvas_id}")
}

fn summary_from_api(item: ApiCanvas) -> CanvasSummary {
    let creator_id = item
        .creator_id
        .parse::<i64>()
        .map(UserId)
        .unwrap_or(UserId(0));
    CanvasSummary {
        id: item.id,
        title: item.title,
        is_default: item.is_default,
        creator_id,
        update_time: i64::from(item.update_time_seconds),
        create_time: i64::from(item.create_time_seconds),
    }
}

fn detail_from_api(detail: ApiCanvasDetail) -> CanvasDetail {
    let creator_id = detail
        .creator_id
        .parse::<i64>()
        .map(UserId)
        .unwrap_or(UserId(0));
    let editor_id = detail
        .editor_id
        .parse::<i64>()
        .map(UserId)
        .unwrap_or(UserId(0));
    CanvasDetail {
        id: detail.id,
        title: detail.title,
        content: detail.content,
        creator_id,
        editor_id,
        is_default: detail.is_default,
    }
}

fn sort_canvases(canvases: &mut [CanvasSummary]) {
    canvases.sort_by(|a, b| match (a.create_time > 0, b.create_time > 0) {
        (true, true) => b.create_time.cmp(&a.create_time),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => b.id.cmp(&a.id),
    });
}

fn image_dimensions_from_bytes(data: &[u8]) -> (i32, i32) {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .map(|(w, h)| {
            (
                i32::try_from(w).unwrap_or(i32::MAX),
                i32::try_from(h).unwrap_or(i32::MAX),
            )
        })
        .unwrap_or((0, 0))
}

async fn upload_canvas_image_bytes(
    api: &AppApi,
    base_img_url: &str,
    data: Vec<u8>,
    filetype: &str,
    filename: &str,
) -> Result<UploadedCanvasImage, String> {
    let size = i32::try_from(data.len()).map_err(|_| "Image file is too large".to_string())?;
    let (width, height) = image_dimensions_from_bytes(&data);
    let upload = api
        .upload_attachment_file(filename, filetype, size, width, height)
        .await
        .map_err(|e| e.to_string())?;
    mezon_client::transport_runtime::put_bytes_to_content_type(&upload.url, data, filetype)
        .await
        .map_err(|e| e.to_string())?;
    if upload.filename.is_empty() {
        return Err("UploadAttachmentFile returned empty filename".into());
    }
    let base = base_img_url.trim_end_matches('/');
    Ok(UploadedCanvasImage {
        url: format!("{base}/{}", upload.filename),
        width: width.max(0) as u32,
        height: height.max(0) as u32,
    })
}

#[cfg(test)]
fn display_title(title: &str) -> &str {
    if title.is_empty() { "Untitled" } else { title }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_canvas_store(cx: &mut App) -> Entity<CanvasStore> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        crate::realtime::RealtimeDispatch::init(api.clone(), cx);
        crate::clan::ClanList::init(api.clone(), cx);
        ChannelList::init(api.clone(), cx);
        CanvasStore::init(api, cx)
    }

    fn canvas_event(id: i64, channel_id: i64, status: i32, title: &str) -> RealtimeEvent {
        RealtimeEvent::Unhandled(realtime::envelope::Message::CanvasEvent(
            realtime::ChannelCanvas {
                id,
                title: title.into(),
                channel_id,
                status,
                editor_id: 5,
                ..Default::default()
            },
        ))
    }

    fn seed(store: &mut CanvasStore, channel_id: &str, ids: &[i64]) {
        store.channel_id = Some(channel_id.to_string());
        store.clan_id = Some("1".to_string());
        let list = ids
            .iter()
            .map(|id| CanvasSummary {
                id: id.to_string(),
                title: format!("canvas {id}"),
                is_default: false,
                creator_id: UserId(1),
                update_time: *id,
                create_time: *id,
            })
            .collect();
        store.cache.insert(channel_id.to_string(), list, None);
    }

    #[gpui::test]
    fn a_created_event_updates_a_canvas_already_in_the_list(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_canvas_store(cx);
            store.update(cx, |store, cx| {
                seed(store, "42", &[7]);
                store.handle_canvas_event(
                    &canvas_event(7, 42, CANVAS_STATUS_CREATED, "renamed"),
                    cx,
                );
                assert_eq!(store.canvases().len(), 1);
                assert_eq!(store.canvases()[0].title, "renamed");
                assert_eq!(store.canvases()[0].creator_id, UserId(5));
            });
        });
    }

    #[gpui::test]
    fn an_update_event_renames_in_place_and_never_removes(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_canvas_store(cx);
            store.update(cx, |store, cx| {
                seed(store, "42", &[7, 8]);
                store.handle_canvas_event(&canvas_event(7, 42, CANVAS_STATUS_UPDATE, "edited"), cx);
                let ids: Vec<_> = store.canvases().iter().map(|c| c.id.as_str()).collect();
                assert_eq!(
                    ids,
                    ["7", "8"],
                    "saving a canvas must not drop it from the list"
                );
                assert_eq!(store.canvases()[0].title, "edited");
                assert_eq!(
                    store.canvases()[0].creator_id,
                    UserId(1),
                    "an edit by someone else must not rewrite the author"
                );
            });
        });
    }

    #[gpui::test]
    fn an_unknown_status_removes_the_canvas(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_canvas_store(cx);
            store.update(cx, |store, cx| {
                seed(store, "42", &[7, 8]);
                store.handle_canvas_event(&canvas_event(7, 42, 3, ""), cx);
                let ids: Vec<_> = store.canvases().iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, ["8"]);
            });
        });
    }

    #[gpui::test]
    fn an_event_for_another_channel_only_stales_that_cache(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_canvas_store(cx);
            store.update(cx, |store, cx| {
                seed(store, "42", &[7]);
                store.cache.insert("99".to_string(), Vec::new(), None);
                store.handle_canvas_event(&canvas_event(7, 99, 3, ""), cx);
                assert_eq!(store.canvases().len(), 1, "the open channel is untouched");
                assert!(!store.cache.is_fresh("99", crate::CACHE_TTL));
            });
        });
    }

    #[test]
    fn summary_from_api_maps_fields() {
        let summary = summary_from_api(ApiCanvas {
            id: "10".into(),
            title: "Notes".into(),
            content: "{}".into(),
            is_default: true,
            creator_id: "42".into(),
            update_time_seconds: 100,
            create_time_seconds: 90,
        });
        assert_eq!(summary.id, "10");
        assert_eq!(summary.title, "Notes");
        assert!(summary.is_default);
        assert_eq!(summary.creator_id, UserId(42));
        assert_eq!(summary.update_time, 100);
        assert_eq!(summary.create_time, 90);
    }

    #[test]
    fn sort_canvases_newest_first() {
        let mut items = vec![
            CanvasSummary {
                id: "1".into(),
                title: "old".into(),
                is_default: false,
                creator_id: UserId(1),
                update_time: 1,
                create_time: 10,
            },
            CanvasSummary {
                id: "2".into(),
                title: "new".into(),
                is_default: false,
                creator_id: UserId(1),
                update_time: 2,
                create_time: 20,
            },
            CanvasSummary {
                id: "3".into(),
                title: String::new(),
                is_default: false,
                creator_id: UserId(1),
                update_time: 0,
                create_time: 0,
            },
        ];
        sort_canvases(&mut items);
        assert_eq!(items[0].id, "2");
        assert_eq!(items[1].id, "1");
        assert_eq!(items[2].id, "3");
    }

    #[test]
    fn canvas_web_link_trims_trailing_slash() {
        let link = canvas_web_link("https://mezon.ai/", "1", "2", "3");
        assert_eq!(link, "https://mezon.ai/chat/clans/1/channels/2/canvas/3");
    }

    #[test]
    fn display_title_falls_back_untitled() {
        assert_eq!(display_title(""), "Untitled");
        assert_eq!(display_title("Hello"), "Hello");
    }
}
