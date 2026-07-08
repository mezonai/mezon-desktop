use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus};
use mezon_proto::api;

use crate::Freshness;
use crate::clan::{ClanEvent, ClanList};

const STICKER_MEDIA_TYPE: i32 = 0;
const AUDIO_MEDIA_TYPE: i32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sticker {
    pub id: String,
    pub shortname: String,
    pub src: String,
    pub category: String,
    pub clan_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClanSound {
    pub id: String,
    pub shortname: String,
    pub src: String,
    pub clan_id: String,
    pub clan_name: String,
    pub logo: String,
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
    _clan_sub: Subscription,
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
        let clan_sub = cx.subscribe(&ClanList::global(cx), |this, _clan, event, cx| {
            if let ClanEvent::ActiveClanChanged(Some(_)) = event {
                this.ensure_loaded(cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            by_id: HashMap::new(),
            order: Vec::new(),
            sounds: Vec::new(),
            freshness: Freshness::new(),
            loading: false,
            api,
            _clan_sub: clan_sub,
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
        if !self.freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_stickers_by_user_id().await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(stickers) => {
                        this.by_id.clear();
                        this.order.clear();
                        this.sounds.clear();
                        for proto in stickers {
                            if proto.media_type == AUDIO_MEDIA_TYPE {
                                if let Some(sound) = sound_from_proto(proto) {
                                    this.sounds.push(sound);
                                }
                            } else if let Some(sticker) = sticker_from_proto(proto) {
                                this.insert(sticker);
                            }
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
        .detach();
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

    pub fn for_clan(&self, clan_id: &str) -> Vec<&Sticker> {
        ordered_stickers(&self.by_id, &self.order)
            .filter(|sticker| sticker.clan_id == clan_id)
            .collect()
    }

    pub fn active_clan_first(&self, active_clan_id: Option<&str>) -> Vec<&Sticker> {
        active_clan_first_in(&self.by_id, &self.order, active_clan_id)
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
    let category = if !s.category.is_empty() {
        s.category
    } else {
        s.clan_name
    };
    Some(Sticker {
        id: s.id.to_string(),
        shortname: s.shortname,
        src: s.source,
        category,
        clan_id: s.clan_id.to_string(),
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
