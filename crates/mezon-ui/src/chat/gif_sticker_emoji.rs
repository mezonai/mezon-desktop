use gpui::{
    AnyElement, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, SharedString, Subscription, UniformListScrollHandle, Window, div, img, prelude::*,
    px, uniform_list,
};
use mezon_store::{ClanList, StickerEvent, StickerStore};

use crate::chat::message::{ReactionPicker, ReactionPickerEvent};
use crate::components::primitives::{Input, InputEvent, InputState};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const STICKER_COLS: usize = 3;
const STICKER_CELL_PX: f32 = 92.;
const STICKER_ROW_PX: f32 = 100.;
const STICKER_IMG_PX: f32 = 80.;
const PANEL_W: f32 = 500.;

#[derive(Clone)]
struct StickerCell {
    id: SharedString,
    src: SharedString,
    lower: String,
}

pub enum StickerPanelEvent {
    Picked { url: String, filename: String },
}

pub struct StickerPanel {
    query: String,
    all: Vec<StickerCell>,
    rows: Vec<Vec<StickerCell>>,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _sub: Option<Subscription>,
}

impl EventEmitter<StickerPanelEvent> for StickerPanel {}

impl StickerPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sub = StickerStore::try_global(cx).map(|store| {
            cx.subscribe(&store, |this, _store, _event: &StickerEvent, cx| {
                this.rebuild(cx);
                cx.notify();
            })
        });
        if let Some(store) = StickerStore::try_global(cx) {
            store.update(cx, |store, cx| store.ensure_loaded(cx));
        }
        let image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "sticker-panel",
                256,
                12 * 1024 * 1024,
                4 * 1024 * 1024,
                cx,
            )
        });
        let mut panel = Self {
            query: String::new(),
            all: Vec::new(),
            rows: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            image_cache,
            _sub: sub,
        };
        panel.rebuild(cx);
        panel
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.regroup();
            cx.notify();
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let active = ClanList::global(cx)
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string());
        self.all = match StickerStore::try_global(cx) {
            Some(store) => store
                .read(cx)
                .active_clan_first(active.as_deref())
                .into_iter()
                .map(|s| StickerCell {
                    id: s.id.clone().into(),
                    src: s.src.clone().into(),
                    lower: s.shortname.to_lowercase(),
                })
                .collect(),
            None => Vec::new(),
        };
        self.regroup();
    }

    fn regroup(&mut self) {
        let needle = self.query.trim().to_lowercase();
        let rows = self
            .all
            .iter()
            .filter(|cell| needle.is_empty() || cell.lower.contains(&needle))
            .cloned()
            .collect::<Vec<_>>();
        self.rows = rows.chunks(STICKER_COLS).map(<[_]>::to_vec).collect();
    }
}

impl Render for StickerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        if self.rows.is_empty() {
            return div()
                .image_cache(self.image_cache.clone())
                .w_full()
                .h(px(STICKER_ROW_PX))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.text_muted)
                        .child("No stickers"),
                )
                .into_any_element();
        }
        let entity = cx.entity();
        let count = self.rows.len();
        let list = uniform_list("sticker-panel-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let this = entity.read(cx);
            range
                .map(|ix| match this.rows.get(ix) {
                    Some(cells) => render_sticker_row(&theme, cells, &entity),
                    None => div().h(px(STICKER_ROW_PX)).into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .h(px(STICKER_ROW_PX * 3.5))
        .w_full();
        div()
            .image_cache(self.image_cache.clone())
            .w_full()
            .child(list)
            .into_any_element()
    }
}

fn render_sticker_row(
    theme: &Theme,
    cells: &[StickerCell],
    entity: &Entity<StickerPanel>,
) -> AnyElement {
    let hover_bg = theme.bg_hover;
    let mut row = div()
        .h(px(STICKER_ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap_2();
    for cell in cells {
        let ent = entity.clone();
        let url = cell.src.to_string();
        let filename = cell.id.to_string();
        let item = div()
            .id(SharedString::from(format!("sticker-{}", cell.id)))
            .flex()
            .items_center()
            .justify_center()
            .size(px(STICKER_CELL_PX))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(hover_bg))
            .when(!cell.src.is_empty(), |s| {
                s.child(img(cell.src.clone()).size(px(STICKER_IMG_PX)))
            })
            .on_click(move |_event, _window, cx| {
                let url = url.clone();
                let filename = filename.clone();
                ent.update(cx, |_this, cx| {
                    cx.emit(StickerPanelEvent::Picked { url, filename })
                });
            });
        row = row.child(item);
    }
    row.into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SubPanel {
    Gifs,
    Stickers,
    Emoji,
    Sounds,
}

pub enum GifStickerEmojiEvent {
    Emoji { emoji_id: String, emoji: String },
    Sticker { url: String, filename: String },
}

pub struct GifStickerEmojiPopup {
    focus_handle: FocusHandle,
    active: SubPanel,
    search: Entity<InputState>,
    emoji: Entity<ReactionPicker>,
    sticker: Entity<StickerPanel>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<GifStickerEmojiEvent> for GifStickerEmojiPopup {}
impl EventEmitter<DismissEvent> for GifStickerEmojiPopup {}

impl Focusable for GifStickerEmojiPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GifStickerEmojiPopup {
    pub fn new(active: SubPanel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .height(px(32.))
        });
        let emoji = cx.new(|cx| ReactionPicker::new_hosted(window, cx));
        let sticker = cx.new(StickerPanel::new);

        let mut subs = Vec::new();
        subs.push(cx.subscribe(&emoji, |_this, _emoji, event, cx| {
            let ReactionPickerEvent::Picked { emoji_id, emoji } = event;
            cx.emit(GifStickerEmojiEvent::Emoji {
                emoji_id: emoji_id.clone(),
                emoji: emoji.clone(),
            });
        }));
        subs.push(cx.subscribe(&sticker, |_this, _sticker, event, cx| {
            let StickerPanelEvent::Picked { url, filename } = event;
            cx.emit(GifStickerEmojiEvent::Sticker {
                url: url.clone(),
                filename: filename.clone(),
            });
        }));
        subs.push(cx.subscribe(&search, |this, _input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let query = this.search.read(cx).value().to_string();
                this.apply_query(query, cx);
            }
        }));

        Self {
            focus_handle,
            active,
            search,
            emoji,
            sticker,
            _subs: subs,
        }
    }

    pub fn active_tab(&self) -> SubPanel {
        self.active
    }

    fn apply_query(&mut self, query: String, cx: &mut Context<Self>) {
        match self.active {
            SubPanel::Emoji => self.emoji.update(cx, |p, cx| p.set_query(query, cx)),
            SubPanel::Stickers => self.sticker.update(cx, |p, cx| p.set_query(query, cx)),
            SubPanel::Gifs | SubPanel::Sounds => {}
        }
    }

    pub fn set_tab(&mut self, tab: SubPanel, cx: &mut Context<Self>) {
        if self.active == tab {
            return;
        }
        self.active = tab;
        self.search.update(cx, |input, cx| input.clear(cx));
        self.emoji
            .update(cx, |p, cx| p.set_query(String::new(), cx));
        self.sticker
            .update(cx, |p, cx| p.set_query(String::new(), cx));
        cx.notify();
    }
}

impl Render for GifStickerEmojiPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entity = cx.entity();

        let mut tab_bar = div().flex().flex_row().gap_2().px_3().pt_3();
        for (tab, label) in [
            (SubPanel::Gifs, "GIFs"),
            (SubPanel::Stickers, "Stickers"),
            (SubPanel::Emoji, "Emojis"),
            (SubPanel::Sounds, "Sounds"),
        ] {
            let active = self.active == tab;
            let ent = entity.clone();
            tab_bar = tab_bar.child(
                div()
                    .id(SharedString::from(format!("gse-tab-{label}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(14.))
                    .when(active, |s| {
                        s.font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                    })
                    .when(!active, |s| s.text_color(theme.text_muted))
                    .child(label)
                    .on_click(move |_event, _window, cx| {
                        ent.update(cx, |this, cx| this.set_tab(tab, cx));
                    }),
            );
        }

        let content = match self.active {
            SubPanel::Emoji => self.emoji.clone().into_any_element(),
            SubPanel::Stickers => self.sticker.clone().into_any_element(),
            SubPanel::Gifs | SubPanel::Sounds => div()
                .w_full()
                .h(px(STICKER_ROW_PX * 3.5))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.text_muted)
                        .child("Coming soon"),
                )
                .into_any_element(),
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(PANEL_W))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.bg_theme_contexify)
            .shadow_lg()
            .child(tab_bar)
            .child(div().px_3().py_2().child(Input::new(&self.search)))
            .child(div().px_2().pb_2().child(content))
    }
}
