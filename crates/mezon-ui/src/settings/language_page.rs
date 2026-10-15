use crate::components::primitives::{Label, Switch, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};
use gpui::{Context, Entity, FontWeight, Rgba, SharedString, Window, div, img, prelude::*};
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
        "ukr",
        "icons/flags/ukr.svg",
        "setting.language.ukrainian",
        "robits",
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
        "pl",
        "icons/flags/pl.svg",
        "setting.language.polish",
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
    (
        "blr",
        "icons/flags/blr.svg",
        "setting.language.belarusian",
        "deferensed",
    ),
    (
        "fr",
        "icons/flags/fr.svg",
        "setting.language.french",
        "robits",
    ),
    (
        "nl",
        "icons/flags/nl.svg",
        "setting.language.dutch",
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
            mezon_store::schedule_settings_save(&settings, cx);
        })
}

fn toggle_row(
    id: &'static str,
    title: SharedString,
    description: SharedString,
    enabled: bool,
    toggle: fn(&mut Settings),
    theme: &Theme,
    cx: &mut Context<LanguagePage>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            v_flex()
                .min_w_0()
                .gap_1()
                .child(
                    Label::new(title)
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary),
                )
                .child(
                    Label::new(description)
                        .text_xs()
                        .text_color(theme.text_muted),
                ),
        )
        .child(
            Switch::new(id)
                .checked(enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings.update(cx, |settings, _| toggle(settings));
                    mezon_store::schedule_settings_save(&this.settings, cx);
                    cx.notify();
                })),
        )
}

impl Render for LanguagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let theme = cx.theme().clone();
        let settings = self.settings.clone();
        let (spellcheck, quick_replies) = {
            let settings = self.settings.read(cx);
            (settings.spellcheck_enabled, settings.quick_replies_enabled)
        };
        v_flex().gap_8().child(
            v_flex()
                .rounded_lg()
                .bg(theme.bg_primary)
                .p_4()
                .gap_4()
                .child(toggle_row(
                    "spellcheck",
                    mezon_i18n::t(&locale, "setting.language.spellcheck").into(),
                    mezon_i18n::t(&locale, "setting.language.spellcheckDesc").into(),
                    spellcheck,
                    |settings| settings.spellcheck_enabled = !settings.spellcheck_enabled,
                    &theme,
                    cx,
                ))
                .child(toggle_row(
                    "quick-replies",
                    mezon_i18n::t(&locale, "setting.language.quickReplies").into(),
                    mezon_i18n::t(&locale, "setting.language.quickRepliesDesc").into(),
                    quick_replies,
                    |settings| settings.quick_replies_enabled = !settings.quick_replies_enabled,
                    &theme,
                    cx,
                ))
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
                                &theme,
                                settings.clone(),
                            )
                        },
                    )),
                ),
        )
    }
}
