use crate::components::primitives::{Label, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};
use gpui::{Context, Entity, FontWeight, Rgba, Window, div, img, prelude::*};
use mezon_store::Settings;

pub struct LanguagePage {
    settings: Entity<Settings>,
}

impl LanguagePage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self { settings }
    }
}

const LANGUAGES: &[(&str, &str, &str, &str)] = &[
    (
        "en",
        "icons/flags/en.svg",
        "setting.language.english",
        "Mezon Team",
    ),
    (
        "vi",
        "icons/flags/vi.svg",
        "setting.language.vietnamese",
        "Mezon Team",
    ),
    (
        "ru",
        "icons/flags/ru.svg",
        "setting.language.russian",
        "reburst",
    ),
    (
        "es",
        "icons/flags/es.svg",
        "setting.language.spanish",
        "robits",
    ),
    (
        "tt",
        "icons/flags/tt.svg",
        "setting.language.tatar",
        "reburst",
    ),
    (
        "de",
        "icons/flags/de.svg",
        "setting.language.german",
        "robits",
    ),
    (
        "it",
        "icons/flags/it.svg",
        "setting.language.italian",
        "robits",
    ),
    (
        "pt",
        "icons/flags/pt.svg",
        "setting.language.portuguese",
        "robits",
    ),
    (
        "jpn",
        "icons/flags/jpn.svg",
        "setting.language.japanese",
        "robits",
    ),
    (
        "kr",
        "icons/flags/kr.svg",
        "setting.language.korean",
        "robits",
    ),
    (
        "swe",
        "icons/flags/swe.svg",
        "setting.language.swedish",
        "robits",
    ),
];

fn language_row(
    value: &'static str,
    flag_path: &'static str,
    name: &'static str,
    contributor: &'static str,
    is_selected: bool,
    theme: &Theme,
    settings: Entity<Settings>,
) -> impl IntoElement {
    let selected_bg = Rgba {
        a: 0.12,
        ..theme.brand
    };
    let hover_border = theme.text_muted;

    div()
        .id(value)
        .flex()
        .items_center()
        .justify_between()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(if is_selected {
            theme.brand
        } else {
            theme.border
        })
        .when(is_selected, |el| el.bg(selected_bg))
        .when(!is_selected, |el| {
            el.hover(move |s| s.border_color(hover_border))
        })
        .cursor_pointer()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(img(flag_path).size_6().flex_none())
                .child(
                    Label::new(name)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary),
                ),
        )
        .child(
            div()
                .text_xs()
                .italic()
                .pr_2()
                .text_color(theme.text_muted)
                .child(format!("by {contributor}")),
        )
        .on_click(move |_, _, cx| {
            settings.update(cx, |s, cx| {
                s.language = value.to_string();
                cx.notify();
            });
            let snapshot = settings.read(cx).clone();
            cx.background_executor()
                .spawn(async move {
                    snapshot.save_sync();
                })
                .detach();
        })
}

impl Render for LanguagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let theme = cx.theme();
        let settings = self.settings.clone();

        v_flex().gap_8().child(
            v_flex()
                .rounded_lg()
                .bg(theme.bg_primary)
                .p_4()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.language.description"))
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary),
                )
                .child(
                    v_flex().gap_3().children(LANGUAGES.iter().map(
                        |&(value, flag, name_key, contributor)| {
                            language_row(
                                value,
                                flag,
                                mezon_i18n::t(&locale, name_key),
                                contributor,
                                locale == value,
                                theme,
                                settings.clone(),
                            )
                        },
                    )),
                ),
        )
    }
}
