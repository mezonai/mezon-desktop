use gpui::{
    App, Context, FocusHandle, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};
use mezon_store::Settings;

use super::state::{TourState, TourTrigger, available_tracks_for, host_route, track_done};
use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants, Icon, IconName, h_flex, v_flex};
use crate::router::{self, Route, Router};
use crate::theme::{ActiveTheme, Theme};

pub fn settings_entry_row(
    id: &'static str,
    locale: &str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .w_full()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(4.0))
        .text_base()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.tokens.text_theme_primary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .child(SharedString::from(mezon_i18n::t(
            locale,
            "tour.settingsEntry",
        )))
        .on_click(|_, window, cx| TourLauncher::open(window, cx))
}

pub struct TourLauncher {
    focus_handle: FocusHandle,
    host_route: Route,
    restore_focus: Option<FocusHandle>,
}

impl TourLauncher {
    pub fn open(window: &mut Window, cx: &mut App) {
        let host_route = host_route(cx);
        let restore_focus = window.focused(cx);
        let view = cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
            host_route,
            restore_focus,
        });
        window.focus(&view.read(cx).focus_handle.clone(), cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }
}

impl Render for TourLauncher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = Settings::try_global(cx)
            .map(|settings| settings.read(cx).language.clone())
            .unwrap_or_else(|| "en".to_string());
        let tracks = available_tracks_for(&self.host_route);
        let entries: Vec<(&'static str, SharedString, SharedString, bool)> = tracks
            .iter()
            .map(|track| {
                (
                    track.id,
                    SharedString::from(mezon_i18n::t(&locale, track.name_key)),
                    SharedString::from(mezon_i18n::t(&locale, track.summary_key)),
                    track_done(track.id, cx),
                )
            })
            .collect();
        let host_route = self.host_route.clone();
        let restore_focus = self.restore_focus.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(460.))
            .gap_3()
            .p(px(20.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(SharedString::from(mezon_i18n::t(&locale, "tour.title"))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(SharedString::from(mezon_i18n::t(&locale, "tour.subtitle"))),
            )
            .when(entries.is_empty(), |el| {
                el.child(
                    div()
                        .p(px(12.))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .text_sm()
                        .text_color(theme.tokens.text_secondary)
                        .child(SharedString::from(mezon_i18n::t(&locale, "tour.empty"))),
                )
            })
            .children(entries.into_iter().map(|(id, name, summary, seen)| {
                let host_route = host_route.clone();
                let restore_focus = restore_focus.clone();
                h_flex()
                    .id(id)
                    .items_center()
                    .gap_3()
                    .p(px(12.))
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg_hover))
                    .on_click(move |_, window, cx| {
                        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                        if *Router::global(cx).read(cx).route_ref() != host_route {
                            router::navigate(cx, host_route.clone());
                        }
                        TourState::start_track_restoring(
                            id,
                            TourTrigger::Manual,
                            restore_focus.clone(),
                            window,
                            cx,
                        );
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.tokens.text_secondary)
                                    .child(summary),
                            ),
                    )
                    .when(seen, |el| {
                        el.child(
                            Icon::new(IconName::Check)
                                .size_4()
                                .text_color(theme.status_online),
                        )
                    })
            }))
            .child(
                h_flex().justify_end().child(
                    Button::new("tour-launcher-close")
                        .label(mezon_i18n::t(&locale, "tour.close"))
                        .ghost()
                        .on_click(|_, _window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                        }),
                ),
            )
    }
}
