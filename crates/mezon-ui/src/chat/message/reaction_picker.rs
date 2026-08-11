use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, div,
    img, prelude::*, px, uniform_list,
};
use mezon_store::{ClanList, EmojiEvent, EmojiStore};

use super::reaction_detail::emoji_error_fallback;
use crate::components::primitives::{Icon, IconName, Input, InputEvent, InputState};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const RAIL_W: f32 = 44.;
const RAIL_TILE_PX: f32 = 36.;
const RAIL_ICON_PX: f32 = 28.;
const ACCENT_BLUE: u32 = 0x5865f2;
const CELL_PX: f32 = 36.;
const EMOJI_PX: f32 = 32.;
const ROW_PX: f32 = 48.;
const PREFETCH_ROWS: usize = 8;
const EMOJI_ROW_GAP: f32 = 12.;
const PANEL_W: f32 = 500.;
const PANEL_MAX_H: f32 = 512.;
const PANEL_MIN_H: f32 = 400.;
const PANEL_VIEWPORT_INSET: f32 = 88.;
const HOVER_BAR_PX: f32 = 36.;
const HEADER_ICON_PX: f32 = 16.;
const COLS: usize = 9;
const SEARCH_PLACEHOLDER: &str = "Find the perfect reaction";

#[derive(Clone)]
struct PickerEmoji {
    emoji_id: SharedString,
    emoji: SharedString,
    src: SharedString,
    cell_id: SharedString,
}

/// A pre-lowercased emoji kept in the cached snapshot so per-keystroke search
/// filters over `&str` without re-fetching/re-lowercasing the whole corpus.
struct SnapshotEmoji {
    emoji: PickerEmoji,
    lower: String,
}

struct CategorySnapshot {
    name: SharedString,
    rail_id: SharedString,
    icon: SharedString,
    initial: SharedString,
    emojis: Vec<SnapshotEmoji>,
}

enum PickerRow {
    Header {
        name: SharedString,
        icon: SharedString,
        initial: SharedString,
        collapsed: bool,
    },
    Emojis(Vec<PickerEmoji>),
}

struct NavCategory {
    name: SharedString,
    rail_id: SharedString,
    icon: SharedString,
    initial: SharedString,
    row_index: usize,
}

pub enum ReactionPickerEvent {
    Picked { emoji_id: String, emoji: String },
}

pub struct ReactionPicker {
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    query: String,
    snapshot: Vec<CategorySnapshot>,
    snapshot_stale: bool,
    rows: Vec<PickerRow>,
    nav: Vec<NavCategory>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    hover_emoji: Option<(SharedString, SharedString)>,
    /// The cell the pointer is over. Only that one animates: a grid is scanned
    /// rather than watched, and animating every visible cell uploads all 24
    /// frames of each to the sprite atlas and redraws the window at the rate of
    /// whichever emoji is fastest. Animating what the pointer is on keeps the
    /// motion where the eye is for a fraction of the cost.
    hovered_cell: Option<SharedString>,
    embedded_search: bool,
    fill_container: bool,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    /// Last row the warm-ahead reached. The list closure runs every frame while
    /// scrolling, and re-walking the same rows to re-request images already in
    /// flight is pure per-frame cost.
    warmed_through: std::cell::Cell<usize>,
    _emoji_sub: Option<Subscription>,
    _search_sub: Subscription,
}

impl EventEmitter<ReactionPickerEvent> for ReactionPicker {}
impl EventEmitter<DismissEvent> for ReactionPicker {}

impl Focusable for ReactionPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ReactionPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(true, false, window, cx)
    }

    pub fn new_in_container(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(true, true, window, cx)
    }

    pub fn new_hosted(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(false, false, window, cx)
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.rebuild();
            cx.notify();
        }
    }

    fn build(
        embedded_search: bool,
        fill_container: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(SEARCH_PLACEHOLDER)
                .height(px(32.))
        });
        let search_sub = cx.subscribe(&search, |this, _input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = this.search.read(cx).value().to_string();
                this.rebuild();
                cx.notify();
            }
        });
        let emoji_sub = EmojiStore::try_global(cx).map(|store| {
            cx.subscribe(&store, |this, _store, _event: &EmojiEvent, cx| {
                this.snapshot_stale = true;
                cx.notify();
            })
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::icon_thumbnail(
                "reaction-picker",
                crate::image_cache::EMOJI_PICKER_CACHE_CAPACITY,
                crate::image_cache::EMOJI_PICKER_CACHE_BYTES,
                crate::image_cache::ICON_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let mut picker = Self {
            focus_handle,
            search,
            query: String::new(),
            snapshot: Vec::new(),
            snapshot_stale: false,
            rows: Vec::new(),
            nav: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            hover_emoji: None,
            hovered_cell: None,
            embedded_search,
            fill_container,
            scroll: UniformListScrollHandle::new(),
            image_cache,
            warmed_through: std::cell::Cell::new(0),
            _emoji_sub: emoji_sub,
            _search_sub: search_sub,
        };
        picker.rebuild_snapshot(cx);
        picker
    }

    /// Emoji just past the bottom of the viewport. A cell only starts fetching
    /// once it is painted, so scrolling into fresh rows shows empty squares for
    /// as long as the request takes; warming the next rows means the images are
    /// already decoded by the time they scroll in.
    fn set_hovered_cell(&mut self, emoji_id: Option<SharedString>, cx: &mut Context<Self>) {
        if self.hovered_cell != emoji_id {
            self.hovered_cell = emoji_id;
            cx.notify();
        }
    }

    fn sources_below(&self, first_unseen_row: usize) -> Vec<SharedString> {
        if self.warmed_through.get() == first_unseen_row {
            return Vec::new();
        }
        self.warmed_through.set(first_unseen_row);
        self.rows
            .iter()
            .skip(first_unseen_row)
            .take(PREFETCH_ROWS)
            .filter_map(|row| match row {
                PickerRow::Emojis(emojis) => Some(emojis),
                PickerRow::Header { .. } => None,
            })
            .flatten()
            .filter(|emoji| !emoji.src.is_empty())
            .map(|emoji| emoji.src.clone())
            .collect()
    }

    fn rebuild_snapshot(&mut self, cx: &mut Context<Self>) {
        self.snapshot_stale = false;
        let active = ClanList::global(cx)
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string());
        self.snapshot = match EmojiStore::try_global(cx) {
            Some(store) => store
                .read(cx)
                .by_category(active.as_deref())
                .into_iter()
                .map(|(category, emojis)| {
                    let name = SharedString::from(category);
                    let rail_id = SharedString::from(format!("rail-{name}"));
                    let clan_logo = emojis
                        .first()
                        .map(|e| e.clan_logo.clone())
                        .filter(|logo| !logo.is_empty());
                    let items: Vec<SnapshotEmoji> = emojis
                        .iter()
                        .map(|e| SnapshotEmoji {
                            emoji: PickerEmoji {
                                emoji_id: e.id.clone().into(),
                                emoji: e.shortname.clone().into(),
                                src: crate::util::imgproxy::emoji_url(cx, &e.id).into(),
                                cell_id: SharedString::from(format!(
                                    "react-pick-{}-{}",
                                    name, e.id
                                )),
                            },
                            lower: e.shortname.to_lowercase(),
                        })
                        .collect();
                    let icon = match clan_logo {
                        Some(logo) => SharedString::from(crate::util::imgproxy::proxied(
                            cx, &logo, 100, 100, "fill",
                        )),
                        None => SharedString::default(),
                    };
                    let initial = SharedString::from(
                        name.chars()
                            .next()
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_default(),
                    );
                    CategorySnapshot {
                        name,
                        rail_id,
                        icon,
                        initial,
                        emojis: items,
                    }
                })
                .collect(),
            None => Vec::new(),
        };
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.warmed_through.set(0);
        let query = self.query.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut nav = Vec::new();

        if !query.is_empty() {
            for cat in &self.snapshot {
                nav.push(NavCategory {
                    name: cat.name.clone(),
                    rail_id: cat.rail_id.clone(),
                    icon: cat.icon.clone(),
                    initial: cat.initial.clone(),
                    row_index: 0,
                });
            }
            let mut current: Vec<PickerEmoji> = Vec::new();
            let mut seen: HashSet<SharedString> = HashSet::new();
            for cat in &self.snapshot {
                for e in &cat.emojis {
                    if !e.lower.contains(&query) {
                        continue;
                    }
                    if !seen.insert(e.emoji.emoji_id.clone()) {
                        continue;
                    }
                    current.push(e.emoji.clone());
                    if current.len() == COLS {
                        rows.push(PickerRow::Emojis(std::mem::take(&mut current)));
                    }
                }
            }
            if !current.is_empty() {
                rows.push(PickerRow::Emojis(current));
            }
        } else {
            for cat in &self.snapshot {
                let collapsed = self.collapsed.contains(cat.name.as_ref());
                nav.push(NavCategory {
                    name: cat.name.clone(),
                    rail_id: cat.rail_id.clone(),
                    icon: cat.icon.clone(),
                    initial: cat.initial.clone(),
                    row_index: rows.len(),
                });
                rows.push(PickerRow::Header {
                    name: cat.name.clone(),
                    icon: cat.icon.clone(),
                    initial: cat.initial.clone(),
                    collapsed,
                });
                if collapsed {
                    continue;
                }
                let mut current: Vec<PickerEmoji> = Vec::new();
                for se in &cat.emojis {
                    current.push(se.emoji.clone());
                    if current.len() == COLS {
                        rows.push(PickerRow::Emojis(std::mem::take(&mut current)));
                    }
                }
                if !current.is_empty() {
                    rows.push(PickerRow::Emojis(current));
                }
            }
        }

        self.rows = rows;
        self.nav = nav;
    }

    fn toggle_collapse(&mut self, category: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&category) {
            self.collapsed.insert(category);
        }
        self.rebuild();
        cx.notify();
    }

    fn set_hover_emoji(&mut self, src: SharedString, name: SharedString, cx: &mut Context<Self>) {
        if self
            .hover_emoji
            .as_ref()
            .is_none_or(|(_, current)| current != &name)
        {
            self.hover_emoji = Some((src, name));
            cx.notify();
        }
    }
}

impl Render for ReactionPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.snapshot_stale {
            self.rebuild_snapshot(cx);
        }
        let theme = cx.theme();
        let bg_tertiary = theme.bg_tertiary;
        let text_muted = theme.text_muted;
        let text_primary = theme.text_primary;
        let border_color = theme.border;
        let bg_contexify = theme.tokens.bg_theme_contexify;
        let rail_bg = theme.tokens.bg_active_member_channel;
        let rail_item_hover = theme.tokens.bg_item_hover;
        let rail_icon_color = theme.tokens.text_theme_primary;
        let entity = cx.entity();
        let searching = !self.query.trim().is_empty();
        let hosted = !self.embedded_search;

        let rail = (!self.nav.is_empty()).then(|| {
            let mut rail = div()
                .id("reaction-rail")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .w(px(RAIL_W))
                .py_2()
                .px_1()
                .bg(rail_bg)
                .overflow_y_scroll();
            rail = if hosted {
                rail.h_full().rounded_l(px(8.))
            } else {
                rail.h_full().rounded_tl_lg()
            };
            for nav in &self.nav {
                let ent = entity.clone();
                let idx = nav.row_index;
                let cat = nav.name.to_string();
                let active = self.selected.as_deref() == Some(nav.name.as_ref());
                let icon_src = nav.icon.clone();
                let rail_icon = category_rail_icon(nav.name.as_ref());
                let mut btn = div()
                    .id(nav.rail_id.clone())
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(RAIL_TILE_PX))
                    .rounded_lg()
                    .cursor_pointer();
                if active {
                    btn = btn.bg(gpui::rgb(ACCENT_BLUE)).shadow_md();
                } else {
                    btn = btn.hover(move |s| s.bg(rail_item_hover));
                }
                if let Some(icon) = rail_icon {
                    btn = btn.child(
                        Icon::new(icon)
                            .size(px(RAIL_ICON_PX))
                            .text_color(rail_icon_color),
                    );
                } else {
                    btn = btn.child(category_logo(theme, &icon_src, &nav.initial, RAIL_ICON_PX));
                }
                let btn = btn.on_click(move |_, _, cx| {
                    if searching {
                        return;
                    }
                    ent.update(cx, |this, cx| {
                        this.selected = Some(cat.clone());
                        this.scroll.scroll_to_item_strict(idx, ScrollStrategy::Top);
                        cx.notify();
                    });
                });
                rail = rail.child(btn);
            }
            rail
        });

        let count = self.rows.len();
        let list_entity = entity.clone();
        let track_hover = self.embedded_search;
        let list = uniform_list("reaction-picker-list", count, move |range, window, cx| {
            let theme = cx.theme().clone();
            let (rows, ahead, cache) = {
                let this = list_entity.read(cx);
                let rows = range
                    .clone()
                    .map(|ix| match this.rows.get(ix) {
                        Some(PickerRow::Header {
                            name,
                            icon,
                            initial,
                            collapsed,
                        }) => render_header(&theme, name, icon, initial, *collapsed, &list_entity),
                        Some(PickerRow::Emojis(emojis)) => render_emoji_row(
                            &theme,
                            emojis,
                            &list_entity,
                            track_hover,
                            this.hovered_cell.as_ref(),
                        ),
                        None => div().h(px(ROW_PX)).into_any_element(),
                    })
                    .collect::<Vec<_>>();
                let ahead = this.sources_below(range.end);
                (rows, ahead, this.image_cache.clone())
            };
            warm_emoji_sources(&cache, ahead, window, cx);
            rows
        })
        .track_scroll(&self.scroll)
        .flex_1();

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .children(rail)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .pl_2()
                    .pr_1()
                    .child(list),
            );

        if !self.embedded_search {
            return div()
                .image_cache(self.image_cache.clone())
                .size_full()
                .px_2()
                .pt_2()
                .flex()
                .flex_col()
                .child(body)
                .into_any_element();
        }

        let panel_h = (window.viewport_size().height - px(PANEL_VIEWPORT_INSET))
            .min(px(PANEL_MAX_H))
            .max(px(PANEL_MIN_H));

        let hover_bar = div()
            .w_full()
            .h(px(HOVER_BAR_PX))
            .mt_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pl_1()
            .bg(bg_tertiary)
            .rounded_md()
            .when_some(self.hover_emoji.clone(), |el, (src, name)| {
                el.when(!src.is_empty(), |el| {
                    el.child(
                        img(src)
                            .id("picker-hover-emoji-frames")
                            .max_w(px(28.))
                            .max_h(px(28.))
                            .with_fallback(emoji_error_fallback(px(28.), text_muted)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .text_color(text_primary)
                        .truncate()
                        .child(name),
                )
            });

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .image_cache(self.image_cache.clone())
            .when(self.fill_container, |el| el.w_full().h_full())
            .when(!self.fill_container, |el| el.w(px(PANEL_W)).h(panel_h))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(border_color)
            .bg(bg_contexify)
            .when(!self.fill_container, |el| el.shadow_lg())
            .p_2()
            .child(div().w_full().pb_2().child(Input::new(&self.search)))
            .child(body)
            .child(hover_bar)
            .into_any_element()
    }
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
            .with_fallback(emoji_error_fallback(px(size), theme.text_muted))
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

fn category_rail_icon(name: &str) -> Option<IconName> {
    Some(match name {
        "Recent" => IconName::EmojiCatStar,
        "Frequently" => IconName::ClockIcon,
        "People" => IconName::Smile,
        "Nature" => IconName::EmojiCatLeaf,
        "Food" => IconName::EmojiCatBowl,
        "Activities" => IconName::EmojiCatGame,
        "Travel" => IconName::EmojiCatBicycle,
        "Objects" => IconName::EmojiCatObject,
        "Symbols" => IconName::EmojiCatHeart,
        "Flags" => IconName::EmojiCatRibbon,
        _ => return None,
    })
}

fn render_header(
    theme: &Theme,
    name: &SharedString,
    icon: &SharedString,
    initial: &SharedString,
    collapsed: bool,
    entity: &Entity<ReactionPicker>,
) -> AnyElement {
    let ent = entity.clone();
    let cat = name.to_string();
    let chevron = if collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    let category_icon: AnyElement = match category_rail_icon(name.as_ref()) {
        Some(icon) => Icon::new(icon)
            .size(px(HEADER_ICON_PX))
            .text_color(theme.text_muted)
            .into_any_element(),
        None => category_logo(theme, icon, initial, HEADER_ICON_PX),
    };
    div()
        .id(SharedString::from(format!("reaction-cat-{}", name)))
        .h(px(ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .px_1()
        .cursor_pointer()
        .child(category_icon)
        .child(
            div()
                .ml_2()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .truncate()
                .child(name.to_uppercase()),
        )
        .child(
            div().ml_1().flex().items_center().child(
                Icon::new(chevron)
                    .size(px(16.))
                    .text_color(theme.text_muted),
            ),
        )
        .on_click(move |_, _, cx| {
            ent.update(cx, |this, cx| this.toggle_collapse(cat.clone(), cx));
        })
        .into_any_element()
}

fn render_emoji_row(
    theme: &Theme,
    emojis: &[PickerEmoji],
    entity: &Entity<ReactionPicker>,
    track_hover: bool,
    hovered_cell: Option<&SharedString>,
) -> AnyElement {
    let hover_bg = theme.bg_hover;
    let text_muted = theme.text_muted;
    let mut row = div()
        .h(px(ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(EMOJI_ROW_GAP));
    for emoji in emojis {
        let emoji_id = emoji.emoji_id.clone();
        let shortname = emoji.emoji.clone();
        let ent = entity.clone();
        let mut cell = div()
            .id(emoji.cell_id.clone())
            .flex()
            .items_center()
            .justify_center()
            .size(px(CELL_PX))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(hover_bg));
        if !emoji.src.is_empty() {
            let image = img(emoji.src.clone())
                .size(px(EMOJI_PX))
                .object_fit(gpui::ObjectFit::Contain)
                .with_fallback(emoji_error_fallback(px(EMOJI_PX), text_muted));
            cell = cell.child(if hovered_cell == Some(&emoji.emoji_id) {
                image.id("picker-emoji-frames").into_any_element()
            } else {
                image.into_any_element()
            });
        }
        let hover_ent = entity.clone();
        let hover_name = emoji.emoji.clone();
        let hover_src = emoji.src.clone();
        let hover_id = emoji.emoji_id.clone();
        cell = cell.on_hover(move |hovered, _window, cx| {
            let entered = *hovered;
            let id = entered.then(|| hover_id.clone());
            let name = hover_name.clone();
            let src = hover_src.clone();
            hover_ent.update(cx, |this, cx| {
                this.set_hovered_cell(id, cx);
                if entered && track_hover {
                    this.set_hover_emoji(src, name, cx);
                }
            });
        });
        let cell = cell.on_click(move |_event, _window, cx| {
            let emoji_id = emoji_id.to_string();
            let emoji = shortname.to_string();
            ent.update(cx, |_this, cx| {
                cx.emit(ReactionPickerEvent::Picked { emoji_id, emoji });
            });
        });
        row = row.child(cell);
    }
    row.into_any_element()
}

/// Ask the picker's cache to decode sources that are not on screen yet. Visible
/// rows are built before this runs, so they take the pipeline's permits first.
fn warm_emoji_sources(
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
