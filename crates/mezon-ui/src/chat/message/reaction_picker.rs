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
const CELL_PX: f32 = 40.;
const EMOJI_PX: f32 = 32.;
const ROW_PX: f32 = 44.;
const PANEL_W: f32 = 468.;
const LIST_H: f32 = 352.;
const HOVER_BAR_PX: f32 = 36.;
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
    emojis: Vec<SnapshotEmoji>,
}

enum PickerRow {
    Header { name: SharedString, collapsed: bool },
    Emojis(Vec<PickerEmoji>),
}

struct NavCategory {
    name: SharedString,
    rail_id: SharedString,
    icon: SharedString,
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
    rows: Vec<PickerRow>,
    nav: Vec<NavCategory>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    hover_emoji: Option<(SharedString, SharedString)>,
    embedded_search: bool,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
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
        Self::build(true, window, cx)
    }

    pub fn new_hosted(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(false, window, cx)
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.rebuild();
            cx.notify();
        }
    }

    fn build(embedded_search: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
                this.rebuild_snapshot(cx);
                cx.notify();
            })
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "reaction-picker",
                256,
                12 * 1024 * 1024,
                4 * 1024 * 1024,
                cx,
            )
        });
        let mut picker = Self {
            focus_handle,
            search,
            query: String::new(),
            snapshot: Vec::new(),
            rows: Vec::new(),
            nav: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            hover_emoji: None,
            embedded_search,
            scroll: UniformListScrollHandle::new(),
            image_cache,
            _emoji_sub: emoji_sub,
            _search_sub: search_sub,
        };
        picker.rebuild_snapshot(cx);
        picker
    }

    fn rebuild_snapshot(&mut self, cx: &mut Context<Self>) {
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
                    let items: Vec<SnapshotEmoji> = emojis
                        .iter()
                        .map(|e| SnapshotEmoji {
                            emoji: PickerEmoji {
                                emoji_id: e.id.clone().into(),
                                emoji: e.shortname.clone().into(),
                                src: crate::util::imgproxy::emoji_url(cx, &e.id).into(),
                                cell_id: SharedString::from(format!("react-pick-{}", e.id)),
                            },
                            lower: e.shortname.to_lowercase(),
                        })
                        .collect();
                    let icon = items
                        .first()
                        .map(|e| e.emoji.src.clone())
                        .unwrap_or_default();
                    CategorySnapshot {
                        name,
                        rail_id,
                        icon,
                        emojis: items,
                    }
                })
                .collect(),
            None => Vec::new(),
        };
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let query = self.query.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut nav = Vec::new();

        if !query.is_empty() {
            for cat in &self.snapshot {
                nav.push(NavCategory {
                    name: cat.name.clone(),
                    rail_id: cat.rail_id.clone(),
                    icon: cat.icon.clone(),
                    row_index: 0,
                });
            }
            let mut current: Vec<PickerEmoji> = Vec::new();
            for cat in &self.snapshot {
                for e in &cat.emojis {
                    if !e.lower.contains(&query) {
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
                    row_index: rows.len(),
                });
                rows.push(PickerRow::Header {
                    name: cat.name.clone(),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entity = cx.entity();
        let searching = !self.query.trim().is_empty();

        let rail = (!self.nav.is_empty()).then(|| {
            let mut rail = div()
                .id("reaction-rail")
                .flex()
                .flex_col()
                .gap_1()
                .w(px(RAIL_W))
                .h(px(LIST_H))
                .py_1()
                .px_1()
                .bg(theme.bg_tertiary)
                .rounded_tl_lg()
                .overflow_y_scroll();
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
                    .size(px(36.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover));
                if active {
                    btn = btn.bg(theme.bg_hover);
                }
                if let Some(icon) = rail_icon {
                    btn = btn.child(Icon::new(icon).size(px(24.)).text_color(theme.text_muted));
                } else if !icon_src.is_empty() {
                    btn = btn.child(
                        img(icon_src)
                            .size(px(24.))
                            .with_fallback(emoji_error_fallback(px(24.), theme.text_muted)),
                    );
                }
                let btn = btn.on_click(move |_, _, cx| {
                    if searching {
                        return;
                    }
                    ent.update(cx, |this, cx| {
                        this.selected = Some(cat.clone());
                        this.scroll.scroll_to_item(idx, ScrollStrategy::Top);
                        cx.notify();
                    });
                });
                rail = rail.child(btn);
            }
            rail
        });

        let count = self.rows.len();
        let list_entity = entity.clone();
        let list = uniform_list("reaction-picker-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let this = list_entity.read(cx);
            range
                .map(|ix| match this.rows.get(ix) {
                    Some(PickerRow::Header { name, collapsed }) => {
                        render_header(&theme, name, *collapsed, &list_entity)
                    }
                    Some(PickerRow::Emojis(emojis)) => {
                        render_emoji_row(&theme, emojis, &list_entity)
                    }
                    None => div().h(px(ROW_PX)).into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .h(px(LIST_H))
        .flex_1();

        let body = div()
            .flex()
            .flex_row()
            .h(px(LIST_H))
            .children(rail)
            .child(div().flex_1().child(list));

        let hover_bar = div()
            .w_full()
            .h(px(HOVER_BAR_PX))
            .mt_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pl_1()
            .bg(theme.bg_tertiary)
            .rounded_md()
            .when_some(self.hover_emoji.clone(), |el, (src, name)| {
                el.when(!src.is_empty(), |el| {
                    el.child(
                        img(src)
                            .max_w(px(28.))
                            .max_h(px(28.))
                            .with_fallback(emoji_error_fallback(px(28.), theme.text_muted)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .text_color(theme.text_primary)
                        .truncate()
                        .child(name),
                )
            });

        if !self.embedded_search {
            return div()
                .image_cache(self.image_cache.clone())
                .w_full()
                .flex()
                .flex_col()
                .child(body)
                .child(hover_bar)
                .into_any_element();
        }

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .image_cache(self.image_cache.clone())
            .w(px(PANEL_W))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.bg_theme_contexify)
            .shadow_lg()
            .p_2()
            .child(div().w_full().pb_2().child(Input::new(&self.search)))
            .child(body)
            .child(hover_bar)
            .into_any_element()
    }
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
    div()
        .id(SharedString::from(format!("reaction-cat-{}", name)))
        .h(px(ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_1()
        .cursor_pointer()
        .child(
            Icon::new(chevron)
                .size(px(16.))
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(name.to_uppercase()),
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
) -> AnyElement {
    let hover_bg = theme.bg_hover;
    let text_muted = theme.text_muted;
    let mut row = div().h(px(ROW_PX)).flex().flex_row().items_center().gap_1();
    for emoji in emojis {
        let emoji_id = emoji.emoji_id.clone();
        let shortname = emoji.emoji.clone();
        let ent = entity.clone();
        let hover_ent = entity.clone();
        let hover_name = emoji.emoji.clone();
        let hover_src = emoji.src.clone();
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
            cell = cell.child(
                img(emoji.src.clone())
                    .size(px(EMOJI_PX))
                    .with_fallback(emoji_error_fallback(px(EMOJI_PX), text_muted)),
            );
        }
        let cell = cell
            .on_hover(move |hovered, _window, cx| {
                if *hovered {
                    let name = hover_name.clone();
                    let src = hover_src.clone();
                    hover_ent.update(cx, |this, cx| this.set_hover_emoji(src, name, cx));
                }
            })
            .on_click(move |_event, _window, cx| {
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
