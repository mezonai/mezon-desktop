use crate::theme::Theme;
use gpui::{Context, Entity, FontWeight, Rgba, Window, div, prelude::*, px};
use gpui_component::{Icon, IconName, h_flex, label::Label, v_flex};
use mezon_store::Settings;

pub struct AppearancePage {
    settings: Entity<Settings>,
}

impl AppearancePage {
    pub fn new(settings: Entity<Settings>, _cx: &mut Context<Self>) -> Self {
        Self { settings }
    }
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

fn message_row(
    avatar_bg: Rgba,
    display_name: String,
    timestamp: String,
    message: String,
    theme: Theme,
) -> impl IntoElement {
    h_flex()
        .gap_3()
        .child(
            div()
                .size(px(45.0))
                .rounded_full()
                .bg(avatar_bg)
                .flex_none(),
        )
        .child(
            v_flex()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Label::new(display_name)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary),
                        )
                        .child(Label::new(timestamp).text_xs().text_color(theme.text_muted)),
                )
                .child(Label::new(message).text_color(theme.text_secondary)),
        )
}

fn theme_swatch(
    key: String,
    label: String,
    swatch_color: Rgba,
    is_selected: bool,
    theme: Theme,
    settings: Entity<Settings>,
) -> impl IntoElement {
    div()
        .id(key.clone())
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            settings.update(cx, |s, _| {
                s.theme = key.clone();
                s.save_sync();
            });
        })
        .child(
            div()
                .relative()
                .child(
                    div()
                        .size(px(60.0))
                        .rounded_full()
                        .bg(swatch_color)
                        .border_2()
                        .border_color(if is_selected {
                            theme.brand
                        } else {
                            theme.border
                        })
                        .when(is_selected, |el| el.shadow_lg()),
                )
                .when(is_selected, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .rounded_full()
                            .bg(theme.brand)
                            .p(px(2.0))
                            .child(
                                Icon::new(IconName::Check)
                                    .size_3()
                                    .text_color(rgba(255, 255, 255, 1.0)),
                            ),
                    )
                }),
        )
        .child(Label::new(label).text_sm().text_color(theme.text_primary))
}

impl Render for AppearancePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_theme = self.settings.read(cx).theme.clone();
        let locale = self.settings.read(cx).language.clone();
        let theme = match current_theme.as_str() {
            "light" => Theme::light(),
            "purple" => Theme::purple(),
            "abyss" => Theme::abyss(),
            "red_dark" => Theme::red_dark(),
            _ => Theme::dark(),
        };
        let settings = self.settings.clone();

        let themes = [
            ("dark", mezon_i18n::t(&locale, "appThemeSetting.fields.dark"), rgba(49, 51, 56, 1.0)),
            ("light", mezon_i18n::t(&locale, "appThemeSetting.fields.light"), rgba(255, 255, 255, 1.0)),
            ("purple", mezon_i18n::t(&locale, "appThemeSetting.fields.purpleHaze"), rgba(120, 90, 200, 1.0)),
            ("abyss", mezon_i18n::t(&locale, "appThemeSetting.fields.abyssDark"), rgba(13, 15, 22, 1.0)),
            ("red_dark", mezon_i18n::t(&locale, "appThemeSetting.fields.redDark"), rgba(210, 80, 80, 1.0)),
        ];

        let sample_msgs = [
            (
                rgba(88, 101, 242, 1.0),
                "Alice",
                "Today at 2:30 PM",
                "Hey, have you seen the new theme?",
            ),
            (
                rgba(67, 181, 129, 1.0),
                "Bob",
                "Today at 2:31 PM",
                "Yeah! The dark mode looks great!",
            ),
            (
                rgba(240, 178, 50, 1.0),
                "Carol",
                "Today at 2:32 PM",
                "Look at me I'm a beautiful butterfly",
            ),
        ];

        v_flex()
            .gap_6()
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.appSettings.appearance"))
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .bg(theme.bg_primary)
                    .p_5()
                    .gap_5()
                    .overflow_hidden()
                    .children(sample_msgs.map(|(bg, name, ts, msg)| {
                        message_row(
                            bg,
                            name.to_string(),
                            ts.to_string(),
                            msg.to_string(),
                            theme.clone(),
                        )
                    })),
            )
            .child(
                Label::new("Theme")
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary),
            )
            .child(h_flex().flex_wrap().gap(px(30.0)).children(themes.map(
                |(key, label, swatch_color)| {
                    let is_selected = current_theme == key;
                    theme_swatch(
                        key.to_string(),
                        label.to_string(),
                        swatch_color,
                        is_selected,
                        theme.clone(),
                        settings.clone(),
                    )
                },
            )))
    }
}
