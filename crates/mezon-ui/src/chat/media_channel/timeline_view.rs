use gpui::{Entity, FontWeight, SharedString, div, img, prelude::*, px, relative, rgb};
use mezon_store::{AppConfig, ChannelTimeline, TimelineCardSide, timeline_card_position};
use ui::Tooltip;

use crate::components::primitives::h_flex;
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

use super::media_image::render_media_image_cover;

pub fn render_timeline_header(
    theme: &Theme,
    locale: &str,
    first_year: Option<&str>,
    on_calendar: Option<std::sync::Arc<dyn Fn(&mut gpui::Window, &mut gpui::App)>>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_6()
        .py_4()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .flex_col()
                .when_some(first_year, |col, year| {
                    col.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.bg_button_primary)
                            .child(format!(
                                "{} {}",
                                mezon_i18n::t(
                                    locale,
                                    "channelCreator.fields.mediaHighlights.since"
                                ),
                                year
                            )),
                    )
                })
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(mezon_i18n::t(
                            locale,
                            "channelCreator.fields.mediaHighlights.eventsTitle",
                        )),
                ),
        )
        .when_some(on_calendar, |row, handler| {
            row.child(
                div()
                    .id("timeline-calendar")
                    .cursor_pointer()
                    .rounded_lg()
                    .p_2()
                    .hover(|el| el.bg(theme.bg_hover))
                    .tooltip(Tooltip::text(mezon_i18n::t(
                        locale,
                        "channelCreator.fields.eventDetail.calendar",
                    )))
                    .on_click(move |_, window, cx| handler(window, cx))
                    .child(img("icons/bullet-list-icon.svg").size(px(20.)).flex_none()),
            )
        })
}

pub fn render_event_card(
    theme: &Theme,
    locale: &str,
    event: &ChannelTimeline,
    index: usize,
    config: &AppConfig,
    image_cache: Entity<LruImageCache>,
    on_click: Option<std::sync::Arc<dyn Fn(i64, u32, &mut gpui::Window, &mut gpui::App)>>,
) -> impl IntoElement {
    let side = timeline_card_position(index);
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
    let date = if event.start_time_seconds > 0 {
        Some(format_event_date(event.start_time_seconds, locale))
    } else {
        None
    };

    div()
        .relative()
        .mb_8()
        .child(
            div()
                .absolute()
                .left(px(0.))
                .right(px(0.))
                .top(px(16.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(12.))
                        .h(px(12.))
                        .rounded_full()
                        .bg(theme.tokens.bg_button_primary)
                        .border_2()
                        .border_color(theme.bg_primary),
                ),
        )
        .when_some(date.as_ref(), |el, date| {
            el.child(
                div()
                    .absolute()
                    .top(px(8.))
                    .text_center()
                    .when(side == TimelineCardSide::Left, |el| {
                        el.left(relative(0.5)).ml(px(20.))
                    })
                    .when(side == TimelineCardSide::Right, |el| {
                        el.right(relative(0.5)).mr(px(20.))
                    })
                    .child(render_date_badge(theme, date)),
            )
        })
        .child(
            div()
                .flex()
                .w_full()
                .when(side == TimelineCardSide::Left, |el| el.justify_start())
                .when(side == TimelineCardSide::Right, |el| el.justify_end())
                .child(
                    div()
                        .w(relative(0.5))
                        .when(side == TimelineCardSide::Left, |el| el.pr(px(24.)))
                        .when(side == TimelineCardSide::Right, |el| el.pl(px(24.)))
                        .child(render_event_card_body(
                            theme,
                            locale,
                            event,
                            &preview_urls,
                            is_album,
                            on_click.is_some(),
                            image_cache,
                            on_click,
                        )),
                ),
        )
}

struct EventDateParts {
    month: String,
    day: String,
    year: String,
}

fn month_short(locale: &str, month_idx: u32) -> String {
    let key = match month_idx {
        0 => "channelCreator.monthsShort.0",
        1 => "channelCreator.monthsShort.1",
        2 => "channelCreator.monthsShort.2",
        3 => "channelCreator.monthsShort.3",
        4 => "channelCreator.monthsShort.4",
        5 => "channelCreator.monthsShort.5",
        6 => "channelCreator.monthsShort.6",
        7 => "channelCreator.monthsShort.7",
        8 => "channelCreator.monthsShort.8",
        9 => "channelCreator.monthsShort.9",
        10 => "channelCreator.monthsShort.10",
        _ => "channelCreator.monthsShort.11",
    };
    mezon_i18n::t(locale, key).to_string()
}

pub fn format_event_date_label(timestamp_seconds: u32, locale: &str) -> String {
    let parts = format_event_date(timestamp_seconds, locale);
    format!("{} {}, {}", parts.month, parts.day, parts.year)
}

fn format_event_date(timestamp_seconds: u32, locale: &str) -> EventDateParts {
    use chrono::{Datelike, TimeZone};
    let month_idx = chrono::Utc
        .timestamp_opt(timestamp_seconds as i64, 0)
        .single()
        .map(|dt| dt.month0())
        .unwrap_or(0);
    let (day, year) = chrono::Utc
        .timestamp_opt(timestamp_seconds as i64, 0)
        .single()
        .map(|dt| (format!("{:02}", dt.day()), dt.year().to_string()))
        .unwrap_or_else(|| ("00".to_string(), String::new()));
    EventDateParts {
        month: month_short(locale, month_idx),
        day,
        year,
    }
}

fn render_date_badge(theme: &Theme, date: &EventDateParts) -> impl IntoElement {
    div()
        .text_center()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.tokens.bg_button_primary)
                .child(date.month.clone()),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(date.day.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(date.year.clone()),
        )
}

fn render_event_card_body(
    theme: &Theme,
    locale: &str,
    event: &ChannelTimeline,
    preview_urls: &[SharedString],
    is_album: bool,
    interactive: bool,
    image_cache: Entity<LruImageCache>,
    on_click: Option<std::sync::Arc<dyn Fn(i64, u32, &mut gpui::Window, &mut gpui::App)>>,
) -> impl IntoElement {
    let event_id = event.id;
    let start_time_seconds = event.start_time_seconds;
    let mut card = div()
        .id(SharedString::from(format!("timeline-event-{}", event.id)))
        .w_full()
        .rounded_xl()
        .bg(theme.bg_primary)
        .border_1()
        .border_color(theme.border)
        .when(event.is_special(), |el| {
            el.border_2().border_color(theme.brand)
        })
        .p_4();
    if interactive && let Some(handler) = on_click {
        card = card
            .cursor_pointer()
            .hover(|el| el.bg(theme.bg_hover))
            .on_click(move |_, window, cx| handler(event_id, start_time_seconds, window, cx));
    }
    card.child(
        div()
            .flex()
            .flex_col()
            .when(event.is_special(), |col| {
                col.child(
                    h_flex().items_center().gap_2().mb_2().child(
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
                    ),
                )
            })
            .when(!event.title.is_empty(), |col| {
                col.child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .mb_1()
                        .child(event.title.clone()),
                )
            })
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
                col.child(render_preview_images(
                    theme,
                    preview_urls,
                    is_album,
                    image_cache,
                ))
            })
            .when(is_album, |col| {
                col.child(
                    div()
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

fn render_preview_images(
    theme: &Theme,
    preview_urls: &[SharedString],
    is_album: bool,
    image_cache: Entity<LruImageCache>,
) -> impl IntoElement {
    let bg = theme.bg_tertiary;
    let muted = theme.text_muted;
    if is_album {
        h_flex()
            .w_full()
            .gap_1()
            .rounded_lg()
            .overflow_hidden()
            .mb_2()
            .children(preview_urls.iter().take(2).map(|url| {
                div().flex_1().min_w_0().child(render_media_image_cover(
                    bg,
                    muted,
                    url.clone(),
                    px(80.),
                    image_cache.clone(),
                ))
            }))
    } else {
        div()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_event_date_label_matches_parts() {
        let locale = "en";
        let label = format_event_date_label(1_704_067_200, locale);
        assert!(label.contains("2024"));
        assert!(label.contains(','));
    }

    #[test]
    fn month_short_uses_i18n_keys() {
        let jan = month_short("en", 0);
        assert!(!jan.is_empty());
    }
}
