use crate::theme::{Theme, resolve_theme};
use gpui::{Context, Entity, FontWeight, Window, div, prelude::*, px};
use gpui_component::{Icon, IconName, label::Label, v_flex};
use mezon_store::Settings;

pub struct LanguagePage {
    settings: Entity<Settings>,
}

impl LanguagePage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        Self { settings }
    }
}

fn language_row(
    lang_key: &str,
    flag: &str,
    display_name: &str,
    is_selected: bool,
    theme: &Theme,
    settings: Entity<Settings>,
) -> impl IntoElement {
    let lang_key = lang_key.to_string();
    let flag = flag.to_string();
    let display_name = display_name.to_string();

    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_3()
        .cursor_pointer()
        .child(div().rounded_full().size_2().bg(if is_selected {
            theme.status_online
        } else {
            gpui::Rgba {
                r: 128.0 / 255.0,
                g: 132.0 / 255.0,
                b: 142.0 / 255.0,
                a: 1.0,
            }
        }))
        .child(format!("{} {}", flag, display_name))
        .child(div().flex_1());

    let row = if is_selected {
        row.child(
            Icon::new(IconName::Check)
                .size_4()
                .text_color(theme.text_primary),
        )
    } else {
        row
    };

    row.id(lang_key.clone()).on_click(move |_, _, cx| {
        settings.update(cx, |s, _| {
            s.language = lang_key.clone();
            s.save_sync();
        });
    })
}

impl Render for LanguagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let theme = resolve_theme(&self.settings.read(cx).theme);
        let settings_entity = self.settings.clone();

        v_flex()
            .gap_6()
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.language.title"))
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.language.description"))
                    .text_sm()
                    .text_color(theme.text_muted),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .bg(theme.bg_primary)
                    .child(language_row(
                        "en",
                        "\u{1F1FA}\u{1F1F8}",
                        &mezon_i18n::t(&locale, "setting.language.english"),
                        locale == "en",
                        &theme,
                        settings_entity.clone(),
                    ))
                    .child(div().h(px(1.0)).w_full().bg(theme.border))
                    .child(language_row(
                        "vi",
                        "\u{1F1FB}\u{1F1F3}",
                        &mezon_i18n::t(&locale, "setting.language.vietnamese"),
                        locale == "vi",
                        &theme,
                        settings_entity,
                    )),
            )
    }
}
