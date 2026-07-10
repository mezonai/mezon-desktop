use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SCROLL_HOVER_RELEASE_MS: u64 = 150;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, ListState, MouseButton,
    MouseDownEvent, SharedString, Subscription, Task, WeakEntity, Window, div, ease_in_out, list,
    prelude::*, px,
};
use mezon_store::{
    ChannelList, ClanId, ClanList, ClanMembersStore, FAVOR_CATE_ID, Settings, VoiceMember,
};

use crate::clan::clan_menu::{build_clan_menu, clan_menu_overlay};
use crate::components::compositions::channel_row::{channel_type_icon, shows_left_unread_nub};
use crate::components::compositions::channel_row_element::{
    ChannelRowBadge, ChannelRowElement, ThreadConnector,
};
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size, context_menu_at};
use crate::theme::{ActiveTheme, Theme};

mod items;
mod menu;
mod skeleton;
use items::{AppChannelSlot, SidebarItem, VoiceMemberSlot};
use menu::{OpenMenu, build_channel_menu, on_category_click, on_channel_click};

fn resolve_voice_member_slot(
    cx: &App,
    clan_id: Option<ClanId>,
    m: &VoiceMember,
) -> VoiceMemberSlot {
    let mut slot = VoiceMemberSlot::from(m);
    if let Some(clan_id) = clan_id
        && let Some(store) = ClanMembersStore::try_global(cx)
        && let Some(member) = store.read(cx).member(clan_id, m.user_id)
    {
        let name = member.name();
        if !name.is_empty() {
            slot.display_name = name.to_string();
        }
        let avatar = member.avatar();
        if !avatar.is_empty() {
            slot.avatar_url = avatar.to_string();
        }
    }
    if !slot.avatar_url.is_empty() {
        slot.avatar_url = crate::util::imgproxy::avatar_url(cx, &slot.avatar_url);
    }
    slot
}

pub struct ChannelSidebar {
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    settings: Entity<Settings>,
    items: Rc<Vec<SidebarItem>>,
    list_state: ListState,
    active_clan_name: String,
    active_clan_name_upper: String,
    active_clan_id: Option<ClanId>,
    loaded_clans: HashSet<ClanId>,
    channel_list_handle: Entity<ChannelList>,
    open_menu: Option<OpenMenu>,
    pub(super) clan_menu_open: bool,
    skeleton_phase: skeleton::Phase,
    skeleton_clan: Option<ClanId>,
    _skeleton_timer: Option<Task<()>>,
    suppress_hover: bool,
    last_scroll_at: Option<Instant>,
    _clan_observe: Subscription,
    _channel_observe: Subscription,
    _settings_observe: Subscription,
    _router_observe: Subscription,
    _members_observe: Subscription,
}

impl ChannelSidebar {
    pub fn new(
        clan_list: Entity<ClanList>,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel_list_handle = channel_list.clone();

        let list_state = ListState::new(0, gpui::ListAlignment::Top, px(32.));
        let weak = cx.weak_entity();
        list_state.set_scroll_handler(move |_event, _window, cx| {
            weak.update(cx, |this, cx| {
                this.last_scroll_at = Some(Instant::now());
                if !this.suppress_hover {
                    this.suppress_hover = true;
                    cx.notify();
                }
            })
            .ok();
        });

        let clan_observe = cx.observe(&clan_list, |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let channel_observe = cx.observe(&channel_list, |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let settings_observe = cx.observe(&settings, |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let router_observe = cx.observe(&crate::router::Router::global(cx), |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });
        let members_observe = cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
            if this.rebuild_items(cx) {
                cx.notify();
            }
        });

        let mut this = Self {
            clan_list,
            channel_list,
            settings,
            items: Rc::new(Vec::new()),
            list_state,
            active_clan_name: String::new(),
            active_clan_name_upper: String::new(),
            active_clan_id: None,
            loaded_clans: HashSet::new(),
            channel_list_handle,
            open_menu: None,
            clan_menu_open: false,
            skeleton_phase: skeleton::Phase::default(),
            skeleton_clan: None,
            _skeleton_timer: None,
            suppress_hover: false,
            last_scroll_at: None,
            _clan_observe: clan_observe,
            _channel_observe: channel_observe,
            _settings_observe: settings_observe,
            _router_observe: router_observe,
            _members_observe: members_observe,
        };
        this.rebuild_items(cx);
        this
    }

    fn rebuild_items(&mut self, cx: &mut Context<Self>) -> bool {
        let locale = self.settings.read(cx).language.clone();
        let clans = self.clan_list.read(cx);
        let channels = self.channel_list.read(cx);

        let new_clan_id = clans.active_clan_id;
        let clan_changed = self.active_clan_id != new_clan_id;
        if clan_changed {
            self.clan_menu_open = false;
            self.suppress_hover = false;
        }

        let new_clan_name = clans
            .active_clan()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| mezon_i18n::t(&locale, "sidebar.selectClan").to_string());
        let name_changed = new_clan_name != self.active_clan_name;
        if name_changed {
            self.active_clan_name_upper = new_clan_name.to_uppercase();
        }
        self.active_clan_name = new_clan_name;
        self.active_clan_id = new_clan_id;

        let active_channel_id = match crate::router::Router::global(cx).read(cx).route() {
            crate::router::Route::Channel { channel_id, .. }
            | crate::router::Route::Thread { channel_id, .. }
            | crate::router::Route::Canvas { channel_id, .. } => Some(channel_id),
            _ => None,
        };
        let mut items = Vec::new();

        let app_channels: Vec<AppChannelSlot> = clans
            .active_clan_id
            .as_ref()
            .map(|cid| {
                channels
                    .app_channels_for_clan(*cid)
                    .iter()
                    .map(AppChannelSlot::from)
                    .collect()
            })
            .unwrap_or_default();

        items.push(SidebarItem::BannerAndEvents {
            banner_url: clans.active_clan_banner().map(|s| s.to_string()),
            app_channels,
        });

        let cold_loading = if let Some(clan_id) = clans.active_clan_id.as_ref() {
            let categories = channels.categories_for_clan(*clan_id);
            if categories.is_empty() {
                !self.loaded_clans.contains(clan_id)
            } else {
                self.loaded_clans.insert(*clan_id);
                for category in categories {
                    if !channels.is_show_empty_category(*clan_id)
                        && category.channels.is_empty()
                        && category.id != FAVOR_CATE_ID
                    {
                        continue;
                    }
                    let is_favorites = category.id == FAVOR_CATE_ID;
                    let collapsed = channels.is_category_collapsed(*clan_id, &category.id);
                    let name = if is_favorites {
                        mezon_i18n::t(&locale, "channelList.favoriteChannel").to_string()
                    } else {
                        category.name.clone()
                    };
                    items.push(SidebarItem::Category {
                        elem_id: SharedString::from(format!("cat-{}", &category.id)),
                        id: category.id.clone(),
                        name_upper: name.to_uppercase(),
                        collapsed,
                    });
                    if !collapsed {
                        let ch_slice = &category.channels;
                        for (idx, ch) in ch_slice.iter().enumerate() {
                            let is_thread = !is_favorites && ch.parent_id.is_some();
                            let (line_above, line_below) = if is_thread {
                                let pid = ch.parent_id;
                                let has_prev = ch_slice[..idx].iter().any(|c| c.parent_id == pid);
                                let has_next =
                                    ch_slice[idx + 1..].iter().any(|c| c.parent_id == pid);
                                (has_prev, has_next)
                            } else {
                                (false, false)
                            };
                            let badge_count = ch.badge_count;
                            let badge_label = if badge_count > 99 {
                                SharedString::from("99+")
                            } else if badge_count > 0 {
                                SharedString::from(badge_count.to_string())
                            } else {
                                SharedString::from("")
                            };
                            items.push(SidebarItem::Channel {
                                elem_id: SharedString::from(format!(
                                    "ch-{}-{}",
                                    &category.id, &ch.id
                                )),
                                id: ch.id.to_string(),
                                name: truncate_channel_label(&ch.name),
                                channel_type: ch.channel_type,
                                unread: ch.is_unread(),
                                private: ch.private,
                                selected: active_channel_id == Some(ch.id),
                                badge_count,
                                badge_label,
                                muted: ch.muted,
                                is_thread,
                                line_above,
                                line_below,
                                voice_members: ch
                                    .voice_members
                                    .iter()
                                    .map(|m| resolve_voice_member_slot(cx, new_clan_id, m))
                                    .collect(),
                            });
                        }
                    }
                }
                false
            }
        } else {
            true
        };

        let new_count = items.len();
        let old_count = self.items.len();
        let items_changed = *self.items != items;
        self.items = Rc::new(items);

        if clan_changed || old_count == 0 {
            self.list_state.reset(new_count);
        } else if new_count > old_count {
            self.list_state
                .splice(old_count..old_count, new_count - old_count);
        } else if new_count < old_count {
            self.list_state.splice(new_count..old_count, 0);
        }

        let skeleton_changed = self.advance_skeleton(cold_loading, new_clan_id, cx);
        items_changed || name_changed || clan_changed || skeleton_changed
    }

    fn advance_skeleton(
        &mut self,
        loading: bool,
        clan: Option<ClanId>,
        cx: &mut Context<Self>,
    ) -> bool {
        let same_clan = self.skeleton_clan == clan;
        let (next, timer) = skeleton::transition(
            self.skeleton_phase,
            skeleton::Input::Sync { loading, same_clan },
        );
        self.skeleton_clan = clan;
        let changed = next != self.skeleton_phase;
        self.skeleton_phase = next;
        self.arm_skeleton_timer(timer, clan, cx);
        changed
    }

    fn arm_skeleton_timer(
        &mut self,
        timer: skeleton::Timer,
        clan: Option<ClanId>,
        cx: &mut Context<Self>,
    ) {
        match skeleton::timer_action(timer) {
            skeleton::TimerAction::Keep => {}
            skeleton::TimerAction::Clear => self._skeleton_timer = None,
            skeleton::TimerAction::Arm(input, delay_ms) => {
                self._skeleton_timer = Some(Self::spawn_skeleton_timer(input, delay_ms, clan, cx));
            }
        }
    }

    fn spawn_skeleton_timer(
        input: skeleton::Input,
        delay_ms: u64,
        clan: Option<ClanId>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
            this.update(cx, |this, cx| this.on_skeleton_timer(input, clan, cx))
                .ok();
        })
    }

    fn on_skeleton_timer(
        &mut self,
        input: skeleton::Input,
        clan: Option<ClanId>,
        cx: &mut Context<Self>,
    ) {
        if self.skeleton_clan != clan {
            return;
        }
        let (next, timer) = skeleton::transition(self.skeleton_phase, input);
        let changed = next != self.skeleton_phase;
        self.skeleton_phase = next;
        self.arm_skeleton_timer(timer, clan, cx);
        if changed {
            cx.notify();
        }
    }

    fn skeleton_anim_id(&self, prefix: &'static str) -> SharedString {
        match self.skeleton_clan {
            Some(id) => SharedString::from(format!("{prefix}-{id}")),
            None => SharedString::from(prefix),
        }
    }

    pub(crate) fn dismiss_clan_menu(&mut self, cx: &mut Context<Self>) {
        if self.clan_menu_open {
            self.clan_menu_open = false;
            cx.notify();
        }
    }
}

impl Render for ChannelSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("ChannelSidebar");
        let theme = cx.theme();
        let items = self.items.clone();
        let channel_list_handle = self.channel_list_handle.clone();
        let active_clan_id_for_nav = self.active_clan_id;
        let suppress_hover = self.suppress_hover;
        let list_state = self.list_state.clone();
        let sidebar = cx.entity().downgrade();
        let sidebar_for_channel_menu = sidebar.clone();
        let sidebar_for_clan_menu = sidebar.clone();
        let channel_list_for_clan_menu = self.channel_list_handle.clone();
        let locale = self.settings.read(cx).language.clone();
        let menu_overlay = self.open_menu.as_ref().map(|menu| {
            (
                menu.position,
                menu.channel_type,
                menu.is_thread,
                locale.clone(),
            )
        });
        let clan_menu_data = self.clan_menu_open.then(|| {
            (
                self.active_clan_id,
                self.channel_list
                    .read(cx)
                    .is_show_empty_category(self.active_clan_id.unwrap_or(ClanId(0))),
                locale.clone(),
            )
        });

        let list_element = list(list_state, {
            let sidebar = sidebar.clone();
            move |ix, _window, cx| {
                render_sidebar_item(
                    &items,
                    ix,
                    cx,
                    channel_list_handle.clone(),
                    active_clan_id_for_nav,
                    sidebar.clone(),
                    suppress_hover,
                )
            }
        })
        .size_full();

        let skeleton_overlay = match self.skeleton_phase {
            skeleton::Phase::Showing => Some(
                sidebar_skeleton_layer(theme, cx)
                    .with_animation(
                        self.skeleton_anim_id("sidebar-skeleton-in"),
                        Animation::new(Duration::from_millis(skeleton::FADE_IN_MS))
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(delta),
                    )
                    .into_any_element(),
            ),
            skeleton::Phase::Settling => Some(sidebar_skeleton_layer(theme, cx).into_any_element()),
            skeleton::Phase::FadingOut => Some(
                sidebar_skeleton_layer(theme, cx)
                    .with_animation(
                        self.skeleton_anim_id("sidebar-skeleton-out"),
                        Animation::new(Duration::from_millis(skeleton::FADE_OUT_MS))
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(1.0 - delta),
                    )
                    .into_any_element(),
            ),
            skeleton::Phase::Hidden | skeleton::Phase::Pending => None,
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .pb(px(68.))
            .bg(theme.bg_secondary)
            .child({
                let sidebar = sidebar.clone();
                let sidebar_for_menu = sidebar_for_clan_menu.clone();
                let channel_list_for_menu = channel_list_for_clan_menu.clone();
                let clan_name = self.active_clan_name_upper.clone();
                let has_clan = self.active_clan_id.is_some();
                div()
                    .relative()
                    .w_full()
                    .h(px(50.))
                    .border_b_1()
                    .border_color(theme.border)
                    .when(has_clan, |el| {
                        el.child(
                            div()
                                .w_full()
                                .h_full()
                                .px_3()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.bg_hover))
                                .on_mouse_down(MouseButton::Left, {
                                    let sidebar = sidebar.clone();
                                    move |_: &MouseDownEvent, _window, cx| {
                                        if let Some(view) = sidebar.upgrade() {
                                            view.update(cx, |this, cx| {
                                                this.clan_menu_open = !this.clan_menu_open;
                                                cx.notify();
                                            });
                                        }
                                    }
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child(clan_name.clone()),
                                )
                                .child(
                                    Icon::new(IconName::ChevronDownIcon)
                                        .size(px(20.))
                                        .text_color(theme.text_secondary),
                                ),
                        )
                    })
                    .when(!has_clan, |el| {
                        el.child(
                            div()
                                .h_full()
                                .px_3()
                                .flex()
                                .items_center()
                                .text_base()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(clan_name.clone()),
                        )
                    })
                    .when_some(clan_menu_data, move |el, (clan_id, show_empty, locale)| {
                        let Some(clan_id) = clan_id else {
                            return el;
                        };
                        el.child(clan_menu_overlay(
                            build_clan_menu(
                                sidebar_for_menu.clone(),
                                channel_list_for_menu.clone(),
                                clan_id,
                                &locale,
                                show_empty,
                            ),
                            px(50.),
                            px(8.),
                        ))
                    })
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .on_mouse_move(cx.listener(|this, _event, _window, cx| {
                        if this.suppress_hover
                            && this.last_scroll_at.is_none_or(|t| {
                                t.elapsed() >= Duration::from_millis(SCROLL_HOVER_RELEASE_MS)
                            })
                        {
                            this.suppress_hover = false;
                            cx.notify();
                        }
                    }))
                    .child(list_element)
                    .children(skeleton_overlay),
            )
            .when_some(
                menu_overlay,
                move |el, (position, channel_type, is_thread, locale)| {
                    el.child(context_menu_at(
                        position,
                        build_channel_menu(
                            sidebar_for_channel_menu.clone(),
                            &locale,
                            channel_type,
                            is_thread,
                        ),
                    ))
                },
            )
    }
}

fn sk_bar(sk: gpui::Rgba, w: gpui::Pixels, h: gpui::Pixels) -> gpui::Div {
    div().w(w).h(h).rounded(px(4.)).bg(sk)
}

fn sk_channel_row(sk: gpui::Rgba, text_w: f32) -> gpui::Div {
    div()
        .h(px(34.))
        .w_full()
        .px_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(sk_bar(sk, px(18.), px(18.)))
        .child(sk_bar(sk, px(text_w), px(14.)))
}

fn sk_category(sk: gpui::Rgba, name_w: f32, widths: [f32; 4]) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .pt(px(10.))
                .pb(px(6.))
                .px_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(sk_bar(sk, px(18.), px(18.)))
                .child(sk_bar(sk, px(name_w), px(12.))),
        )
        .child(sk_channel_row(sk, widths[0]))
        .child(sk_channel_row(sk, widths[1]))
        .child(sk_channel_row(sk, widths[2]))
        .child(sk_channel_row(sk, widths[3]))
}

fn render_skeleton(cx: &App) -> AnyElement {
    let sk = cx.theme().bg_hover;
    div()
        .flex()
        .flex_col()
        .child(div().w_full().h(px(136.)).mb_2().rounded(px(4.)).bg(sk))
        .child(sk_category(sk, 70., [88., 64., 100., 76.]))
        .child(sk_category(sk, 92., [72., 96., 60., 84.]))
        .child(sk_category(sk, 60., [92., 68., 80., 64.]))
        .into_any_element()
}

fn sidebar_skeleton_layer(theme: &Theme, cx: &App) -> gpui::Div {
    div()
        .absolute()
        .inset_0()
        .bg(theme.bg_secondary)
        .child(render_skeleton(cx))
}

fn nav_row(icon: IconName, label: &'static str, theme: &crate::theme::Theme) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px_2()
        .h(px(34.))
        .gap_2()
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(theme.text_secondary)
        .child(
            Icon::new(icon)
                .size(px(20.))
                .text_color(theme.text_secondary),
        )
        .child(
            div()
                .text_base()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label),
        )
}

const NUMBER_APPS_SHOW_OFF: usize = 4;
const MAX_CHANNEL_LABEL_CHARS: usize = 20;

fn truncate_channel_label(name: &str) -> String {
    if name.chars().count() > MAX_CHANNEL_LABEL_CHARS {
        let head: String = name.chars().take(MAX_CHANNEL_LABEL_CHARS).collect();
        format!("{head}...")
    } else {
        name.to_string()
    }
}

fn render_banner_and_events(
    banner_url: Option<&str>,
    app_channels: &[AppChannelSlot],
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let divider_color = theme.border;
    let hover_bg = theme.bg_hover;

    let mut col = div().flex().flex_col().w_full();

    if let Some(url) = banner_url {
        col = col.child(
            div().w_full().h(px(136.)).mb_2().overflow_hidden().child(
                gpui::img(crate::util::imgproxy::proxied(cx, url, 480, 272, "fill"))
                    .w_full()
                    .h_full()
                    .object_fit(gpui::ObjectFit::Cover),
            ),
        );
    }

    let nav_col = div()
        .flex()
        .flex_col()
        .w_full()
        .p_2()
        .gap_1()
        .child(nav_row(IconName::IconEvents, "Events", theme))
        .child(nav_row(IconName::MemberList, "Members", theme));

    col = col.child(nav_col);

    let hr = || div().w_full().h(px(1.)).ml(px(3.)).bg(divider_color);

    if !app_channels.is_empty() {
        let show_list: Vec<&AppChannelSlot> =
            app_channels.iter().take(NUMBER_APPS_SHOW_OFF).collect();
        let has_more = app_channels.len() > NUMBER_APPS_SHOW_OFF + 1;

        let app_row = if show_list.len() < NUMBER_APPS_SHOW_OFF {
            div().flex().flex_row().items_center().gap_2().py_1().px_2()
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .px_2()
                .justify_center()
        };

        let mut app_row = app_row;
        for (ix, slot) in show_list.iter().enumerate() {
            let icon_el: AnyElement = if let Some(logo) = &slot.app_logo {
                gpui::img(logo.clone())
                    .w(px(24.))
                    .h(px(24.))
                    .into_any_element()
            } else {
                gpui::svg()
                    .path("icons/channel-app-fallback.svg")
                    .w(px(24.))
                    .h(px(24.))
                    .text_color(theme.text_primary)
                    .into_any_element()
            };
            app_row = app_row.child(
                div()
                    .id(SharedString::from(format!("app-channel-{ix}")))
                    .w(px(40.))
                    .h(px(40.))
                    .p_2()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .child(icon_el),
            );
        }
        if has_more {
            app_row = app_row.child(
                div()
                    .id("app-channels-more")
                    .w(px(40.))
                    .h(px(40.))
                    .p_2()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .child(
                        Icon::new(IconName::RightIcon)
                            .size(px(24.))
                            .text_color(theme.text_primary),
                    ),
            );
        }

        col = col.child(hr()).child(app_row).child(hr());
    } else {
        col = col.child(hr());
    }

    col.into_any_element()
}

fn render_sidebar_item(
    items: &[SidebarItem],
    ix: usize,
    cx: &App,
    channel_list_handle: Entity<ChannelList>,
    active_clan_id_for_nav: Option<ClanId>,
    sidebar: WeakEntity<ChannelSidebar>,
    suppress_hover: bool,
) -> AnyElement {
    let theme = cx.theme();
    let Some(item) = items.get(ix) else {
        return div().into_any_element();
    };

    match item {
        SidebarItem::BannerAndEvents {
            banner_url,
            app_channels,
        } => render_banner_and_events(banner_url.as_deref(), app_channels, cx),

        SidebarItem::Category {
            elem_id,
            id,
            name_upper,
            collapsed,
        } => {
            let category_id = id.clone();
            let category_name = name_upper.clone();
            let clan_id_for_toggle = active_clan_id_for_nav.unwrap_or_default();

            let mut header = div()
                .id(elem_id.clone())
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .px_2()
                .cursor_pointer()
                .text_color(theme.text_muted)
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM);

            let icon = if *collapsed {
                IconName::CaretRight
            } else {
                IconName::CaretDown
            };
            header = header
                .child(Icon::new(icon).size(px(18.0)).text_color(theme.text_muted))
                .child(div().ml_1().child(category_name));

            header.interactivity().on_click(on_category_click(
                channel_list_handle.clone(),
                clan_id_for_toggle,
                category_id,
            ));

            div()
                .pt(px(10.))
                .pb(px(6.))
                .flex()
                .flex_col()
                .child(header)
                .into_any_element()
        }

        SidebarItem::Channel {
            elem_id,
            id,
            name,
            channel_type,
            unread,
            private,
            selected,
            badge_count,
            badge_label: _badge_label,
            muted,
            is_thread,
            line_above,
            line_below,
            voice_members,
        } => {
            let ch_id = id.clone();
            let clan_id_inner = active_clan_id_for_nav;

            let make_channel_element = || {
                let icon = channel_type_icon(*channel_type, *private);
                let highlight_type = shows_left_unread_nub(*channel_type);
                let text_bright = *selected || ((*unread || *badge_count > 0) && highlight_type);
                let bold = (*selected || *unread) && highlight_type;
                let icon_active = *selected || *unread || *badge_count > 0;
                let name_base: gpui::Hsla = if text_bright {
                    theme.tokens.text_secondary.into()
                } else {
                    theme.tokens.text_theme_primary.into()
                };
                let name_hover: gpui::Hsla = theme.tokens.text_secondary.into();
                let icon_base: gpui::Hsla = if icon_active {
                    theme.tokens.bg_icon_theme_active.into()
                } else {
                    theme.tokens.bg_icon_theme.into()
                };
                let icon_hover: gpui::Hsla = theme.tokens.bg_icon_theme_active.into();
                let weight = if bold {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::MEDIUM
                };
                let hoverable = !*selected && !suppress_hover;
                let show_channel_nub = *unread && !*selected && highlight_type;
                let show_channel_badge = crate::SHOW_UNREAD_BADGE_COUNT && *badge_count > 0;
                ChannelRowElement::new(
                    SharedString::from(format!("channel-row-{ch_id}")),
                    icon,
                    name.clone().into(),
                )
                .icon_color(icon_base, icon_hover)
                .name_style(name_base, name_hover, weight)
                .selected_bg(if *selected {
                    Some(theme.tokens.bg_active_member_channel.into())
                } else {
                    None
                })
                .hover_bg(if hoverable {
                    Some(theme.bg_hover.into())
                } else {
                    None
                })
                .muted(*muted)
                .unread_nub(if show_channel_nub {
                    Some(theme.tokens.text_secondary.into())
                } else {
                    None
                })
                .badge(if show_channel_badge {
                    Some(ChannelRowBadge {
                        count: *badge_count,
                        bg: gpui::rgb(0xda_37_3c).into(),
                        text_color: gpui::white(),
                    })
                } else {
                    None
                })
            };

            let make_thread_element = || {
                let thread_highlighted = *selected || *unread;
                let name_base: gpui::Hsla = if thread_highlighted {
                    theme.tokens.text_secondary.into()
                } else {
                    theme.tokens.text_theme_primary.into()
                };
                let name_hover: gpui::Hsla = theme.tokens.text_secondary.into();
                let weight = if thread_highlighted {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::MEDIUM
                };
                let hoverable = !*selected && !suppress_hover;
                let show_thread_badge = crate::SHOW_UNREAD_BADGE_COUNT && *badge_count > 0;
                ChannelRowElement::new(
                    SharedString::from(format!("thread-row-{ch_id}")),
                    IconName::Hashtag,
                    name.clone().into(),
                )
                .name_style(name_base, name_hover, weight)
                .selected_bg(if *selected {
                    Some(theme.tokens.bg_active_member_channel.into())
                } else {
                    None
                })
                .hover_bg(if hoverable {
                    Some(theme.bg_hover.into())
                } else {
                    None
                })
                .unread_nub(if *unread {
                    Some(theme.tokens.text_secondary.into())
                } else {
                    None
                })
                .badge(if show_thread_badge {
                    Some(ChannelRowBadge {
                        count: *badge_count,
                        bg: gpui::rgb(0xda_37_3c).into(),
                        text_color: gpui::white(),
                    })
                } else {
                    None
                })
                .connector(Some(ThreadConnector {
                    line_above: *line_above,
                    line_below: *line_below,
                    color: theme.text_muted.into(),
                }))
            };

            if *is_thread || voice_members.is_empty() {
                let element = if *is_thread {
                    make_thread_element()
                } else {
                    make_channel_element()
                };
                let click_handle = row_handle.clone();
                let click_id = ch_id.clone();
                let click_clan = clan_id_inner;
                let menu_sidebar = sidebar.clone();
                let menu_channel_type = *channel_type;
                let menu_is_thread = *is_thread;
                return element
                    .on_click(move |_window, cx| {
                        click_handle.update(cx, |m, cx| {
                            m.select_channel(click_id.parse().unwrap_or_default(), cx);
                        });
                        if let Some(cid) = click_clan {
                            crate::router::navigate(
                                cx,
                                crate::router::Route::Channel {
                                    clan_id: cid,
                                    channel_id: click_id.parse().unwrap_or_default(),
                                },
                            );
                        }
                    })
                    .on_right_click(move |position, _window, cx| {
                        if let Some(view) = menu_sidebar.upgrade() {
                            view.update(cx, |this, cx| {
                                this.open_menu = Some(OpenMenu {
                                    channel_type: menu_channel_type,
                                    is_thread: menu_is_thread,
                                    position,
                                });
                                cx.notify();
                            });
                        }
                    })
                    .into_any_element();
            }

            let row_content = make_channel_element().into_any_element();

            let mut channel_col = div()
                .id(elem_id.clone())
                .w_full()
                .flex()
                .flex_col()
                .child(row_content);

            if !voice_members.is_empty() {
                let voice_pl = if *is_thread { px(40.) } else { px(32.) };
                let members_el =
                    div()
                        .flex()
                        .flex_col()
                        .pl(voice_pl)
                        .children(voice_members.iter().map(|m| {
                            let name_text = if m.display_name.is_empty() {
                                m.user_id.clone()
                            } else {
                                m.display_name.clone()
                            };
                            let avatar = if m.avatar_url.is_empty() {
                                Avatar::new().name(name_text.clone())
                            } else {
                                Avatar::new()
                                    .src(SharedString::from(m.avatar_url.clone()))
                                    .name(name_text.clone())
                            };
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .px_2()
                                .py(px(2.))
                                .gap_1()
                                .child(avatar.with_size(Size::XSmall))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.text_muted)
                                        .child(name_text),
                                )
                        }));
                channel_col = channel_col.child(members_el);
            }

            let mut channel_col = channel_col.on_mouse_down(MouseButton::Right, {
                let channel_type = *channel_type;
                let is_thread = *is_thread;
                move |event: &MouseDownEvent, _window, cx| {
                    let position = event.position;
                    if let Some(view) = sidebar.upgrade() {
                        view.update(cx, |this, cx| {
                            this.open_menu = Some(OpenMenu {
                                channel_type,
                                is_thread,
                                position,
                            });
                            cx.notify();
                        });
                    }
                }
            });

            channel_col
                .interactivity()
                .on_click(on_channel_click(ch_id, clan_id_inner));

            channel_col.into_any_element()
        }
    }
}
