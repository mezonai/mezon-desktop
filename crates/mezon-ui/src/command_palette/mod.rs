use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window,
};

use crate::app::shell::Shell;
use crate::components::primitives::{v_flex, Input, InputEvent, InputState};
use crate::theme::ActiveTheme;

pub struct CommandPaletteModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    search_input: Entity<InputState>,
    _search_sub: Subscription,
}

impl Focusable for CommandPaletteModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPaletteModal {
    pub fn toggle(locale: SharedString, window: &mut Window, cx: &mut App) {
        let shell = Shell::global(cx);
        if shell.read(cx).command_palette_open() {
            Self::close(cx);
        } else if !shell.read(cx).has_modal() {
            Self::open(locale, window, cx);
        }
    }

    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        let placeholder: SharedString = mezon_i18n::t(&locale, "common.searchModal.placeholder")
            .to_string()
            .into();

        let view = cx.new(|cx| {
            let search_input =
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));
            let search_sub = cx.subscribe(&search_input, |_this, _, _event: &InputEvent, cx| {
                cx.notify();
            });
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                search_input,
                _search_sub: search_sub,
            }
        });

        let focus_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_command_palette(view.into(), cx));
    }

    pub fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    pub fn try_toggle_authenticated(cx: &mut App) {
        use gpui::AppContext;
        use mezon_store::{AuthState, LoginStore, Settings};

        let Some(settings) = Settings::try_global(cx) else {
            return;
        };
        let Some(login) = LoginStore::try_global(cx) else {
            return;
        };
        if !matches!(
            login.read(cx).auth_state().read(cx),
            AuthState::Authenticated(_)
        ) {
            return;
        }
        let Some(window_handle) =
            crate::app::main_window::handle(cx).or_else(|| cx.active_window())
        else {
            return;
        };
        let locale = settings.read(cx).language.clone().into();
        cx.defer(move |cx| {
            let _ = cx.update_window(window_handle, |_, window, cx| {
                Self::toggle(locale, window, cx);
            });
        });
    }
}

impl Render for CommandPaletteModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let protip = mezon_i18n::t(&locale, "common.searchModal.protip");
        let protip_description = mezon_i18n::t(&locale, "common.searchModal.protipDescription");

        let search = div()
            .mt_2()
            .mb(px(15.))
            .rounded_lg()
            .bg(theme.tokens.bg_input_secondary)
            .border_1()
            .border_color(theme.tokens.border_theme_primary)
            .px_3()
            .py(px(18.))
            .child(
                Input::new(&self.search_input)
                    .w_full()
                    .text_size(px(16.))
                    .text_color(theme.tokens.text_theme_message),
            );

        let list = div()
            .id("command-palette-list")
            .max_h(px(250.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(3.))
            .pr(px(5.));

        let footer = div().pt_2().child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.tokens.text_theme_primary)
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.status_online)
                        .child(format!("{protip} ")),
                )
                .child(div().child(protip_description)),
        );

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .occlude()
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Self::close(cx);
            }))
            .mx_4()
            .w(px(640.))
            .px_6()
            .py_4()
            .rounded(px(6.))
            .bg(theme.tokens.bg_modal_theme_search)
            .shadow_lg()
            .child(search)
            .child(list)
            .child(footer)
    }
}
