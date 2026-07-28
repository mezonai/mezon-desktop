use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Context, SharedString, Task, Window,
    div, img, prelude::*, px, rgb,
};
use mezon_store::{ChannelId, DirectKind, DirectMessageStore, NotificationSettingStore};

use crate::components::primitives::Avatar;
use crate::router::{Route, navigate};
use crate::util::assets::AVATAR_GROUP;

use super::ClanSidebar;

const REMOVAL_DEFER_MS: u64 = 200;
const ROW_HEIGHT: f32 = 60.;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DirectUnreadItem {
    pub channel_id: ChannelId,
    pub label: SharedString,
    pub kind: DirectKind,
    pub unread_count: u32,
    pub avatar_src: SharedString,
    pub avatar_raw: SharedString,
    pub row_id: SharedString,
    pub anim_key: SharedString,
}

pub(super) struct DirectUnreadListState {
    live: Rc<Vec<DirectUnreadItem>>,
    live_ids: HashSet<ChannelId>,
    render: Rc<Vec<DirectUnreadItem>>,
    defer_task: Option<Task<()>>,
}

impl DirectUnreadListState {
    pub(super) fn new(items: Vec<DirectUnreadItem>) -> Self {
        let items = Rc::new(items);
        let live_ids = items.iter().map(|i| i.channel_id).collect();
        Self {
            live: items.clone(),
            live_ids,
            render: items,
            defer_task: None,
        }
    }

    pub(super) fn sync(&mut self, live: Vec<DirectUnreadItem>, cx: &mut Context<ClanSidebar>) {
        let live = Rc::new(live);
        let prev_len = self.render.len();
        let new_len = live.len();

        let render_ids: HashSet<ChannelId> = self.render.iter().map(|i| i.channel_id).collect();
        let live_ids: HashSet<ChannelId> = live.iter().map(|i| i.channel_id).collect();
        let has_new_ids = live.iter().any(|i| !render_ids.contains(&i.channel_id));

        self.live = live.clone();
        self.live_ids = live_ids;

        if (self.render.is_empty() && new_len > 0) || new_len > prev_len || has_new_ids {
            self.defer_task = None;
            self.render = live;
            cx.notify();
            return;
        }

        if self.render == live {
            return;
        }

        if render_ids == self.live_ids {
            self.defer_task = None;
            self.render = live;
            cx.notify();
            return;
        }

        self.defer_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(REMOVAL_DEFER_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.direct_unread.apply_deferred_render();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn apply_deferred_render(&mut self) {
        self.render = self.live.clone();
        self.defer_task = None;
    }

    pub(super) fn render(&self) -> impl IntoElement {
        if self.render.is_empty() {
            return div().into_any_element();
        }

        let live_ids = &self.live_ids;

        div()
            .id("direct-unread-list")
            .max_h(px(256.))
            .overflow_y_scroll()
            .p(px(2.))
            .flex()
            .flex_col()
            .items_center()
            .w_full()
            .children(self.render.iter().map(|item| {
                let animate_out = !live_ids.contains(&item.channel_id);
                render_direct_unread_item(item, animate_out)
            }))
            .into_any_element()
    }
}

pub(super) fn direct_unread_fingerprint(store: &DirectMessageStore, cx: &App) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    fn fold(hash: u64, bytes: &[u8]) -> u64 {
        bytes
            .iter()
            .fold(hash, |h, b| (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME))
    }
    let noti = NotificationSettingStore::try_global(cx);
    let muted = noti.as_ref().map(|s| s.read(cx));
    store
        .channels()
        .iter()
        .filter(|ch| ch.counts_toward_unread_badge(muted.is_some_and(|s| s.is_time_muted(ch.id))))
        .fold(FNV_OFFSET, |h, ch| {
            let h = fold(h, &ch.id.0.to_le_bytes());
            let h = fold(h, &ch.unread_count.to_le_bytes());
            let h = fold(h, &[ch.kind as u8]);
            let h = fold(h, ch.label.as_bytes());
            fold(h, ch.avatar.as_bytes())
        })
}

pub(super) fn build_direct_unread_items(
    store: &DirectMessageStore,
    cx: &App,
) -> Vec<DirectUnreadItem> {
    let noti = NotificationSettingStore::try_global(cx);
    let muted = noti.as_ref().map(|s| s.read(cx));
    store
        .channels()
        .iter()
        .filter(|ch| ch.counts_toward_unread_badge(muted.is_some_and(|s| s.is_time_muted(ch.id))))
        .map(|ch| DirectUnreadItem {
            channel_id: ch.id,
            label: SharedString::from(ch.label.clone()),
            kind: ch.kind,
            unread_count: ch.unread_count,
            avatar_src: SharedString::from(crate::util::imgproxy::avatar_url(cx, &ch.avatar)),
            avatar_raw: SharedString::from(ch.avatar.clone()),
            row_id: SharedString::from(format!("direct-unread-{}", ch.id)),
            anim_key: SharedString::from(format!("direct-unread-anim-{}", ch.id)),
        })
        .collect()
}

fn badge_text(count: u32) -> SharedString {
    if count >= 100 {
        SharedString::from("99+")
    } else {
        SharedString::from(count.to_string())
    }
}

fn on_direct_unread_click(
    channel_id: ChannelId,
    channel_type: i32,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        navigate(
            cx,
            Route::DirectMessage {
                direct_id: channel_id,
                message_type: channel_type.to_string(),
            },
        );
    }
}

fn render_direct_unread_item(item: &DirectUnreadItem, animate_out: bool) -> AnyElement {
    let channel_id = item.channel_id;
    let channel_type = item.kind.channel_type();
    let unread_count = item.unread_count;
    let badge = badge_text(unread_count);
    let wide = unread_count >= 10;
    let ease = |t: f32| 1.0 - (1.0 - t).powi(2);

    let avatar_size = px(40.);
    let avatar = if item.kind == DirectKind::Group && item.avatar_src.is_empty() {
        img(AVATAR_GROUP)
            .size(avatar_size)
            .rounded(px(8.))
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element()
    } else {
        let mut avatar = Avatar::new().name(item.label.clone()).size_px(avatar_size);
        if !item.avatar_src.is_empty() {
            avatar = avatar.src(item.avatar_src.clone());
            if !item.avatar_raw.is_empty() && item.avatar_raw != item.avatar_src {
                avatar = avatar.fallback_src(item.avatar_raw.clone());
            }
        } else if !item.avatar_raw.is_empty() {
            avatar = avatar.src(item.avatar_raw.clone());
        }
        div()
            .size(avatar_size)
            .rounded(px(8.))
            .overflow_hidden()
            .child(avatar)
            .into_any_element()
    };

    let badge_el = div()
        .absolute()
        .bottom(px(-1.))
        .right(px(-2.))
        .flex()
        .items_center()
        .justify_center()
        .h(px(16.))
        .when(wide, |s| s.w(px(22.)))
        .when(!wide, |s| s.w(px(16.)))
        .rounded_full()
        .bg(rgb(0xDA373C))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(gpui::white())
        .child(badge);

    let inner: AnyElement = if animate_out {
        div()
            .relative()
            .child(avatar)
            .child(badge_el)
            .with_animation(
                SharedString::from(format!("{}-fade-out", item.anim_key)),
                Animation::new(Duration::from_millis(REMOVAL_DEFER_MS)).with_easing(ease),
                |el, delta| el.opacity(1.0 - delta),
            )
            .into_any_element()
    } else {
        div()
            .relative()
            .child(avatar)
            .child(badge_el)
            .into_any_element()
    };

    let row = div()
        .id(item.row_id.clone())
        .flex()
        .items_end()
        .justify_center()
        .w_full()
        .h(px(ROW_HEIGHT))
        .overflow_hidden()
        .cursor_pointer()
        .on_click(on_direct_unread_click(channel_id, channel_type))
        .child(inner);

    if animate_out {
        row.with_animation(
            SharedString::from(format!("{}-height-out", item.anim_key)),
            Animation::new(Duration::from_millis(REMOVAL_DEFER_MS)).with_easing(ease),
            |el, delta| el.h(px(ROW_HEIGHT * (1.0 - delta).max(0.0))),
        )
        .into_any_element()
    } else {
        row.into_any_element()
    }
}
