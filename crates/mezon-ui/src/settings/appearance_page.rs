use crate::theme::Theme;
use gpui::{Context, Entity, FontWeight, Rgba, Window, div, prelude::*};
use gpui_component::{h_flex, label::Label, v_flex};
use mezon_store::Settings;

pub struct AppearancePage {
    settings: Entity<Settings>,
}

impl AppearancePage {
    pub fn new(settings: Entity<Settings>, _cx: &mut Context<Self>) -> Self {
        Self { settings }
    }
}

impl Render for AppearancePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let current_theme = self.settings.read(cx).theme.clone();
        let settings = self.settings.clone();

        let themes: [(&str, &str, Rgba); 5] = [
            ("dark", "Dark", rgba(49, 51, 56, 1.0)),
            ("light", "Light", rgba(255, 255, 255, 1.0)),
            ("purple", "Purple", rgba(120, 90, 200, 1.0)),
            ("abyss", "Abyss", rgba(13, 15, 22, 1.0)),
            ("red_dark", "Red Dark", rgba(210, 80, 80, 1.0)),
        ];

        v_flex()
            .gap_6()
            .child(
                Label::new("Appearance")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
            )
            .child(h_flex().flex_wrap().gap_3().children(themes.map(
                |(key, label, swatch_color)| {
                    let is_selected = current_theme == key;
                    let settings = settings.clone();
                    let key = key.to_string();
                    let label = label.to_string();
                    div()
                        .id(key.clone())
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .p_3()
                        .rounded_lg()
                        .bg(if is_selected {
                            theme.bg_primary
                        } else {
                            theme.bg_tertiary
                        })
                        .border_1()
                        .border_color(if is_selected {
                            theme.brand
                        } else {
                            theme.border
                        })
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            settings.update(cx, |s, _| {
                                s.theme = key.clone();
                                s.save_sync();
                            });
                        })
                        .child(
                            div()
                                .size_12()
                                .rounded_md()
                                .bg(swatch_color)
                                .border_1()
                                .border_color(theme.border),
                        )
                        .child(Label::new(label).text_sm().text_color(theme.text_primary))
                },
            )))
            .child(
                Label::new("Restart required for theme to apply.")
                    .text_sm()
                    .text_color(theme.text_muted),
            )
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
