use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::Freshness;
use crate::emoji::{
    EmoticonError, EmoticonErrorKind, MAX_STICKER_BYTES, is_valid_emoticon_shortname,
    upload_emoticon_file,
};
use crate::ids::ClanId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::voice::{MAX_SOUND_BYTES, upload_sound_file};

pub const STICKER_MEDIA_TYPE: i32 = 0;
pub const AUDIO_MEDIA_TYPE: i32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sticker {
    pub id: String,
    pub shortname: String,
    pub src: String,
    pub category: String,
    pub clan_id: String,
    pub clan_name: String,
    pub logo: String,
    pub creator_id: String,
    pub is_for_sale: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClanSound {
    pub id: String,
    pub shortname: String,
    pub src: String,
    pub clan_id: String,
    pub clan_name: String,
    pub logo: String,
    pub creator_id: String,
}

#[derive(Debug, Clone)]
pub enum StickerEvent {
    Changed,
}

pub struct StickerStore {
    by_id: HashMap<String, Sticker>,
    order: Vec<String>,
    sounds: Vec<ClanSound>,
    freshness: Freshness,
    loading: bool,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalStickerStore(Entity<StickerStore>);
impl Global for GlobalStickerStore {}

impl EventEmitter<StickerEvent> for StickerStore {}

impl StickerStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalStickerStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStickerStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalStickerStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_id.clear();
        self.order.clear();
        self.sounds.clear();
        self.freshness.mark_stale();
        self.loading = false;
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::StickerCreate,
                RealtimeKind::StickerUpdate,
                RealtimeKind::StickerDelete,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_sticker_event(event, cx);
                });
            }
        });

        Self {
            by_id: HashMap::new(),
            order: Vec::new(),
            sounds: Vec::new(),
            freshness: Freshness::new(),
            loading: false,
            api,
            _conn_watch: conn_watch,
        }
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
                    if this.update(cx, |this, cx| this.ensure_loaded(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.ensure_loaded_task(cx).detach();
    }

    pub fn ensure_loaded_task(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.freshness.is_fresh(crate::CACHE_TTL) {
            return Task::ready(());
        }
        self.fetch(cx)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx).detach();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.loading {
            return Task::ready(());
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_stickers_by_user_id().await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result.map(|stickers| {
                    let mut sounds = Vec::new();
                    let mut mapped_stickers = Vec::new();
                    for proto in stickers {
                        if proto.media_type == AUDIO_MEDIA_TYPE {
                            if let Some(sound) = sound_from_proto(proto) {
                                sounds.push(sound);
                            }
                        } else if let Some(sticker) = sticker_from_proto(proto) {
                            mapped_stickers.push(sticker);
                        }
                    }
                    (mapped_stickers, sounds)
                }) {
                    Ok((mapped_stickers, sounds)) => {
                        this.by_id.clear();
                        this.order.clear();
                        this.sounds = sounds;
                        for sticker in mapped_stickers {
                            this.insert(sticker);
                        }
                        this.freshness.mark_fetched();
                        tracing::info!(
                            "StickerStore: fetched {} stickers, {} sounds",
                            this.order.len(),
                            this.sounds.len()
                        );
                        cx.emit(StickerEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_stickers_by_user_id failed: {e}"),
                }
            });
        })
    }

    fn handle_sticker_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(message) = event else {
            return;
        };
        let changed = match message {
            realtime::envelope::Message::StickerCreateEvent(e) => self.insert_from_event(e),
            realtime::envelope::Message::StickerUpdateEvent(e) => {
                self.rename(&e.sticker_id.to_string(), &e.shortname)
            }
            realtime::envelope::Message::StickerDeleteEvent(e) => {
                self.remove(&e.sticker_id.to_string())
            }
            _ => false,
        };
        if changed {
            cx.emit(StickerEvent::Changed);
            cx.notify();
        }
    }

    fn insert_from_event(&mut self, e: &realtime::StickerCreateEvent) -> bool {
        if e.sticker_id == 0 || e.source.is_empty() {
            return false;
        }
        if is_audio_source(&e.source) {
            let sound = sound_from_event(e);
            if self.sounds.iter().any(|existing| existing.id == sound.id) {
                return false;
            }
            self.sounds.push(sound);
            return true;
        }
        match sticker_from_event(e) {
            Some(sticker) => {
                self.insert(sticker);
                true
            }
            None => false,
        }
    }

    fn rename(&mut self, id: &str, shortname: &str) -> bool {
        if shortname.is_empty() {
            return false;
        }
        if let Some(sticker) = self.by_id.get_mut(id) {
            sticker.shortname = shortname.to_string();
            return true;
        }
        if let Some(sound) = self.sounds.iter_mut().find(|sound| sound.id == id) {
            sound.shortname = shortname.to_string();
            return true;
        }
        false
    }

    fn remove(&mut self, id: &str) -> bool {
        if self.by_id.remove(id).is_some() {
            self.order.retain(|existing| existing != id);
            return true;
        }
        let before = self.sounds.len();
        self.sounds.retain(|sound| sound.id != id);
        self.sounds.len() != before
    }

    fn insert(&mut self, sticker: Sticker) {
        let id = sticker.id.clone();
        if self.by_id.insert(id.clone(), sticker).is_none() {
            self.order.push(id);
        }
    }

    pub fn all(&self) -> Vec<&Sticker> {
        ordered_stickers(&self.by_id, &self.order).collect()
    }

    pub fn sounds(&self) -> &[ClanSound] {
        &self.sounds
    }

    pub fn sounds_for_clan(&self, clan_id: &str) -> Vec<&ClanSound> {
        self.sounds
            .iter()
            .filter(|sound| sound.clan_id == clan_id)
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&Sticker> {
        self.by_id.get(id)
    }

    pub fn get_sound(&self, id: &str) -> Option<&ClanSound> {
        self.sounds.iter().find(|sound| sound.id == id)
    }

    pub fn for_clan(&self, clan_id: &str) -> Vec<&Sticker> {
        ordered_stickers(&self.by_id, &self.order)
            .filter(|sticker| sticker.clan_id == clan_id)
            .collect()
    }

    pub fn active_clan_first(&self, active_clan_id: Option<&str>) -> Vec<&Sticker> {
        active_clan_first_in(&self.by_id, &self.order, active_clan_id)
    }

    pub fn create_sticker(
        &self,
        clan_id: ClanId,
        path: &Path,
        shortname: &str,
        is_for_sale: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), EmoticonError>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        let shortname = shortname.trim().to_string();
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            if !is_valid_emoticon_shortname(&shortname) {
                return Err(EmoticonErrorKind::InvalidName.into());
            }
            let (id, url) =
                upload_emoticon_file(&api, &path, "stickers", MAX_STICKER_BYTES, is_for_sale)
                    .await?;
            api.add_clan_sticker(
                clan,
                &url,
                &shortname,
                "Among Us",
                id,
                STICKER_MEDIA_TYPE,
                is_for_sale,
            )
            .await
            .map_err(|e| EmoticonError::other(e.to_string()))?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| EmoticonError::other("store dropped"))?;
            Ok(())
        })
    }

    pub fn update_sticker(
        &self,
        sticker_id: &str,
        clan_id: ClanId,
        source: &str,
        shortname: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), EmoticonError>> {
        let api = self.api.clone();
        let id: i64 = match sticker_id.parse() {
            Ok(id) => id,
            Err(_) => {
                return cx.spawn(async move |_, _| Err(EmoticonError::other("invalid sticker id")));
            }
        };
        let shortname = shortname.trim().to_string();
        let source = source.to_string();
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            if !is_valid_emoticon_shortname(&shortname) {
                return Err(EmoticonErrorKind::InvalidName.into());
            }
            api.update_clan_sticker_by_id(id, clan, &source, &shortname, "Among Us")
                .await
                .map_err(|e| EmoticonError::other(e.to_string()))?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| EmoticonError::other("store dropped"))?;
            Ok(())
        })
    }

    pub fn delete_sticker(
        &self,
        sticker_id: &str,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let id: i64 = match sticker_id.parse() {
            Ok(id) => id,
            Err(_) => return cx.spawn(async move |_, _| Err("invalid sticker id".into())),
        };
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            api.delete_clan_sticker_by_id(id, clan)
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| {
                this.by_id.remove(&id.to_string());
                this.order.retain(|existing| existing != &id.to_string());
                this.sounds.retain(|sound| sound.id != id.to_string());
                cx.emit(StickerEvent::Changed);
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn create_sound(
        &self,
        clan_id: ClanId,
        path: &Path,
        shortname: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), EmoticonError>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        let shortname = shortname.trim().to_string();
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            if !is_valid_emoticon_shortname(&shortname) {
                return Err(EmoticonErrorKind::InvalidName.into());
            }
            let (id, url) = upload_sound_file(&api, &path, MAX_SOUND_BYTES)
                .await
                .map_err(EmoticonError::other)?;
            api.add_clan_sticker(
                clan,
                &url,
                &shortname,
                "Among Us",
                id,
                AUDIO_MEDIA_TYPE,
                false,
            )
            .await
            .map_err(|e| EmoticonError::other(e.to_string()))?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| EmoticonError::other("store dropped"))?;
            Ok(())
        })
    }

    pub fn update_sound(
        &self,
        sound_id: &str,
        clan_id: ClanId,
        source: &str,
        shortname: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), EmoticonError>> {
        self.update_sticker(sound_id, clan_id, source, shortname, cx)
    }

    pub fn delete_sound(
        &self,
        sound_id: &str,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.delete_sticker(sound_id, clan_id, cx)
    }
}

fn ordered_stickers<'a>(
    by_id: &'a HashMap<String, Sticker>,
    order: &'a [String],
) -> impl Iterator<Item = &'a Sticker> {
    order.iter().filter_map(move |id| by_id.get(id))
}

fn active_clan_first_in<'a>(
    by_id: &'a HashMap<String, Sticker>,
    order: &'a [String],
    active_clan_id: Option<&str>,
) -> Vec<&'a Sticker> {
    let mut ordered: Vec<&Sticker> = ordered_stickers(by_id, order).collect();
    if let Some(active) = active_clan_id {
        ordered.sort_by_key(|sticker| u8::from(sticker.clan_id != active));
    }
    ordered
}

fn sticker_from_proto(s: api::ClanSticker) -> Option<Sticker> {
    if s.id == 0 || s.source.is_empty() || s.media_type != STICKER_MEDIA_TYPE {
        return None;
    }
    let clan_name = s.clan_name;
    let category = if !s.category.is_empty() {
        s.category
    } else {
        clan_name.clone()
    };
    Some(Sticker {
        id: s.id.to_string(),
        shortname: s.shortname,
        src: s.source,
        category,
        clan_id: s.clan_id.to_string(),
        clan_name,
        logo: s.logo,
        creator_id: s.creator_id.to_string(),
        is_for_sale: s.is_for_sale,
    })
}

fn is_audio_source(source: &str) -> bool {
    let lowered = source.to_ascii_lowercase();
    lowered.ends_with(".mp3") || lowered.ends_with(".wav") || lowered.contains("/sounds/")
}

fn sound_from_event(e: &realtime::StickerCreateEvent) -> ClanSound {
    ClanSound {
        id: e.sticker_id.to_string(),
        shortname: if e.shortname.is_empty() {
            "sound.mp3".to_string()
        } else {
            e.shortname.clone()
        },
        src: e.source.clone(),
        clan_id: e.clan_id.to_string(),
        clan_name: e.clan_name.clone(),
        logo: e.logo.clone(),
        creator_id: e.creator_id.to_string(),
    }
}

fn sticker_from_event(e: &realtime::StickerCreateEvent) -> Option<Sticker> {
    if e.sticker_id == 0 || e.source.is_empty() {
        return None;
    }
    let category = if !e.category.is_empty() {
        e.category.clone()
    } else {
        e.clan_name.clone()
    };
    Some(Sticker {
        id: e.sticker_id.to_string(),
        shortname: e.shortname.clone(),
        src: e.source.clone(),
        category,
        clan_id: e.clan_id.to_string(),
        clan_name: e.clan_name.clone(),
        logo: e.logo.clone(),
        creator_id: e.creator_id.to_string(),
        is_for_sale: false,
    })
}

fn sound_from_proto(s: api::ClanSticker) -> Option<ClanSound> {
    if s.id == 0 || s.source.is_empty() || s.media_type != AUDIO_MEDIA_TYPE {
        return None;
    }
    let shortname = if !s.shortname.is_empty() {
        s.shortname
    } else {
        "sound.mp3".to_string()
    };
    let clan_name = if !s.clan_name.is_empty() {
        s.clan_name
    } else {
        "MY SOUNDS".to_string()
    };
    Some(ClanSound {
        id: s.id.to_string(),
        shortname,
        src: s.source,
        clan_id: s.clan_id.to_string(),
        clan_name,
        logo: s.logo,
        creator_id: s.creator_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto(id: i64, shortname: &str, clan_id: i64, media_type: i32) -> api::ClanSticker {
        api::ClanSticker {
            id,
            source: format!("https://cdn/{id}.webp"),
            shortname: shortname.into(),
            category: String::new(),
            clan_name: "MyClan".into(),
            clan_id,
            media_type,
            ..Default::default()
        }
    }

    fn init_sticker_store(cx: &mut App) -> Entity<StickerStore> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        cx.new(|cx| StickerStore::new(api, cx))
    }

    #[gpui::test]
    fn realtime_events_add_rename_and_remove_a_sticker(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_sticker_store(cx);
            store.update(cx, |store, cx| {
                let created = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerCreateEvent(realtime::StickerCreateEvent {
                        sticker_id: 7,
                        source: "https://cdn/7.webp".into(),
                        shortname: "wave".into(),
                        clan_id: 3,
                        clan_name: "MyClan".into(),
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&created, cx);
                let sticker = store.get("7").expect("sticker inserted");
                assert_eq!(sticker.shortname, "wave");
                assert_eq!(sticker.category, "MyClan");

                let updated = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerUpdateEvent(realtime::StickerUpdateEvent {
                        sticker_id: 7,
                        shortname: "hello".into(),
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&updated, cx);
                assert_eq!(store.get("7").map(|s| s.shortname.as_str()), Some("hello"));

                let deleted = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerDeleteEvent(realtime::StickerDeleteEvent {
                        sticker_id: 7,
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&deleted, cx);
                assert!(store.get("7").is_none());
                assert!(store.all().is_empty());
            });
        });
    }

    #[gpui::test]
    fn a_created_sound_never_lands_in_the_sticker_list(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_sticker_store(cx);
            store.update(cx, |store, cx| {
                for (id, source) in [
                    (11i64, "https://cdn/clip.mp3"),
                    (12, "https://cdn/clip.WAV"),
                    (13, "https://cdn/sounds/clip.ogg"),
                ] {
                    let created =
                        RealtimeEvent::Unhandled(realtime::envelope::Message::StickerCreateEvent(
                            realtime::StickerCreateEvent {
                                sticker_id: id,
                                source: source.into(),
                                shortname: "boom".into(),
                                clan_id: 3,
                                clan_name: "MyClan".into(),
                                ..Default::default()
                            },
                        ));
                    store.handle_sticker_event(&created, cx);
                }
                assert!(store.all().is_empty(), "sounds must stay out of the picker");
                assert_eq!(store.sounds().len(), 3);

                let renamed = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerUpdateEvent(realtime::StickerUpdateEvent {
                        sticker_id: 11,
                        shortname: "bang".into(),
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&renamed, cx);
                assert_eq!(
                    store.get_sound("11").map(|s| s.shortname.as_str()),
                    Some("bang")
                );

                let deleted = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerDeleteEvent(realtime::StickerDeleteEvent {
                        sticker_id: 11,
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&deleted, cx);
                assert_eq!(store.sounds().len(), 2);
            });
        });
    }

    #[gpui::test]
    fn an_event_for_an_unknown_sticker_changes_nothing(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_sticker_store(cx);
            store.update(cx, |store, cx| {
                let updated = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerUpdateEvent(realtime::StickerUpdateEvent {
                        sticker_id: 99,
                        shortname: "ghost".into(),
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&updated, cx);
                assert!(store.all().is_empty());

                let sourceless = RealtimeEvent::Unhandled(
                    realtime::envelope::Message::StickerCreateEvent(realtime::StickerCreateEvent {
                        sticker_id: 8,
                        shortname: "nosrc".into(),
                        ..Default::default()
                    }),
                );
                store.handle_sticker_event(&sourceless, cx);
                assert!(store.all().is_empty());
            });
        });
    }

    fn store_with(stickers: Vec<Sticker>) -> (HashMap<String, Sticker>, Vec<String>) {
        let mut by_id = HashMap::new();
        let mut order = Vec::new();
        for s in stickers {
            let id = s.id.clone();
            if by_id.insert(id.clone(), s).is_none() {
                order.push(id);
            }
        }
        (by_id, order)
    }

    fn sticker(id: &str, clan_id: &str) -> Sticker {
        Sticker {
            id: id.into(),
            shortname: id.into(),
            src: format!("https://cdn/{id}.webp"),
            category: String::new(),
            clan_id: clan_id.into(),
            clan_name: String::new(),
            logo: String::new(),
            creator_id: String::new(),
            is_for_sale: false,
        }
    }

    fn ids(stickers: Vec<&Sticker>) -> Vec<String> {
        stickers.into_iter().map(|s| s.id.clone()).collect()
    }

    #[test]
    fn maps_proto_and_falls_back_to_clan_name_for_category() {
        let mapped = sticker_from_proto(proto(1, "wave", 7, STICKER_MEDIA_TYPE)).unwrap();
        assert_eq!(mapped.id, "1");
        assert_eq!(mapped.shortname, "wave");
        assert_eq!(mapped.src, "https://cdn/1.webp");
        assert_eq!(mapped.category, "MyClan");
        assert_eq!(mapped.clan_id, "7");
        assert_eq!(mapped.clan_name, "MyClan");
    }

    #[test]
    fn skips_audio_and_invalid_stickers() {
        assert!(sticker_from_proto(proto(2, "boom", 7, 1)).is_none());
        assert!(sticker_from_proto(proto(0, "wave", 7, STICKER_MEDIA_TYPE)).is_none());
        let mut no_source = proto(3, "wave", 7, STICKER_MEDIA_TYPE);
        no_source.source = String::new();
        assert!(sticker_from_proto(no_source).is_none());
    }

    #[test]
    fn maps_audio_proto_to_sound_with_fallbacks() {
        let mapped = sound_from_proto(proto(5, "boom", 7, AUDIO_MEDIA_TYPE)).unwrap();
        assert_eq!(mapped.id, "5");
        assert_eq!(mapped.shortname, "boom");
        assert_eq!(mapped.src, "https://cdn/5.webp");
        assert_eq!(mapped.clan_id, "7");
        assert_eq!(mapped.clan_name, "MyClan");

        let mut anonymous = proto(6, "", 0, AUDIO_MEDIA_TYPE);
        anonymous.clan_name = String::new();
        let mapped = sound_from_proto(anonymous).unwrap();
        assert_eq!(mapped.shortname, "sound.mp3");
        assert_eq!(mapped.clan_name, "MY SOUNDS");

        assert!(sound_from_proto(proto(8, "boom", 7, STICKER_MEDIA_TYPE)).is_none());
        assert!(sound_from_proto(proto(0, "boom", 7, AUDIO_MEDIA_TYPE)).is_none());
    }

    #[test]
    fn active_clan_first_orders_active_clan_ahead() {
        let (by_id, order) = store_with(vec![
            sticker("1", "B"),
            sticker("2", "A"),
            sticker("3", "B"),
            sticker("4", "A"),
        ]);
        assert_eq!(
            ids(active_clan_first_in(&by_id, &order, Some("A"))),
            vec!["2", "4", "1", "3"]
        );
        assert_eq!(
            ids(active_clan_first_in(&by_id, &order, None)),
            vec!["1", "2", "3", "4"]
        );
    }
}
