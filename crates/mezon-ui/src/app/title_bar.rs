use crate::app::window_controls;
use crate::theme::{ActiveTheme, Theme};
use gpui::{
    Context, Entity, FontWeight, MouseButton, Subscription, Window, WindowControlArea, div,
    prelude::*,
};
use mezon_store::{AutoUpdateStatus, AutoUpdateStore, Settings};
use ui::px;

pub struct TitleBar {
    settings: Entity<Settings>,
    _bounds_observer: Option<Subscription>,
}

impl TitleBar {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        if let Some(store) = AutoUpdateStore::try_global(cx) {
            cx.observe(&store, |_, _, cx| cx.notify()).detach();
        }
        Self {
            settings,
            _bounds_observer: None,
        }
    }

    fn ensure_bounds_observer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._bounds_observer.is_some() {
            return;
        }

        self._bounds_observer = Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
    }
}

fn update_indicator(
    status: Option<AutoUpdateStatus>,
    locale: &str,
    theme: &Theme,
) -> Option<gpui::Stateful<gpui::Div>> {
    match status? {
        AutoUpdateStatus::Downloading { progress, .. } => Some(
            div()
                .id("titlebar-update-progress")
                .flex()
                .items_center()
                .h_full()
                .px_2()
                .mr_2()
                .text_xs()
                .text_color(theme.text_muted)
                .child(match progress {
                    Some(fraction) => format!(
                        "{} {:.0}%",
                        mezon_i18n::t(locale, "setting.update.downloading"),
                        fraction * 100.0
                    ),
                    None => format!("{}…", mezon_i18n::t(locale, "setting.update.downloading")),
                }),
        ),
        AutoUpdateStatus::Installing { .. } => Some(
            div()
                .id("titlebar-update-progress")
                .flex()
                .items_center()
                .h_full()
                .px_2()
                .mr_2()
                .text_xs()
                .text_color(theme.text_muted)
                .child(mezon_i18n::t(locale, "setting.update.installing").to_string()),
        ),
        AutoUpdateStatus::UpdateAvailable { version } => {
            let bg_hover = theme.bg_hover;
            Some(
                div()
                    .id("titlebar-update-install")
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .px_2()
                    .mr_2()
                    .rounded(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(gpui::rgb(0x22c55e))
                    .cursor_pointer()
                    .hover(move |s| s.bg(bg_hover))
                    .on_click(|_, _, cx| {
                        if let Some(store) = AutoUpdateStore::try_global(cx) {
                            store.update(cx, |store, cx| store.check(true, cx));
                        }
                    })
                    .child(format!(
                        "{} (v{version})",
                        mezon_i18n::t(locale, "setting.update.available")
                    )),
            )
        }
        AutoUpdateStatus::Updated { version } => {
            let bg_hover = theme.bg_hover;
            Some(
                div()
                    .id("titlebar-update-restart")
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .px_2()
                    .mr_2()
                    .rounded(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(gpui::rgb(0x22c55e))
                    .cursor_pointer()
                    .hover(move |s| s.bg(bg_hover))
                    .on_click(|_, _, cx| cx.restart())
                    .child(format!(
                        "{} (v{version})",
                        mezon_i18n::t(locale, "setting.update.restart")
                    )),
            )
        }
        _ => None,
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_bounds_observer(window, cx);
        let locale = self.settings.read(cx).language.clone();
        let update_status =
            AutoUpdateStore::try_global(cx).map(|store| store.read(cx).status().clone());
        let theme = cx.theme();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h_8()
            .bg(theme.title_bar_bg)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .h_full()
                    .when(cfg!(target_os = "windows"), |bar| {
                        bar.window_control_area(WindowControlArea::Drag)
                    })
                    .when(cfg!(target_os = "linux"), |bar| {
                        bar.on_mouse_down(MouseButton::Left, |event, window, _| {
                            if event.click_count >= 2 {
                                window.zoom_window();
                            } else {
                                window.start_window_move();
                            }
                        })
                    })
                    .child(
                        div().flex().items_center().px_3().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(window_controls::APP_NAME),
                        ),
                    ),
            )
            .children(update_indicator(update_status, &locale, theme))
            .child(window_controls::render_controls(theme, window))
    }
}
