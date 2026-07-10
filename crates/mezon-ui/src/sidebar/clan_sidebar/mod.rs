use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, ListState, SharedString, Subscription, Window, div, img,
    list, prelude::*, px,
};
use mezon_store::{AccountStore, ClanList, DirectMessageStore, Settings};
use ui::Tooltip;

use crate::app::shell::Shell;
use crate::app::window_controls;
use crate::components::primitives::{Icon, IconName};
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};

mod clan_row;
mod direct_unread_list;
use clan_row::{ClanRow, render_clan_row, render_pill};
use direct_unread_list::{DirectUnreadListState, build_direct_unread_items};

pub struct ClanSidebar {
    clan_list: Entity<ClanList>,
    settings: Entity<Settings>,
    rows: Rc<Vec<ClanRow>>,
    direct_unread: DirectUnreadListState,
    list_state: ListState,
    dm_active: bool,
    can_go_back: bool,
    can_go_forward: bool,
    home_logo: SharedString,
    discover_title: SharedString,
    create_clan_title: SharedString,
    image_cache: Entity<crate::image_cache::LruImageCache>,
    _clan_sub: Subscription,
    _direct_sub: Subscription,
    _settings_sub: Subscription,
    _router_sub: Subscription,
    _account_sub: Subscription,
}

impl ClanSidebar {
    pub fn new(
        clan_list: Entity<ClanList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let direct_store = DirectMessageStore::global(cx);
        let direct_sub = cx.observe(&direct_store, |this, store, cx| {
            let live = build_direct_unread_items(store.read(cx), cx);
            this.direct_unread.sync(live, cx);
        });

        let clan_sub = cx.observe(&clan_list, |this, clan_list, cx| {
            this.sync_rows(clan_list.read(cx), cx);
            cx.notify();
        });
        let settings_sub = cx.observe(&settings, |this, _, cx| {
            this.sync_chrome(cx);
            cx.notify();
        });
        let account_sub = cx.observe(&AccountStore::global(cx), |this, _, cx| {
            this.sync_chrome(cx);
            cx.notify();
        });
        let router_sub = cx.observe(&Router::global(cx), |this, router, cx| {
            let router = router.read(cx);
            let new_dm_active = matches!(
                router.route(),
                Route::Direct | Route::DirectMessage { .. } | Route::Friends
            );
            let new_can_go_back = router.can_go_back();
            let new_can_go_forward = router.can_go_forward();
            if new_dm_active != this.dm_active
                || new_can_go_back != this.can_go_back
                || new_can_go_forward != this.can_go_forward
            {
                this.dm_active = new_dm_active;
                this.can_go_back = new_can_go_back;
                this.can_go_forward = new_can_go_forward;
                cx.notify();
            }
        });

        let router = Router::global(cx);
        let router_view = router.read(cx);
        let initial_dm_active = matches!(
            router_view.route(),
            Route::Direct | Route::DirectMessage { .. } | Route::Friends
        );
        let initial_can_go_back = router_view.can_go_back();
        let initial_can_go_forward = router_view.can_go_forward();
        let mut this = Self {
            clan_list,
            settings,
            rows: Rc::new(Vec::new()),
            direct_unread: DirectUnreadListState::new(build_direct_unread_items(
                direct_store.read(cx),
                cx,
            )),
            list_state: ListState::new(0, gpui::ListAlignment::Top, px(48.)),
            dm_active: initial_dm_active,
            can_go_back: initial_can_go_back,
            can_go_forward: initial_can_go_forward,
            home_logo: SharedString::default(),
            discover_title: SharedString::default(),
            create_clan_title: SharedString::default(),
            image_cache: cx.new(|cx| {
                crate::image_cache::LruImageCache::avatar_thumbnail_small(
                    "clan-rail",
                    256,
                    6 * 1024 * 1024,
                    4 * 1024 * 1024,
                    cx,
                )
            }),
            _clan_sub: clan_sub,
            _direct_sub: direct_sub,
            _settings_sub: settings_sub,
            _router_sub: router_sub,
            _account_sub: account_sub,
        };
        let clan_list_handle = this.clan_list.clone();
        this.sync_rows(clan_list_handle.read(cx), cx);
        this.sync_chrome(cx);
        this
    }

    fn sync_chrome(&mut self, cx: &App) {
        let locale = self.settings.read(cx).language.clone();
        self.discover_title = mezon_i18n::t(&locale, "common.discover").to_string().into();
        self.create_clan_title = mezon_i18n::t(&locale, "common.createClan")
            .to_string()
            .into();
        let custom_logo = AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .and_then(|account| account.logo.clone())
            .filter(|logo| !logo.is_empty());
        self.home_logo = SharedString::from(crate::util::imgproxy::proxied(
            cx,
            custom_logo
                .as_deref()
                .unwrap_or("https://cdn.mezon.ai/landing-page-mezon/logodefault.webp"),
            100,
            100,
            "fill-down",
        ));
    }

    fn sync_rows(&mut self, clan_list_view: &ClanList, cx: &App) {
        let rows: Vec<ClanRow> = clan_list_view
            .clans
            .iter()
            .map(|clan| {
                let id = SharedString::from(clan.id.to_string());
                let row_id = SharedString::from(format!("clan-{}", clan.id));
                let group_name = SharedString::from(format!("clan-group-{}", clan.id));
                let avatar_id = SharedString::from(format!("clan-avatar-{}", clan.id));
                let proxied_avatar_url: Option<SharedString> =
                    clan.avatar_url.as_deref().map(|url| {
                        SharedString::from(crate::util::imgproxy::proxied(
                            cx,
                            url,
                            100,
                            100,
                            "fill-down",
                        ))
                    });
                ClanRow {
                    id,
                    id_num: clan.id,
                    row_id,
                    group_name,
                    name: SharedString::from(clan.name.clone()),
                    proxied_avatar_url,
                    avatar_id,
                    badge_count: clan.badge_count,
                    has_unread: clan.has_unread,
                    muted: clan.muted,
                }
            })
            .collect();
        let count = rows.len();
        let item_count = count + 1;
        let needs_reset = self.list_state.item_count() != item_count;
        self.rows = Rc::new(rows);
        if needs_reset {
            self.list_state.reset(item_count);
        }
    }
}

impl Render for ClanSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let avatar_cache = self.image_cache.clone();
        let theme = cx.theme();
        let dm_active = self.dm_active;
        let pill_color = theme.tokens.text_theme_primary;
        let rows = self.rows.clone();
        let clan_list_handle = self.clan_list.clone();
        let list_state = self.list_state.clone();
        let locale = self.settings.read(cx).language.clone();
        let discover_title = self.discover_title.clone();
        let create_clan_title = self.create_clan_title.clone();
        let clan_list_for_modal = self.clan_list.clone();
        let settings_for_modal = self.settings.clone();
        let home_logo = self.home_logo.clone();

        let clan_count = rows.len();
        let list_element = list(list_state, move |ix, _window, cx| {
            if ix < clan_count {
                render_clan_row(&rows, ix, cx, clan_list_handle.clone())
            } else {
                render_clan_footer(
                    cx,
                    &locale,
                    discover_title.clone(),
                    create_clan_title.clone(),
                    clan_list_for_modal.clone(),
                    settings_for_modal.clone(),
                )
            }
        })
        .size_full();

        let unread_list = self.direct_unread.render();

        div()
            .image_cache(avatar_cache)
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .pb(px(68.))
            .items_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .w_full()
                    .bg(theme.bg_tertiary)
                    .pt(px(window_controls::CLAN_SIDEBAR_HEADER_TOP))
                    .child(render_window_nav(
                        theme,
                        self.can_go_back,
                        self.can_go_forward,
                    ))
                    .child(
                        div()
                            .id("dm-logo")
                            .group("dm-group")
                            .relative()
                            .w_full()
                            .h(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(|_, _, cx| {
                                let target = DirectMessageStore::global(cx)
                                    .read(cx)
                                    .current()
                                    .map(|(id, ty)| Route::DirectMessage {
                                        direct_id: id,
                                        message_type: ty.to_string(),
                                    })
                                    .unwrap_or(Route::Direct);
                                crate::router::navigate(cx, target);
                            })
                            .child(render_pill(dm_active, "dm-group".into(), pill_color))
                            .child(
                                img(home_logo)
                                    .size(px(40.))
                                    .rounded(px(8.))
                                    .object_fit(gpui::ObjectFit::Cover),
                            ),
                    )
                    .child(unread_list)
                    .child(div().w(px(40.)).h(px(1.)).bg(theme.border).mt_3().mb_3()),
            )
            .child(div().flex_1().min_h_0().w_full().child(list_element))
    }
}

fn render_window_nav(theme: &Theme, can_go_back: bool, can_go_forward: bool) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .w_full()
        .pb(px(4.))
        .child(nav_arrow("clan-nav-back", can_go_back, true, theme))
        .child(nav_arrow("clan-nav-forward", can_go_forward, false, theme))
        .into_any_element()
}

fn nav_arrow(id: &'static str, enabled: bool, is_back: bool, theme: &Theme) -> AnyElement {
    let icon_color = if enabled {
        theme.text_secondary
    } else {
        theme.text_muted
    };
    let bg_hover = theme.bg_hover;

    let mut icon = Icon::new(IconName::LongArrowRight)
        .size(px(window_controls::NAV_ARROW_ICON_SIZE))
        .text_color(icon_color);
    if is_back {
        icon = icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
            std::f32::consts::PI,
        )));
    }

    let mut button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .p(px(window_controls::NAV_ARROW_BUTTON_PADDING))
        .child(icon);

    if enabled {
        button = button
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .on_click(move |_, _, cx| {
                if is_back {
                    crate::router::go_back(cx);
                } else {
                    crate::router::go_forward(cx);
                }
            });
    }

    button.into_any_element()
}

fn render_clan_footer(
    cx: &App,
    locale: &str,
    discover_title: SharedString,
    create_clan_title: SharedString,
    clan_list_for_modal: Entity<ClanList>,
    settings_for_modal: Entity<Settings>,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .items_center()
        .w_full()
        .child(
            div()
                .id("discover-btn")
                .w(px(40.))
                .h(px(40.))
                .rounded(px(12.))
                .bg(theme.tokens.theme_base_color)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| {
                    s.bg(theme.tokens.bg_button_add_friend)
                        .text_color(gpui::white())
                })
                .tooltip(Tooltip::text(discover_title))
                .on_click({
                    let locale = locale.to_string();
                    move |_, _, cx| {
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.info(mezon_i18n::t(&locale, "common.comingSoon").to_string(), cx);
                        });
                    }
                })
                .child(
                    Icon::new(IconName::CompassIcon)
                        .size(px(20.))
                        .text_color(theme.tokens.text_theme_primary),
                ),
        )
        .child(
            div()
                .id("create-clan-btn")
                .mt_3()
                .w(px(40.))
                .h(px(40.))
                .rounded(px(12.))
                .bg(theme.tokens.theme_base_color)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| {
                    s.bg(theme.tokens.bg_button_add_friend)
                        .text_color(gpui::white())
                })
                .tooltip(Tooltip::text(create_clan_title))
                .on_click(move |_, window, cx| {
                    use crate::clan::create_clan_modal::CreateClanModal;
                    let modal = cx.new(|cx| {
                        CreateClanModal::new(
                            clan_list_for_modal.clone(),
                            settings_for_modal.clone(),
                            window,
                            cx,
                        )
                    });
                    Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
                })
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child("+"),
                ),
        )
        .into_any_element()
}
