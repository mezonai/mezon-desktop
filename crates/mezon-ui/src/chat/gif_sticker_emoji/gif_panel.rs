use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, SharedString, Subscription,
    UniformListScrollHandle, Window, div, img, prelude::*, px, uniform_list,
};
use mezon_store::{Gif, GifStore};

use crate::components::primitives::{Icon, IconName};
use crate::image_cache::{LruImageCache, VIEWER_IMAGE_ENTRY_MAX_BYTES};
use crate::theme::{ActiveTheme, Theme};

const PANEL_H: f32 = 400.;
const GIF_COLS: usize = 3;
const GIF_CELL_PX: f32 = 100.;
const GIF_ROW_PX: f32 = 104.;
const CAT_COLS: usize = 2;
const CAT_TILE_PX: f32 = 128.;
const CAT_ROW_PX: f32 = 136.;
const GIF_CACHE_CAPACITY: usize = 64;
const GIF_CACHE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone)]
struct GifCell {
    url: SharedString,
    preview: SharedString,
    cell_id: SharedString,
    img_id: SharedString,
    width: u32,
    height: u32,
}

#[derive(Clone)]
enum CatCell {
    Trending {
        label: SharedString,
        image: SharedString,
    },
    Category {
        name: SharedString,
        searchterm: SharedString,
        image: SharedString,
        tile_id: SharedString,
        img_id: SharedString,
    },
}

enum GridRow {
    Gifs(Vec<GifCell>),
    Categories(Vec<CatCell>),
}

pub enum GifPanelEvent {
    Picked {
        url: String,
        width: u32,
        height: u32,
    },
    SetSearch {
        term: String,
    },
}

pub struct GifPanel {
    locale: String,
    query: String,
    show_featured: bool,
    rows: Vec<GridRow>,
    row_height: f32,
    searching_label: SharedString,
    no_gifs_label: SharedString,
    categories_label: SharedString,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _sub: Option<Subscription>,
}

impl EventEmitter<GifPanelEvent> for GifPanel {}

impl GifPanel {
    pub fn new(locale: String, cx: &mut Context<Self>) -> Self {
        let sub = GifStore::try_global(cx).map(|store| {
            store.update(cx, |store, cx| store.ensure_loaded(cx));
            cx.observe(&store, |this, _store, cx| {
                this.rebuild(cx);
                cx.notify();
            })
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::viewer(
                "gse-gif-panel",
                GIF_CACHE_CAPACITY,
                GIF_CACHE_BYTES,
                VIEWER_IMAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let mut panel = Self {
            searching_label: SharedString::new_static(mezon_i18n::t(
                &locale,
                "searchMessageChannel.searching",
            )),
            no_gifs_label: SharedString::new_static(mezon_i18n::t(
                &locale,
                "chat.gifPicker.noGifs",
            )),
            categories_label: SharedString::new_static(mezon_i18n::t(
                &locale,
                "chat.gifPicker.categories",
            )),
            locale,
            query: String::new(),
            show_featured: false,
            rows: Vec::new(),
            row_height: CAT_ROW_PX,
            scroll: UniformListScrollHandle::new(),
            image_cache,
            _sub: sub,
        };
        panel.rebuild(cx);
        panel
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query == query {
            return;
        }
        self.query = query.clone();
        if !query.trim().is_empty() {
            self.show_featured = false;
        }
        if let Some(store) = GifStore::try_global(cx) {
            store.update(cx, |store, cx| store.search(query, cx));
        }
        self.rebuild(cx);
        cx.notify();
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn rebuild(&mut self, cx: &App) {
        let Some(store) = GifStore::try_global(cx) else {
            self.rows = Vec::new();
            self.row_height = CAT_ROW_PX;
            return;
        };
        let store = store.read(cx);
        if self.searching() || self.show_featured {
            let gifs: Vec<GifCell> = if self.searching() {
                store.search_results()
            } else {
                store.featured()
            }
            .iter()
            .map(gif_cell)
            .collect();
            self.rows = gifs
                .chunks(GIF_COLS)
                .map(|chunk| GridRow::Gifs(chunk.to_vec()))
                .collect();
            self.row_height = GIF_ROW_PX;
        } else {
            let trending =
                SharedString::new_static(mezon_i18n::t(&self.locale, "chat.gifPicker.trending"));
            let mut items: Vec<CatCell> = vec![CatCell::Trending {
                label: trending,
                image: store
                    .featured()
                    .first()
                    .map(|gif| gif.preview_url.clone())
                    .unwrap_or_default(),
            }];
            items.extend(store.categories().iter().map(|category| CatCell::Category {
                name: category.name.clone(),
                searchterm: category.searchterm.clone(),
                image: category.image.clone(),
                tile_id: SharedString::from(format!("gse-gif-cat-{}", category.searchterm)),
                img_id: SharedString::from(format!("gse-gif-cat-img-{}", category.searchterm)),
            }));
            self.rows = items
                .chunks(CAT_COLS)
                .map(|chunk| GridRow::Categories(chunk.to_vec()))
                .collect();
            self.row_height = CAT_ROW_PX;
        }
    }
}

impl Render for GifPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        let theme = cx.theme();
        let entity = cx.entity();
        let searching = self.searching();
        let is_searching =
            GifStore::try_global(cx).is_some_and(|store| store.read(cx).is_searching());
        let is_loading_featured =
            GifStore::try_global(cx).is_some_and(|store| store.read(cx).is_featured_loading());

        let empty_label: Option<SharedString> = self.rows.is_empty().then(|| {
            if (searching && is_searching) || (self.show_featured && is_loading_featured) {
                self.searching_label.clone()
            } else {
                self.no_gifs_label.clone()
            }
        });

        let back_bar = (searching || self.show_featured).then(|| {
            let ent = entity.clone();
            div()
                .id("gse-gif-back")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_1()
                .pt_2()
                .pb_2()
                .cursor_pointer()
                .child(
                    Icon::new(IconName::BackToCategoriesGif)
                        .size(px(20.))
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(self.categories_label.clone()),
                )
                .on_click(move |_, _, cx| {
                    ent.update(cx, |this, cx| {
                        this.show_featured = false;
                        cx.emit(GifPanelEvent::SetSearch {
                            term: String::new(),
                        });
                        this.rebuild(cx);
                        cx.notify();
                    });
                })
        });

        let body: AnyElement = if let Some(label) = empty_label {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.tokens.text_secondary)
                        .child(label),
                )
                .into_any_element()
        } else {
            let list_entity = entity.clone();
            uniform_list(
                "gse-gif-list",
                self.rows.len(),
                move |range, _window, cx| {
                    let theme = cx.theme().clone();
                    let this = list_entity.read(cx);
                    range
                        .map(|ix| match this.rows.get(ix) {
                            Some(GridRow::Gifs(cells)) => {
                                render_gif_row(&theme, cells, &list_entity)
                            }
                            Some(GridRow::Categories(cells)) => {
                                render_category_row(&theme, cells, &list_entity)
                            }
                            None => div().h(px(this.row_height)).into_any_element(),
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .into_any_element()
        };

        div()
            .image_cache(self.image_cache.clone())
            .w_full()
            .h(px(PANEL_H))
            .flex()
            .flex_col()
            .px_2()
            .when_some(back_bar, |el, bar| el.child(bar))
            .child(body)
    }
}

fn gif_cell(gif: &Gif) -> GifCell {
    GifCell {
        cell_id: SharedString::from(format!("gse-gif-{}", gif.url)),
        img_id: SharedString::from(format!("gse-gif-img-{}", gif.url)),
        url: gif.url.clone(),
        preview: gif.preview_url.clone(),
        width: gif.width,
        height: gif.height,
    }
}

fn render_gif_row(theme: &Theme, cells: &[GifCell], entity: &Entity<GifPanel>) -> AnyElement {
    let hover_bg = theme.bg_hover;
    let mut row = div()
        .w_full()
        .h(px(GIF_ROW_PX))
        .flex()
        .flex_row()
        .items_start()
        .gap_1();
    for cell in cells {
        let ent = entity.clone();
        let url = cell.url.clone();
        let (width, height) = (cell.width, cell.height);
        row = row.child(
            div()
                .id(cell.cell_id.clone())
                .flex_1()
                .h(px(GIF_CELL_PX))
                .rounded_lg()
                .overflow_hidden()
                .bg(hover_bg)
                .cursor_pointer()
                .child(
                    img(cell.preview.clone())
                        .id(cell.img_id.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .on_click(move |_, _, cx| {
                    let url = url.to_string();
                    ent.update(cx, |_this, cx| {
                        cx.emit(GifPanelEvent::Picked { url, width, height })
                    });
                }),
        );
    }
    for _ in cells.len()..GIF_COLS {
        row = row.child(div().flex_1().h(px(GIF_CELL_PX)));
    }
    row.into_any_element()
}

fn render_category_row(theme: &Theme, cells: &[CatCell], entity: &Entity<GifPanel>) -> AnyElement {
    let mut row = div()
        .w_full()
        .h(px(CAT_ROW_PX))
        .flex()
        .flex_row()
        .items_start()
        .gap_2();
    for cell in cells {
        row = row.child(render_category_tile(theme, cell, entity));
    }
    for _ in cells.len()..CAT_COLS {
        row = row.child(div().flex_1().h(px(CAT_TILE_PX)));
    }
    row.into_any_element()
}

type CatTile = (
    SharedString,
    Option<(SharedString, SharedString)>,
    SharedString,
    CatAction,
);

fn render_category_tile(theme: &Theme, cell: &CatCell, entity: &Entity<GifPanel>) -> AnyElement {
    let ent = entity.clone();
    let (label, image, id, action): CatTile = match cell {
        CatCell::Trending { label, image } => (
            label.clone(),
            (!image.is_empty()).then(|| {
                (
                    SharedString::new_static("gse-gif-cat-img-trending"),
                    image.clone(),
                )
            }),
            SharedString::new_static("gse-gif-cat-trending"),
            CatAction::Trending,
        ),
        CatCell::Category {
            name,
            searchterm,
            image,
            tile_id,
            img_id,
        } => (
            name.clone(),
            (!image.is_empty()).then(|| (img_id.clone(), image.clone())),
            tile_id.clone(),
            CatAction::Search(searchterm.clone()),
        ),
    };
    let mut tile = div()
        .id(id)
        .relative()
        .flex_1()
        .h(px(CAT_TILE_PX))
        .rounded_md()
        .overflow_hidden()
        .bg(theme.bg_tertiary)
        .cursor_pointer();
    if let Some((img_id, source)) = image {
        tile = tile.child(
            img(source)
                .id(img_id)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(gpui::ObjectFit::Cover),
        );
    }
    tile = tile.child(div().absolute().inset_0().bg(gpui::black()).opacity(0.6));
    let mut label_row = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .gap_2();
    if matches!(cell, CatCell::Trending { .. }) {
        label_row = label_row.child(
            Icon::new(IconName::TrendingGifs)
                .size(px(20.))
                .text_color(gpui::white()),
        );
    }
    tile = tile.child(
        label_row.child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::white())
                .child(label),
        ),
    );
    tile.on_click(move |_, _, cx| {
        let action = action.clone();
        ent.update(cx, |this, cx| match action {
            CatAction::Trending => {
                this.show_featured = true;
                if let Some(store) = GifStore::try_global(cx) {
                    store.update(cx, |store, cx| {
                        store.ensure_loaded(cx);
                        store.ensure_featured(cx);
                    });
                }
                this.rebuild(cx);
                cx.notify();
            }
            CatAction::Search(term) => {
                let term = term.to_string();
                this.query = term.clone();
                this.show_featured = false;
                if let Some(store) = GifStore::try_global(cx) {
                    store.update(cx, |store, cx| store.search_now(term.clone(), cx));
                }
                this.rebuild(cx);
                cx.notify();
                cx.emit(GifPanelEvent::SetSearch { term });
            }
        });
    })
    .into_any_element()
}

#[derive(Clone)]
enum CatAction {
    Trending,
    Search(SharedString),
}
