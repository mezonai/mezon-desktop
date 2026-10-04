use gpui::{
    App, FontWeight, MouseDownEvent, SharedString, WeakEntity, Window, deferred, div, prelude::*,
    px,
};
use mezon_store::PlatformStore;
use ui::Tooltip;

use crate::components::primitives::{Icon, IconName};
use crate::sidebar::channel_sidebar::ChannelSidebar;
use crate::sidebar::channel_sidebar::items::AppChannelSlot;
use crate::theme::ActiveTheme;

const PANEL_WIDTH: f32 = 360.;
const PANEL_MAX_HEIGHT: f32 = 420.;
const GRID_ICON: f32 = 48.;
const FIND_APP_URL: &str = "https://top.mezon.ai/search?q=&tags=&type=app";

pub fn app_list_popover_overlay(
    apps: &[AppChannelSlot],
    locale: &str,
    sidebar: WeakEntity<ChannelSidebar>,
    suppress_hover: bool,
    icon_cache: gpui::Entity<crate::image_cache::LruImageCache>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let title = mezon_i18n::t(locale, "channelList.channelApps");
    let find_title = mezon_i18n::t(locale, "channelList.findChannelApp");
    let find_subtitle = mezon_i18n::t(locale, "channelList.findChannelAppHint");

    let mut grid = div()
        .id("app-list-grid")
        .grid()
        .grid_cols(4)
        .gap_2()
        .p_4()
        .overflow_y_scroll()
        .max_h(px(PANEL_MAX_HEIGHT - 120.));

    for (ix, app) in apps.iter().enumerate() {
        let slot = app.clone();
        let sidebar_for_click = sidebar.clone();
        let app_name = if app.app_name.is_empty() {
            SharedString::from("Channel App")
        } else {
            SharedString::from(app.app_name.clone())
        };
        let icon_el = if let Some(logo) = &app.app_logo {
            gpui::img(logo.clone())
                .w(px(28.))
                .h(px(28.))
                .aspect_square()
                .into_any_element()
        } else {
            gpui::svg()
                .path("icons/channel-app-fallback.svg")
                .w(px(28.))
                .h(px(28.))
                .text_color(theme.text_primary)
                .into_any_element()
        };
        grid = grid.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .child(
                    div()
                        .id(SharedString::from(format!("app-list-item-{ix}")))
                        .relative()
                        .w(px(GRID_ICON))
                        .h(px(GRID_ICON))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .when(!suppress_hover, |cell| {
                            cell.hover(|s| s.bg(theme.bg_hover))
                                .tooltip(Tooltip::text(app_name.clone()))
                        })
                        .on_click(move |_, _, cx| {
                            let _ = sidebar_for_click.update(cx, |sidebar, cx| {
                                sidebar.dismiss_app_list(cx);
                                sidebar.launch_channel_app(slot.clone(), cx);
                            });
                        })
                        .child(icon_el),
                )
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .truncate()
                        .max_w(px(GRID_ICON + 8.))
                        .text_color(theme.text_secondary)
                        .child(app_name),
                ),
        );
    }

    let sidebar_dismiss = sidebar.clone();
    let sidebar_find = sidebar.clone();
    deferred(
        div()
            .id("app-list-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .child(
                div()
                    .image_cache(icon_cache)
                    .id("app-list-popover")
                    .absolute()
                    .top(px(196.))
                    .left(px(8.))
                    .w(px(PANEL_WIDTH))
                    .max_h(px(PANEL_MAX_HEIGHT))
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_secondary)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(move |_: &MouseDownEvent, _: &mut Window, cx| {
                        let _ = sidebar_dismiss.update(cx, |sidebar, cx| {
                            sidebar.dismiss_app_list(cx);
                        });
                    })
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(theme.border)
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .child(grid)
                    .child(
                        div()
                            .id("app-list-find")
                            .group("app-list-find")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .cursor_pointer()
                            .rounded_b_lg()
                            .when(!suppress_hover, |row| row.hover(|s| s.bg(theme.bg_hover)))
                            .on_click(move |_, _, cx| {
                                let _ = sidebar_find.update(cx, |sidebar, cx| {
                                    sidebar.dismiss_app_list(cx);
                                });
                                if let Some(store) = PlatformStore::try_global(cx) {
                                    let _ = store.read(cx).open_url_external(FIND_APP_URL);
                                }
                            })
                            .child(
                                div()
                                    .w(px(32.))
                                    .h(px(32.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(theme.bg_primary)
                                    .group_hover("app-list-find", |s| s.bg(theme.bg_tertiary))
                                    .child(
                                        Icon::new(IconName::Search)
                                            .size(px(20.))
                                            .text_color(theme.text_primary),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(find_title),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_muted)
                                            .child(find_subtitle),
                                    ),
                            )
                            .child(
                                Icon::new(IconName::ArrowRight)
                                    .size(px(20.))
                                    .text_color(theme.text_secondary),
                            ),
                    ),
            ),
    )
}
