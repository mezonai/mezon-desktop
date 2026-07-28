use std::sync::Arc;

use gpui::{Entity, FontWeight, SharedString, div, prelude::*, px, rgb};
use mezon_store::{AppConfig, ChannelTimeline};

use crate::components::primitives::{Icon, IconName, Spinner};
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

use super::media_image::render_media_image_cover;
use super::timeline_view::format_event_date_label;

pub fn render_events_header(
    theme: &Theme,
    locale: &str,
    on_back: Arc<dyn Fn(&mut gpui::Window, &mut gpui::App)>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_6()
        .py_4()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .id("events-back")
                .cursor_pointer()
                .rounded_lg()
                .p_1()
                .hover(|el| el.bg(theme.bg_hover))
                .on_click(move |_, window, cx| on_back(window, cx))
                .child(
                    Icon::new(IconName::ArrowLeft)
                        .size(px(20.))
                        .text_color(theme.text_primary),
                ),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(
                    locale,
                    "channelCreator.fields.familyEvents.title",
                )),
        )
}

pub fn render_year_tabs(
    theme: &Theme,
    years: &[i32],
    selected_year: i32,
    on_select: Arc<dyn Fn(i32, &mut gpui::Window, &mut gpui::App)>,
) -> impl IntoElement {
    div()
        .id("events-year-tabs")
        .flex()
        .flex_row()
        .gap_2()
        .px_6()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .overflow_x_scroll()
        .children(years.iter().map(|year| {
            let active = *year == selected_year;
            let on_select = on_select.clone();
            let year_val = *year;
            div()
                .id(SharedString::from(format!("events-year-{year_val}")))
                .px_4()
                .py(px(6.))
                .rounded_full()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(theme.tokens.bg_button_primary)
                        .text_color(theme.text_primary)
                })
                .when(!active, |el| {
                    el.bg(theme.bg_tertiary)
                        .text_color(theme.text_muted)
                        .hover(|el| el.bg(theme.bg_hover))
                })
                .on_click(move |_, window, cx| on_select(year_val, window, cx))
                .child(year_val.to_string())
        }))
}

pub fn render_events_list_header(theme: &Theme, locale: &str, year: i32) -> impl IntoElement {
    div().px_6().py_3().child(
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text_muted)
            .child(format!(
                "{} {}",
                mezon_i18n::t(locale, "channelCreator.fields.familyEvents.eventsIn"),
                year
            )),
    )
}

pub fn render_events_empty(theme: &Theme, locale: &str, year: i32) -> impl IntoElement {
    div().flex().items_center().justify_center().py_10().child(
        div().text_color(theme.text_muted).child(format!(
            "{} {}",
            mezon_i18n::t(locale, "channelCreator.fields.familyEvents.noEvents"),
            year
        )),
    )
}

pub fn render_events_list_card(
    theme: &Theme,
    locale: &str,
    event: &ChannelTimeline,
    config: &AppConfig,
    image_cache: Entity<LruImageCache>,
    on_click: Arc<dyn Fn(i64, u32, &mut gpui::Window, &mut gpui::App)>,
) -> impl IntoElement {
    let raw_preview_urls = event.preview_urls();
    let is_album = raw_preview_urls.len() > 1;
    let preview_urls: Vec<SharedString> = raw_preview_urls
        .into_iter()
        .map(|url| {
            if url.starts_with("http") {
                if is_album {
                    config.timeline_album_thumb_proxy(url).into()
                } else {
                    config.timeline_single_thumb_proxy(url).into()
                }
            } else {
                url.into()
            }
        })
        .collect();
    let date = event
        .start_time_seconds
        .gt(&0)
        .then(|| format_event_date_label(event.start_time_seconds, locale));
    let event_id = event.id;
    let start_time_seconds = event.start_time_seconds;

    div()
        .id(SharedString::from(format!("events-card-{}", event.id)))
        .w_full()
        .cursor_pointer()
        .rounded_xl()
        .bg(theme.bg_primary)
        .border_1()
        .border_color(theme.border)
        .when(event.is_special(), |el| {
            el.border_2().border_color(theme.brand)
        })
        .p_4()
        .hover(|el| el.bg(theme.bg_hover))
        .on_click(move |_, window, cx| on_click(event_id, start_time_seconds, window, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .mb_1()
                        .when(event.is_special(), |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .bg(theme.tokens.bg_button_primary)
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xffffff))
                                    .child(mezon_i18n::t(
                                        locale,
                                        "channelCreator.fields.mediaHighlights.special",
                                    )),
                            )
                        })
                        .when(!event.title.is_empty(), |row| {
                            row.child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(event.title.clone()),
                            )
                        }),
                )
                .when(!event.description.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .mb_3()
                            .line_clamp(2)
                            .child(event.description.clone()),
                    )
                })
                .when(!preview_urls.is_empty(), |col| {
                    col.child(render_list_previews(
                        theme,
                        &preview_urls,
                        is_album,
                        image_cache,
                    ))
                })
                .when_some(date.as_ref(), |col, date| {
                    col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .mt_2()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(
                                Icon::new(IconName::History)
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            )
                            .child(date.clone()),
                    )
                })
                .when(is_album, |col| {
                    col.child(
                        div()
                            .mt_2()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.tokens.bg_button_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "channelCreator.fields.mediaHighlights.viewAlbum",
                            )),
                    )
                }),
        )
}

fn render_list_previews(
    theme: &Theme,
    preview_urls: &[SharedString],
    is_album: bool,
    image_cache: Entity<LruImageCache>,
) -> impl IntoElement {
    let bg = theme.bg_tertiary;
    let muted = theme.text_muted;
    if is_album {
        div()
            .grid()
            .grid_cols(2)
            .gap_1()
            .rounded_lg()
            .overflow_hidden()
            .max_w(px(320.))
            .mb_2()
            .children(preview_urls.iter().take(2).map(|url| {
                div().min_w_0().child(render_media_image_cover(
                    bg,
                    muted,
                    url.clone(),
                    px(80.),
                    image_cache.clone(),
                ))
            }))
    } else {
        div()
            .w(px(192.))
            .mb_2()
            .rounded_lg()
            .overflow_hidden()
            .child(render_media_image_cover(
                bg,
                muted,
                preview_urls[0].clone(),
                px(112.),
                image_cache,
            ))
    }
}

pub fn render_events_loading() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .py_10()
        .child(Spinner::new())
}

pub fn year_list() -> Vec<i32> {
    use chrono::Datelike;
    let current = chrono::Utc::now().year();
    (0..=5).map(|offset| current - offset).collect()
}
