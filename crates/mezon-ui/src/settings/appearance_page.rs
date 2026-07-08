use crate::components::primitives::{Icon, IconName, Label, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme, resolve_theme, set_theme};
use gpui::{Context, Entity, FontWeight, Rgba, Window, div, prelude::*, px};
use mezon_store::Settings;

const THEME_KEYS: &[&str] = &[
    "dark",
    "light",
    "purple",
    "abyss",
    "red_dark",
    "sunrise",
    "sunset",
    "cisher",
    "berrynade",
];

pub struct AppearancePage {
    settings: Entity<Settings>,
    swatch_colors: Vec<Rgba>,
}

impl AppearancePage {
    pub fn new(settings: Entity<Settings>, _cx: &mut Context<Self>) -> Self {
        let swatch_colors = THEME_KEYS
            .iter()
            .map(|k| resolve_theme(k).bg_primary)
            .collect();
        Self {
            settings,
            swatch_colors,
        }
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
    theme: &Theme,
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
    theme: &Theme,
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
            set_theme(resolve_theme(&key), cx);
            settings.update(cx, |s, cx| {
                s.theme = key.clone();
                cx.notify();
            });
            let snapshot = settings.read(cx).clone();
            cx.background_executor()
                .spawn(async move {
                    snapshot.save_sync();
                })
                .detach();
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
        let theme = cx.theme();
        let settings = self.settings.clone();

        let themes = [
            (
                "dark",
                mezon_i18n::t(&locale, "appThemeSetting.fields.dark"),
            ),
            (
                "light",
                mezon_i18n::t(&locale, "appThemeSetting.fields.light"),
            ),
            (
                "purple",
                mezon_i18n::t(&locale, "appThemeSetting.fields.purpleHaze"),
            ),
            (
                "abyss",
                mezon_i18n::t(&locale, "appThemeSetting.fields.abyssDark"),
            ),
            (
                "red_dark",
                mezon_i18n::t(&locale, "appThemeSetting.fields.redDark"),
            ),
            (
                "sunrise",
                mezon_i18n::t(&locale, "appThemeSetting.fields.sunrise"),
            ),
            (
                "sunset",
                mezon_i18n::t(&locale, "appThemeSetting.fields.sunset"),
            ),
            (
                "cisher",
                mezon_i18n::t(&locale, "appThemeSetting.fields.cisher"),
            ),
            (
                "berrynade",
                mezon_i18n::t(&locale, "appThemeSetting.fields.berrynade"),
            ),
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
                v_flex()
                    .rounded_lg()
                    .bg(theme.bg_primary)
                    .p_5()
                    .gap_5()
                    .overflow_hidden()
                    .children(sample_msgs.map(|(bg, name, ts, msg)| {
                        message_row(bg, name.to_string(), ts.to_string(), msg.to_string(), theme)
                    })),
            )
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.appearance.theme"))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary),
            )
            .child(h_flex().flex_wrap().gap(px(30.0)).children(
                themes.iter().zip(self.swatch_colors.iter().copied()).map(
                    |((key, label), swatch_color)| {
                        let is_selected = current_theme == *key;
                        theme_swatch(
                            key.to_string(),
                            label.to_string(),
                            swatch_color,
                            is_selected,
                            theme,
                            settings.clone(),
                        )
                    },
                ),
            ))
    }
}
