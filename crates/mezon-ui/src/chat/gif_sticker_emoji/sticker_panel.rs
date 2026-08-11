use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, ListAlignment, ListState,
    SharedString, Subscription, Window, div, img, list, prelude::*, px,
};
use mezon_store::{ClanList, StickerEvent, StickerStore};

use crate::components::primitives::{Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const RAIL_W: f32 = 44.;
const PANEL_RADIUS: f32 = 8.;
const ACCENT_BLUE: u32 = 0x5865f2;
const STICKER_COLS: usize = 3;
const STICKER_CELL_PX: f32 = 92.;
const STICKER_IMG_PX: f32 = 80.;
const STICKER_ROW_PX: f32 = 104.;
const HEADER_PX: f32 = 40.;
const PREFETCH_ROWS: usize = 4;

#[derive(Clone)]
struct StickerCell {
    id: SharedString,
    src: SharedString,
    shortname_lc: SharedString,
    cell_id: SharedString,
    img_id: SharedString,
}

struct StickerCategory {
    key: String,
    name_upper: SharedString,
    initial: SharedString,
    rail_id: SharedString,
    header_id: SharedString,
    logo: SharedString,
    stickers: Vec<StickerCell>,
}

enum PickerRow {
    Header { cat_ix: usize, collapsed: bool },
    Stickers(Vec<StickerCell>),
}

pub enum StickerPanelEvent {
    Picked { url: String, filename: String },
}

pub struct StickerPanel {
    query: String,
    categories: Vec<StickerCategory>,
    rows: Vec<PickerRow>,
    header_row: Vec<usize>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    empty_label: SharedString,
    list_state: ListState,
    /// Last row the warm-ahead reached; the list closure runs per row per frame.
    warmed_through: std::cell::Cell<usize>,
    list_dirty: bool,
    image_cache: Entity<LruImageCache>,
    _sub: Option<Subscription>,
}

impl EventEmitter<StickerPanelEvent> for StickerPanel {}

impl StickerPanel {
    pub fn new(locale: String, cx: &mut Context<Self>) -> Self {
        let sub = StickerStore::try_global(cx).map(|store| {
            store.update(cx, |store, cx| store.ensure_loaded(cx));
            cx.subscribe(&store, |this, _store, _event: &StickerEvent, cx| {
                this.rebuild_snapshot(cx);
                cx.notify();
            })
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::sticker_thumbnail(
                "gse-sticker-panel",
                crate::image_cache::STICKER_CACHE_CAPACITY,
                crate::image_cache::STICKER_CACHE_BYTES,
                crate::image_cache::STICKER_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let mut panel = Self {
            query: String::new(),
            categories: Vec::new(),
            rows: Vec::new(),
            header_row: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            empty_label: SharedString::new_static(mezon_i18n::t(
                &locale,
                "chat.stickerPicker.noStickers",
            )),
            list_state: ListState::new(0, ListAlignment::Top, px(200.)),
            warmed_through: std::cell::Cell::new(0),
            list_dirty: true,
            image_cache,
            _sub: sub,
        };
        panel.rebuild_snapshot(cx);
        panel
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.rebuild();
            cx.notify();
        }
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn rebuild_snapshot(&mut self, cx: &App) {
        let active = ClanList::global(cx)
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string());
        let mut categories: Vec<StickerCategory> = Vec::new();
        if let Some(store) = StickerStore::try_global(cx) {
            for sticker in store.read(cx).active_clan_first(active.as_deref()) {
                let item = StickerCell {
                    id: sticker.id.clone().into(),
                    src: sticker.src.clone().into(),
                    shortname_lc: sticker.shortname.to_lowercase().into(),
                    cell_id: SharedString::from(format!("gse-sticker-{}", sticker.id)),
                    img_id: SharedString::from(format!("gse-sticker-img-{}", sticker.id)),
                };
                let label = if !sticker.clan_name.is_empty() {
                    sticker.clan_name.clone()
                } else {
                    sticker.category.clone()
                };
                match categories.iter_mut().find(|c| c.key == sticker.clan_id) {
                    Some(cat) => cat.stickers.push(item),
                    None => {
                        let logo = if sticker.logo.is_empty() {
                            SharedString::default()
                        } else {
                            SharedString::from(crate::util::imgproxy::avatar_url(cx, &sticker.logo))
                        };
                        categories.push(StickerCategory {
                            key: sticker.clan_id.clone(),
                            name_upper: SharedString::from(label.to_uppercase()),
                            initial: SharedString::from(initial_letter(&label)),
                            rail_id: SharedString::from(format!(
                                "gse-sticker-rail-{}",
                                sticker.clan_id
                            )),
                            header_id: SharedString::from(format!(
                                "gse-sticker-cat-{}",
                                sticker.clan_id
                            )),
                            logo,
                            stickers: vec![item],
                        })
                    }
                }
            }
        }
        self.categories = categories;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.warmed_through.set(0);
        let needle = self.query.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut header_row = Vec::new();
        let flat = self.searching();
        for (cat_ix, cat) in self.categories.iter().enumerate() {
            let collapsed = !flat && self.collapsed.contains(&cat.key);
            if !flat {
                header_row.push(rows.len());
                rows.push(PickerRow::Header { cat_ix, collapsed });
                if collapsed {
                    continue;
                }
                for chunk in cat.stickers.chunks(STICKER_COLS) {
                    rows.push(PickerRow::Stickers(chunk.to_vec()));
                }
                continue;
            }
            let matching: Vec<StickerCell> = cat
                .stickers
                .iter()
                .filter(|cell| cell.shortname_lc.contains(&needle))
                .cloned()
                .collect();
            for chunk in matching.chunks(STICKER_COLS) {
                rows.push(PickerRow::Stickers(chunk.to_vec()));
            }
        }
        self.rows = rows;
        self.header_row = header_row;
        self.list_dirty = true;
    }

    /// Stickers a few rows below the one being painted. A cell only starts
    /// fetching once it is on screen, so scrolling into fresh rows shows empty
    /// squares for as long as the request takes; a sticker is a ~2 MB decode
    /// when animated, so that wait is longer here than in the emoji grid.
    fn sources_ahead(&self, row: usize) -> Vec<SharedString> {
        let target = row + PREFETCH_ROWS;
        if self.warmed_through.get() >= target {
            return Vec::new();
        }
        self.warmed_through.set(target);
        match self.rows.get(target) {
            Some(PickerRow::Stickers(cells)) => cells
                .iter()
                .filter(|cell| !cell.src.is_empty())
                .map(|cell| cell.src.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    fn sync_list(&mut self) {
        if self.list_dirty || self.list_state.item_count() != self.rows.len() {
            let scroll = self.list_state.logical_scroll_top();
            self.list_state.reset(self.rows.len());
            if !self.rows.is_empty() && scroll.item_ix > 0 {
                self.list_state.scroll_to(gpui::ListOffset {
                    item_ix: scroll.item_ix.min(self.rows.len() - 1),
                    offset_in_item: scroll.offset_in_item,
                });
            }
            self.list_dirty = false;
        }
    }

    fn toggle_collapse(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.rebuild();
        cx.notify();
    }
}

impl Render for StickerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        let theme = cx.theme().clone();
        let entity = cx.entity();
        let show_rail = !self.searching() && !self.categories.is_empty();

        let rail = show_rail.then(|| {
            let mut rail = div()
                .id("gse-sticker-rail")
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .w(px(RAIL_W))
                .h_full()
                .py_2()
                .px_1()
                .flex_shrink_0()
                .rounded_l(px(PANEL_RADIUS))
                .bg(theme.tokens.bg_active_member_channel)
                .overflow_y_scroll();
            for (cat_ix, cat) in self.categories.iter().enumerate() {
                let ent = entity.clone();
                let active = self.selected.as_deref() == Some(cat.key.as_str());
                let mut btn = div()
                    .id(cat.rail_id.clone())
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded_lg()
                    .cursor_pointer()
                    .overflow_hidden();
                btn = if active {
                    btn.bg(gpui::rgb(ACCENT_BLUE)).shadow_md()
                } else {
                    btn.hover(|s| s.bg(theme.tokens.bg_item_hover))
                };
                btn = btn.child(category_logo(&theme, &cat.logo, &cat.initial, 28.));
                let key = cat.key.clone();
                let btn = btn.on_click(move |_, _, cx| {
                    ent.update(cx, |this, cx| {
                        this.selected = Some(key.clone());
                        if let Some(&row) = this.header_row.get(cat_ix) {
                            this.list_state.scroll_to(gpui::ListOffset {
                                item_ix: row,
                                offset_in_item: px(0.),
                            });
                        }
                        cx.notify();
                    });
                });
                rail = rail.child(btn);
            }
            rail
        });

        self.sync_list();
        let list_entity = entity.clone();
        let list = list(self.list_state.clone(), move |ix, window, cx| {
            let theme = cx.theme().clone();
            let (row, ahead, cache) = {
                let this = list_entity.read(cx);
                let row = match this.rows.get(ix) {
                    Some(PickerRow::Header { cat_ix, collapsed }) => {
                        match this.categories.get(*cat_ix) {
                            Some(cat) => render_header(&theme, cat, *collapsed, &list_entity),
                            None => div().h(px(HEADER_PX)).into_any_element(),
                        }
                    }
                    Some(PickerRow::Stickers(cells)) => {
                        render_sticker_row(&theme, cells, &list_entity)
                    }
                    None => div().h(px(STICKER_ROW_PX)).into_any_element(),
                };
                (row, this.sources_ahead(ix), this.image_cache.clone())
            };
            warm_sticker_sources(&cache, ahead, window, cx);
            row
        })
        .size_full();

        let content: AnyElement = if self.categories.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.tokens.text_theme_primary)
                        .child(self.empty_label.clone()),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .px_2()
                .child(list)
                .into_any_element()
        };

        div()
            .image_cache(self.image_cache.clone())
            .size_full()
            .px_2()
            .pt_2()
            .flex()
            .flex_row()
            .overflow_hidden()
            .when_some(rail, |el, rail| el.child(rail))
            .child(content)
    }
}

fn render_header(
    theme: &Theme,
    cat: &StickerCategory,
    collapsed: bool,
    entity: &Entity<StickerPanel>,
) -> AnyElement {
    let ent = entity.clone();
    let key = cat.key.clone();
    let chevron = if collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    div()
        .id(cat.header_id.clone())
        .h(px(HEADER_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(category_logo(theme, &cat.logo, &cat.initial, 16.))
        .child(
            div()
                .max_w(px(300.))
                .truncate()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(cat.name_upper.clone()),
        )
        .child(
            Icon::new(chevron)
                .size(px(16.))
                .text_color(theme.tokens.text_theme_primary),
        )
        .child(div().h(px(1.)).flex_1().ml_2().bg(theme.border))
        .on_click(move |_, _, cx| {
            ent.update(cx, |this, cx| this.toggle_collapse(key.clone(), cx));
        })
        .into_any_element()
}

fn render_sticker_row(
    theme: &Theme,
    cells: &[StickerCell],
    entity: &Entity<StickerPanel>,
) -> AnyElement {
    let hover_bg = theme.bg_hover;
    let border = theme.border;
    let mut row = div()
        .h(px(STICKER_ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap_3();
    for cell in cells {
        let ent = entity.clone();
        let url = cell.src.clone();
        let filename = cell.id.clone();
        row = row.child(
            div()
                .id(cell.cell_id.clone())
                .flex()
                .items_center()
                .justify_center()
                .size(px(STICKER_CELL_PX))
                .rounded_lg()
                .border_1()
                .border_color(border)
                .cursor_pointer()
                .hover(|s| s.bg(hover_bg))
                .when(!cell.src.is_empty(), |s| {
                    s.child(
                        img(cell.src.clone())
                            .id(cell.img_id.clone())
                            .size(px(STICKER_IMG_PX))
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                })
                .on_click(move |_, _, cx| {
                    let url = url.to_string();
                    let filename = filename.to_string();
                    ent.update(cx, |_this, cx| {
                        cx.emit(StickerPanelEvent::Picked { url, filename })
                    });
                }),
        );
    }
    row.into_any_element()
}

fn category_logo(
    theme: &Theme,
    logo: &SharedString,
    initial: &SharedString,
    size: f32,
) -> AnyElement {
    if !logo.is_empty() {
        return img(logo.clone())
            .size(px(size))
            .rounded_full()
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element();
    }
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded_full()
        .bg(theme.tokens.bg_active_member_channel)
        .text_size(px(size * 0.42))
        .font_weight(FontWeight::BOLD)
        .text_color(theme.tokens.text_theme_primary)
        .child(initial.clone())
        .into_any_element()
}

fn initial_letter(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

/// Decode sticker rows that have not scrolled into view yet. The row being
/// painted is built first, so it takes the pipeline's permits ahead of these.
fn warm_sticker_sources(
    cache: &Entity<LruImageCache>,
    sources: Vec<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    if sources.is_empty() {
        return;
    }
    cache.update(cx, |cache, cx| {
        for src in sources {
            cache.prefetch(&gpui::Resource::Uri(src.to_string().into()), window, cx);
        }
    });
}
