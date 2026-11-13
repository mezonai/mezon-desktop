use gpui::{
    Animation, AnimationExt, AnyElement, Context, Entity, FontWeight, Hsla, ObjectFit,
    SharedString, div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb,
};
use mezon_store::{COMMUNITY_HASHTAGS, MAX_COMMUNITY_HASHTAGS};
use std::time::Duration;

use super::{
    ABOUT_MAX_LEN, COMMUNITY_BANNER_PREVIEW_H, COMMUNITY_BANNER_PREVIEW_W, CommunitySettingPage,
    DESCRIPTION_MAX_LEN, VANITY_URL_MAX_LEN,
};
use crate::components::primitives::{
    Button, ButtonVariants, FocusCycle, Icon, IconName, Input, Sizable, Size, TextArea,
    TextAreaField, Tooltip, h_flex, v_flex,
};
use crate::theme::Theme;
use crate::util::imgproxy;

const MODAL_WIDTH: f32 = 512.0;
const MODAL_MAX_HEIGHT: f32 = 0.9;
const HEADER_GRADIENT_START: u32 = 0x3b82f6;
const HEADER_GRADIENT_END: u32 = 0x9333ea;
const HEADER_SUBTITLE: u32 = 0xdbeafe;
const ACCENT_BLUE: u32 = 0x3b82f6;
const ICON_GREEN: u32 = 0x22c55e;
const ICON_INDIGO: u32 = 0x6366f1;
const ICON_PURPLE: u32 = 0xa855f7;
const CHANGE_BANNER_BG: u32 = 0x818cf8;
const CHANGE_BANNER_HOVER_BG: u32 = 0x6366f1;
const BANNER_PREVIEW_HEIGHT: f32 = 160.0;
const BANNER_EMPTY_HEIGHT: f32 = 192.0;
const BANNER_HOVER_TRANSITION: Duration = Duration::from_millis(300);

fn hover_transition(
    element: gpui::Div,
    id: &'static str,
    hovered: Option<bool>,
    apply: impl Fn(gpui::Div, f32) -> gpui::Div + 'static,
) -> AnyElement {
    match hovered {
        None => apply(element, 0.0).into_any_element(),
        Some(entering) => element
            .with_animation(
                (id, usize::from(entering)),
                Animation::new(BANNER_HOVER_TRANSITION).with_easing(gpui::ease_in_out),
                move |el, delta| apply(el, if entering { delta } else { 1.0 - delta }),
            )
            .into_any_element(),
    }
}

pub(super) fn hashtag_label_key(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "Gaming" => "onBoardingClan.communitySettings.hashtags.items.Gaming",
        "Technology" => "onBoardingClan.communitySettings.hashtags.items.Technology",
        "Education" => "onBoardingClan.communitySettings.hashtags.items.Education",
        "Entertainment" => "onBoardingClan.communitySettings.hashtags.items.Entertainment",
        "Music" => "onBoardingClan.communitySettings.hashtags.items.Music",
        "Art" => "onBoardingClan.communitySettings.hashtags.items.Art",
        "Crypto" => "onBoardingClan.communitySettings.hashtags.items.Crypto",
        "Anime" => "onBoardingClan.communitySettings.hashtags.items.Anime",
        "Sports" => "onBoardingClan.communitySettings.hashtags.items.Sports",
        "Community" => "onBoardingClan.communitySettings.hashtags.items.Community",
        _ => return None,
    })
}

fn hashtag_label(locale: &str, tag: &'static str) -> &'static str {
    hashtag_label_key(tag).map_or(tag, |key| mezon_i18n::t(locale, key))
}

fn section_label(
    id: &'static str,
    icon: IconName,
    icon_color: impl Into<Hsla>,
    text: &'static str,
    required: bool,
    locale: &str,
    theme: &Theme,
) -> impl IntoElement {
    let label = h_flex()
        .id(id)
        .gap_2()
        .items_center()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.tokens.text_theme_primary)
        .child(Icon::new(icon).size(px(20.0)).color(icon_color))
        .child(div().whitespace_nowrap().child(text))
        .when(required, |label| {
            label.child(
                div()
                    .id("required")
                    .cursor_pointer()
                    .text_color(theme.danger_text)
                    .tooltip(Tooltip::text(mezon_i18n::t(
                        locale,
                        "onBoardingClan.communitySettings.warnningEmptyField",
                    )))
                    .child("*"),
            )
        });
    Icon::motion_scope(label, true)
}

impl CommunitySettingPage {
    pub(super) fn render_enable_modal(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .id("community-enable-modal")
            .w(px(MODAL_WIDTH))
            .max_w_full()
            .max_h(relative(MODAL_MAX_HEIGHT))
            .overflow_hidden()
            .rounded(px(16.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.bg_surface)
            .shadow_lg()
            .focus_cycle(self.input_focus_handles(cx))
            .child(self.render_modal_header(locale, cx))
            .child(
                div()
                    .id("community-enable-modal-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        self.render_community_fields(locale, theme, true, cx)
                            .child(
                                h_flex()
                                    .justify_end()
                                    .pt_6()
                                    .border_t_1()
                                    .border_color(theme.border)
                                    .child(
                                        Button::new("community-setup-save")
                                            .label(mezon_i18n::t(
                                                locale,
                                                if self.saving {
                                                    "onBoardingClan.communitySettings.buttons.saving"
                                                } else {
                                                    "onBoardingClan.communitySettings.buttons.enableAndSave"
                                                },
                                            ))
                                            .primary()
                                            .with_size(Size::Large)
                                            .disabled(self.saving)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.save(true, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_community_fields(
        &self,
        locale: &str,
        theme: &Theme,
        enable_flow: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        v_flex()
            .w_full()
            .p_6()
            .gap_6()
            .child(self.render_modal_banner(locale, theme, enable_flow, cx))
            .when_some(self.description_input.as_ref(), |el, input| {
                el.child(self.render_modal_textarea(
                    "community-enable-description-required",
                    mezon_i18n::t(locale, "onBoardingClan.communitySettings.description.title"),
                    input,
                    self.draft.description.chars().count(),
                    DESCRIPTION_MAX_LEN,
                    self.field_errors.description,
                    locale,
                    theme,
                ))
            })
            .when_some(self.about_input.as_ref(), |el, input| {
                el.child(self.render_modal_textarea(
                    "community-enable-about-required",
                    mezon_i18n::t(locale, "onBoardingClan.communitySettings.about.title"),
                    input,
                    self.draft.about.chars().count(),
                    ABOUT_MAX_LEN,
                    self.field_errors.about,
                    locale,
                    theme,
                ))
            })
            .child(self.render_hashtag_section(locale, theme, cx))
            .child(self.render_modal_vanity_url(locale, theme, cx))
    }

    fn render_modal_header(&self, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .relative()
            .flex_shrink_0()
            .rounded_t(px(16.0))
            .p_6()
            .pr(px(56.0))
            .bg(linear_gradient(
                90.0,
                linear_color_stop(rgb(HEADER_GRADIENT_START), 0.0),
                linear_color_stop(rgb(HEADER_GRADIENT_END), 1.0),
            ))
            .text_color(gpui::white())
            .child(
                div()
                    .mb_2()
                    .text_size(px(30.0))
                    .font_weight(FontWeight::BOLD)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.communitySettings.enableTitle",
                    )),
            )
            .child(div().text_color(rgb(HEADER_SUBTITLE)).child(mezon_i18n::t(
                locale,
                "onBoardingClan.communitySettings.enableSubtitle",
            )))
            .child(Icon::motion_scope(
                div()
                    .id("community-enable-modal-close")
                    .absolute()
                    .top(px(16.0))
                    .right(px(16.0))
                    .p_2()
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(gpui::white().alpha(0.8))
                    .hover(|style| style.bg(gpui::white().alpha(0.2)).text_color(gpui::white()))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(20.0))
                            .color(gpui::white().alpha(0.8)),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_setup(window, cx);
                    })),
                true,
            ))
    }

    fn render_modal_banner(
        &self,
        locale: &str,
        theme: &Theme,
        enable_flow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let banner = &self.draft.community_banner;
        let has_banner = !banner.is_empty();
        let banner_error = self.field_errors.banner;
        let uploading = self.banner_uploading;
        let radius = px(12.0);

        let content = if has_banner {
            let banner_src = imgproxy::proxied(
                cx,
                banner,
                COMMUNITY_BANNER_PREVIEW_W,
                COMMUNITY_BANNER_PREVIEW_H,
                "fit",
            );
            let hovered = self.banner_hovered;
            div()
                .id("community-enable-banner-preview")
                .relative()
                .w_full()
                .h(px(BANNER_PREVIEW_HEIGHT))
                .rounded(radius)
                .overflow_hidden()
                .shadow_lg()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if this.banner_hovered != Some(*hovered) {
                        this.banner_hovered = Some(*hovered);
                        cx.notify();
                    }
                }))
                .child(
                    img(banner_src)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Cover),
                )
                .child(hover_transition(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius)
                        .bg(gpui::black().alpha(0.2))
                        .child(
                            div()
                                .id("community-enable-change-banner")
                                .px_6()
                                .py_3()
                                .rounded(px(8.0))
                                .bg(rgb(CHANGE_BANNER_BG))
                                .text_color(gpui::white())
                                .font_weight(FontWeight::MEDIUM)
                                .shadow_lg()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(CHANGE_BANNER_HOVER_BG)))
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.communitySettings.banner.changeBanner",
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if !this.banner_uploading {
                                        this.pick_banner(cx);
                                    }
                                })),
                        ),
                    "community-enable-banner-overlay",
                    hovered,
                    |el, progress| el.opacity(progress),
                ))
                .when(enable_flow, |preview| {
                    preview.child(Icon::motion_scope(
                        div()
                            .id("community-enable-remove-banner")
                            .absolute()
                            .top(px(8.0))
                            .right(px(8.0))
                            .p_2()
                            .rounded_full()
                            .bg(gpui::white().alpha(0.8))
                            .shadow_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(gpui::white()))
                            .tooltip(Tooltip::text(mezon_i18n::t(
                                locale,
                                "onBoardingClan.communitySettings.banner.removeBanner",
                            )))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(20.0))
                                    .color(theme.danger_text),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.remove_banner(cx))),
                        true,
                    ))
                })
                .into_any_element()
        } else {
            let upload = v_flex()
                .id("community-enable-upload-banner")
                .w_full()
                .h(px(BANNER_EMPTY_HEIGHT))
                .items_center()
                .justify_center()
                .gap_1()
                .px_4()
                .rounded(radius)
                .border_2()
                .border_dashed()
                .border_color(if banner_error {
                    theme.danger_text
                } else {
                    theme.tokens.theme_border_input
                })
                .bg(theme.tokens.bg_surface)
                .text_color(theme.tokens.text_theme_primary)
                .cursor_pointer()
                .hover(|style| {
                    style
                        .border_color(rgb(ACCENT_BLUE))
                        .text_color(rgb(ACCENT_BLUE))
                        .bg(Hsla::from(rgb(ACCENT_BLUE)).alpha(0.08))
                })
                .child(
                    div()
                        .mb_2()
                        .child(Icon::new(IconName::ImageAdd).size(px(48.0))),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::MEDIUM)
                        .text_center()
                        .child(mezon_i18n::t(
                            locale,
                            "onBoardingClan.communitySettings.banner.uploadTitle",
                        )),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .opacity(0.75)
                        .child(mezon_i18n::t(
                            locale,
                            "onBoardingClan.communitySettings.banner.uploadDescription",
                        )),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.banner_uploading {
                        this.pick_banner(cx);
                    }
                }));
            Icon::motion_scope(upload, true).into_any_element()
        };

        v_flex()
            .gap_4()
            .child(section_label(
                "community-enable-banner-required",
                IconName::Image,
                rgb(ACCENT_BLUE),
                mezon_i18n::t(locale, "onBoardingClan.communitySettings.banner.title"),
                enable_flow,
                locale,
                theme,
            ))
            .child(
                div()
                    .relative()
                    .w_full()
                    .rounded(radius)
                    .when(banner_error && has_banner, |el| {
                        el.border_2().border_color(theme.danger_text)
                    })
                    .child(content)
                    .when(uploading, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(radius)
                                .occlude()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(gpui::black().alpha(0.45))
                                .child(div().text_sm().text_color(gpui::white()).child(
                                    mezon_i18n::t(
                                        locale,
                                        "onBoardingClan.communitySettings.buttons.saving",
                                    ),
                                )),
                        )
                    }),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_modal_textarea(
        &self,
        required_id: &'static str,
        title: &'static str,
        input: &Entity<TextArea>,
        len: usize,
        max: usize,
        has_error: bool,
        locale: &str,
        theme: &Theme,
    ) -> impl IntoElement {
        v_flex()
            .gap_4()
            .child(section_label(
                required_id,
                IconName::Info,
                rgb(ICON_GREEN),
                title,
                true,
                locale,
                theme,
            ))
            .child(
                div()
                    .relative()
                    .w_full()
                    .overflow_hidden()
                    .rounded(px(12.0))
                    .border_2()
                    .border_color(if has_error {
                        theme.danger_text
                    } else {
                        theme.tokens.theme_border_input
                    })
                    .child(TextAreaField::new(input))
                    .child(
                        div()
                            .absolute()
                            .bottom(px(10.0))
                            .right(px(10.0))
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_surface)
                            .child(Self::render_char_counter(len, max, theme)),
                    ),
            )
    }

    fn render_modal_vanity_url(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let vanity_len = self.draft.short_url.chars().count();
        let has_error = self.field_errors.vanity_url;

        v_flex()
            .gap_3()
            .child(section_label(
                "community-enable-vanity-required",
                IconName::Link,
                rgb(ICON_PURPLE),
                mezon_i18n::t(locale, "onBoardingClan.communitySettings.vanityUrl.title"),
                true,
                locale,
                theme,
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .opacity(0.75)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.communitySettings.vanityUrl.description",
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .rounded(px(12.0))
                    .border_2()
                    .border_color(if has_error {
                        theme.danger_text
                    } else {
                        theme.tokens.theme_border_input
                    })
                    .overflow_hidden()
                    .bg(theme.tokens.bg_tertiary)
                    .child(
                        div()
                            .flex_shrink_0()
                            .h(px(40.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_item_hover)
                            .text_sm()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(self.vanity_url_prefix(cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when_some(self.vanity_input.as_ref(), |el, input| {
                                el.child(Input::new(input))
                            }),
                    ),
            )
            .when(!self.draft.short_url.is_empty(), |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .p_3()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(Hsla::from(rgb(ACCENT_BLUE)).alpha(0.4))
                        .text_sm()
                        .text_color(theme.text_link)
                        .child(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).child(
                            mezon_i18n::t(
                                locale,
                                "onBoardingClan.communitySettings.vanityUrl.previewLabel",
                            ),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(self.preview_url(cx)),
                        ),
                )
            })
            .child(h_flex().justify_end().child(Self::render_char_counter(
                vanity_len,
                VANITY_URL_MAX_LEN,
                theme,
            )))
    }

    pub(super) fn render_hashtag_section(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = self.draft.hashtags.len();
        let has_error = self.field_errors.hashtags;
        let status: &'static str = if count >= MAX_COMMUNITY_HASHTAGS {
            mezon_i18n::t(
                locale,
                "onBoardingClan.communitySettings.hashtags.maxReached",
            )
        } else if count == 1 {
            mezon_i18n::t(
                locale,
                "onBoardingClan.communitySettings.hashtags.minRequired",
            )
        } else {
            ""
        };
        let selected_count: SharedString = mezon_i18n::t(
            locale,
            "onBoardingClan.communitySettings.hashtags.selectedCount",
        )
        .replace("{{count}}", &count.to_string())
        .replace("{{max}}", &MAX_COMMUNITY_HASHTAGS.to_string())
        .into();
        let mut chips = Vec::with_capacity(COMMUNITY_HASHTAGS.len());
        for (index, tag) in COMMUNITY_HASHTAGS.iter().enumerate() {
            chips.push(self.render_hashtag_chip(index, tag, locale, theme, cx));
        }

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .child(section_label(
                        "community-hashtags-required",
                        IconName::Channel,
                        rgb(ICON_INDIGO),
                        mezon_i18n::t(locale, "onBoardingClan.communitySettings.hashtags.title"),
                        true,
                        locale,
                        theme,
                    ))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .items_center()
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_surface)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(format!("{count}/{MAX_COMMUNITY_HASHTAGS}")),
                            )
                            .child(h_flex().gap_1().children((0..MAX_COMMUNITY_HASHTAGS).map(
                                |index| {
                                    div().size(px(8.0)).rounded_full().map(|dot| {
                                        if index < count {
                                            dot.bg(rgb(ACCENT_BLUE))
                                        } else {
                                            dot.bg(theme.tokens.text_theme_primary).opacity(0.25)
                                        }
                                    })
                                },
                            ))),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .opacity(0.75)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.communitySettings.hashtags.description",
                    )),
            )
            .child(
                v_flex()
                    .p(px(14.0))
                    .rounded(px(16.0))
                    .bg(Hsla::from(theme.tokens.theme_input).opacity(0.4))
                    .map(|el| {
                        if has_error {
                            el.border_2().border_color(theme.danger_text)
                        } else {
                            el.border_1().border_color(theme.border)
                        }
                    })
                    .child(h_flex().flex_wrap().gap_2().children(chips))
                    .child(
                        h_flex()
                            .mt_3()
                            .pt(px(10.0))
                            .gap_3()
                            .justify_between()
                            .border_t_2()
                            .border_color(theme.tokens.theme_border_input)
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary)
                            .opacity(0.7)
                            .child(div().whitespace_nowrap().child(selected_count))
                            .child(div().whitespace_nowrap().child(status)),
                    ),
            )
    }

    fn render_hashtag_chip(
        &self,
        index: usize,
        tag: &'static str,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.draft.has_hashtag(tag);
        let can_toggle = self.draft.can_toggle_hashtag(tag);
        let blocked_tooltip = (!can_toggle).then(|| {
            mezon_i18n::t(
                locale,
                if selected {
                    "onBoardingClan.communitySettings.hashtags.minRequired"
                } else {
                    "onBoardingClan.communitySettings.hashtags.maxReached"
                },
            )
        });

        let chip = h_flex()
            .id(("community-hashtag", index))
            .gap_2()
            .items_center()
            .px(px(14.0))
            .py_2()
            .rounded(px(12.0))
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .map(|chip| {
                if selected {
                    chip.border_2()
                        .border_color(rgb(ACCENT_BLUE))
                        .bg(theme.tokens.bg_item_hover)
                        .text_color(theme.text_primary)
                        .shadow_md()
                        .cursor_pointer()
                } else if can_toggle {
                    chip.border_1()
                        .border_color(theme.border)
                        .bg(theme.tokens.theme_input)
                        .text_color(theme.tokens.text_theme_primary)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.tokens.bg_item_hover))
                } else {
                    chip.border_1()
                        .border_color(theme.border)
                        .bg(Hsla::from(theme.tokens.theme_input).opacity(0.2))
                        .text_color(theme.tokens.text_theme_primary)
                        .opacity(0.35)
                        .cursor_not_allowed()
                }
            })
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(ACCENT_BLUE))
                    .child("#"),
            )
            .child(hashtag_label(locale, tag))
            .when(selected, |chip| {
                chip.child(
                    div()
                        .size(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(ACCENT_BLUE))
                        .child(
                            Icon::new(IconName::Check)
                                .size(px(10.0))
                                .color(gpui::white()),
                        ),
                )
            })
            .when_some(blocked_tooltip, |chip, text| {
                chip.tooltip(Tooltip::text(text))
            })
            .when(can_toggle, |chip| {
                chip.on_click(cx.listener(move |this, _, _, cx| this.toggle_hashtag(tag, cx)))
            });
        Icon::motion_scope(chip, true).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_hashtag_has_a_translated_label() {
        for tag in COMMUNITY_HASHTAGS {
            let key =
                hashtag_label_key(tag).unwrap_or_else(|| panic!("hashtag {tag} has no label key"));
            assert_ne!(mezon_i18n::t("en", key), key, "missing en label for {tag}");
            assert_ne!(mezon_i18n::t("vi", key), key, "missing vi label for {tag}");
        }
    }

    #[test]
    fn unknown_hashtag_falls_back_to_its_id() {
        assert_eq!(hashtag_label_key("Cooking"), None);
        assert_eq!(hashtag_label("en", "Cooking"), "Cooking");
        assert_eq!(hashtag_label("en", "Anime"), "Anime & Manga");
    }
}
