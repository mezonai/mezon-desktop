use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, FontWeight, SharedString, UniformListScrollHandle, Window, div, img,
    prelude::*, px, uniform_list,
};
use mezon_store::{ChannelId, DirectKind, DirectMessageStore, Settings};

use crate::components::compositions::DmRow;
use crate::components::primitives::{Icon, IconName};
use crate::router::{Route, Router, navigate};
use crate::theme::{ActiveTheme, Theme};

const SCROLL_HOVER_RELEASE_MS: u64 = 150;

#[derive(PartialEq)]
struct DmItem {
    channel_id: ChannelId,
    id: SharedString,
    label: SharedString,
    kind: DirectKind,
    unread: bool,
    online: bool,
    avatar_src: SharedString,
    avatar_raw: SharedString,
}

pub struct DirectSidebar {
    settings: Entity<Settings>,
    list_scroll: UniformListScrollHandle,
    dm_items: Rc<Vec<DmItem>>,
    pending_rebuild: bool,
    suppress_hover: bool,
    last_scroll_at: Option<Instant>,
    image_cache: Entity<crate::image_cache::LruImageCache>,
}

fn is_dm_route(cx: &App) -> bool {
    matches!(
        Router::global(cx).read(cx).route(),
        Route::Direct | Route::DirectMessage { .. } | Route::Friends
    )
}

fn build_dm_items(store: &DirectMessageStore, cx: &App) -> Rc<Vec<DmItem>> {
    Rc::new(
        store
            .channels()
            .iter()
            .map(|ch| DmItem {
                channel_id: ch.id,
                id: SharedString::from(ch.id.to_string()),
                label: SharedString::from(ch.label.clone()),
                kind: ch.kind,
                unread: ch.is_unread(),
                online: ch.online,
                avatar_src: SharedString::from(crate::util::imgproxy::avatar_url(cx, &ch.avatar)),
                avatar_raw: SharedString::from(ch.avatar.clone()),
            })
            .collect(),
    )
}

impl DirectSidebar {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let direct_store = DirectMessageStore::global(cx);

        cx.observe(&direct_store, |this, store, cx| {
            if is_dm_route(cx) {
                let items = build_dm_items(store.read(cx), cx);
                if this.dm_items != items {
                    this.dm_items = items;
                    cx.notify();
                }
            } else {
                this.pending_rebuild = true;
            }
        })
        .detach();
        cx.observe(&Router::global(cx), |this, _, cx| {
            if this.pending_rebuild && is_dm_route(cx) {
                this.pending_rebuild = false;
                let store = DirectMessageStore::global(cx);
                this.dm_items = build_dm_items(store.read(cx), cx);
            }
            cx.notify();
        })
        .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        let dm_items = build_dm_items(direct_store.read(cx), cx);

        Self {
            settings,
            list_scroll: UniformListScrollHandle::new(),
            dm_items,
            pending_rebuild: false,
            suppress_hover: false,
            last_scroll_at: None,
            image_cache: cx.new(|cx| {
                crate::image_cache::LruImageCache::avatar_thumbnail_small(
                    "dm-list",
                    512,
                    12 * 1024 * 1024,
                    4 * 1024 * 1024,
                    cx,
                )
            }),
        }
    }

    fn on_scroll(&mut self, cx: &mut Context<Self>) {
        self.last_scroll_at = Some(Instant::now());
        if self.suppress_hover {
            return;
        }
        self.suppress_hover = true;
        cx.notify();
    }

    fn on_mouse_move_release(&mut self, cx: &mut Context<Self>) {
        if self.suppress_hover
            && self
                .last_scroll_at
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(SCROLL_HOVER_RELEASE_MS))
        {
            self.suppress_hover = false;
            cx.notify();
        }
    }

    fn render_search(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        div()
            .w_full()
            .h(px(50.))
            .px_3()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w_full()
                    .h(px(36.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .rounded_lg()
                    .bg(theme.tokens.bg_tertiary)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(locale, "clan.findOrStartConversation")),
                    ),
            )
    }

    fn render_friends_button(&self, theme: &Theme, locale: &str, active: bool) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        div()
            .id("dm-friends")
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .py_2()
            .px_3()
            .rounded_lg()
            .cursor_pointer()
            .when(active, |this| this.bg(bg_hover))
            .hover(move |this| this.bg(bg_hover))
            .on_click(|_, _window, cx| navigate(cx, Route::Friends))
            .child(img("icons/icon-friends.svg").size(px(20.)).flex_none())
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(locale, "directMessage.friends")),
            )
    }

    fn render_section_header(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .pt_4()
            .pb_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(mezon_i18n::t(locale, "directMessage.directMessages")),
            )
            .child(
                div()
                    .id("dm-create")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(bg_hover))
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(16.))
                            .text_color(theme.text_muted),
                    ),
            )
    }
}

impl Render for DirectSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("DirectSidebar");
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();

        let count = self.dm_items.len();
        let active_id = match Router::global(cx).read(cx).route() {
            Route::DirectMessage { direct_id, .. } => Some(direct_id),
            _ => None,
        };
        let items = self.dm_items.clone();
        let suppress_hover = self.suppress_hover;
        let image_cache = self.image_cache.clone();

        let list = uniform_list("dm-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let active_id = active_id;
            range
                .map(|ix| match items.get(ix) {
                    Some(item) => {
                        let selected = active_id == Some(item.channel_id);
                        DmRow::new(item.id.clone(), item.label.clone(), item.kind)
                            .selected(selected)
                            .unread(item.unread)
                            .online(item.online)
                            .avatar_src(item.avatar_src.clone())
                            .avatar_raw(item.avatar_raw.clone())
                            .suppress_hover(suppress_hover)
                            .image_cache(image_cache.clone())
                            .render(&theme)
                            .into_any_element()
                    }
                    None => div().into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .on_scroll_wheel(cx.listener(|this, _event, _window, cx| this.on_scroll(cx)))
        .on_mouse_move(cx.listener(|this, _event, _window, cx| this.on_mouse_move_release(cx)))
        .flex_1()
        .min_h_0()
        .px_2();

        let on_friends = matches!(Router::global(cx).read(cx).route(), Route::Friends);

        div()
            .flex()
            .flex_col()
            .size_full()
            .pb(px(68.))
            .bg(theme.bg_secondary)
            .child(self.render_search(theme, &locale))
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .child(self.render_friends_button(theme, &locale, on_friends)),
            )
            .child(self.render_section_header(theme, &locale))
            .child(list)
    }
}
