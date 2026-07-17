use std::ops::Range;

use gpui::{
    AnyElement, App, FontWeight, HighlightStyle, Hsla, InteractiveText, ObjectFit, Pixels,
    SharedString, StyledText, UnderlineStyle, div, img, prelude::*, px, relative, rems, rgb,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, LinkKind, Message, MessageCode, MessageSpan,
    PlatformStore, ProfileContext, RichClick, RichRunKind, RichToken, UserId, is_here_user_id,
};

use ui::Clickable;

use super::context::{RichTextRenderPlan, RowCtx};
use super::inline_content::{ClickRegion, IconOverlay, InlineContent, StyledRun};
use crate::app::shell::Shell;
use crate::chat::user_profile_popover::{ClickableContainer, UserProfilePopover};
use crate::components::primitives::{Icon, IconName};
use crate::router::{Route, navigate};
use crate::theme::Theme;

const BUZZ_RED: u32 = 0xef_44_44;
const CANVAS_TEXT: u32 = 0x32_97_ff;
const CANVAS_BG: u32 = 0x3c_42_70;
const CANVAS_HOVER_BG: u32 = 0x58_65_f2;
const CANVAS_HOVER_TEXT: u32 = 0xff_ff_ff;
const YOUTUBE_ACCENT: u32 = 0xff_00_1f;
const TIKTOK_ACCENT: u32 = 0xff_00_50;
const FACEBOOK_ACCENT: u32 = 0x18_77_f2;
const SOCIAL_CARD_BG: u32 = 0x2b_2d_31;
const EMOJI_SIZE: f32 = 24.;
const EMOJI_JUMBO_SIZE: f32 = 48.;
const INLINE_ICON_PLACEHOLDER: char = '\u{2800}';
const RICH_TEXT_PLAN_LIMIT: usize = 512;

struct ContentRenderOptions {
    body_color: gpui::Rgba,
    mentions_only: bool,
    inline: bool,
}

pub fn render_message_content(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let body_color = if msg.code == MessageCode::MessageBuzz {
        rgb(BUZZ_RED)
    } else {
        ctx.theme.tokens.text_theme_message
    };
    let content = render_message_content_with_options(
        msg,
        ctx,
        ContentRenderOptions {
            body_color,
            mentions_only: false,
            inline: false,
        },
    );
    match msg.invite.as_deref() {
        Some(invite) => div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .child(content)
            .child(super::invite_card::render_invite_card(invite, ctx))
            .into_any_element(),
        None => content,
    }
}

pub fn render_system_message_content(
    msg: &Message,
    ctx: &RowCtx,
    mentions_only: bool,
) -> AnyElement {
    render_message_content_with_options(
        msg,
        ctx,
        ContentRenderOptions {
            body_color: ctx.theme.tokens.text_theme_primary,
            mentions_only,
            inline: mentions_only,
        },
    )
}

pub fn render_spans(
    spans: &[MessageSpan],
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    text_size: Pixels,
) -> AnyElement {
    let mut row = rich_content_row(body_color, false).text_size(text_size);
    for span in spans {
        row = append_span(row, span, ctx, body_color, px(EMOJI_SIZE));
    }
    row.into_any_element()
}

pub fn render_embed_description(
    spans: &[MessageSpan],
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    text_size: Pixels,
) -> AnyElement {
    let mut column = div().flex().flex_col().w_full().min_w_0();
    let mut index = 0;
    while index < spans.len() {
        if matches!(spans[index], MessageSpan::CodeBlock { .. }) {
            column = column.child(render_spans(
                &spans[index..index + 1],
                ctx,
                body_color,
                text_size,
            ));
            index += 1;
        } else {
            let start = index;
            while index < spans.len() && !matches!(spans[index], MessageSpan::CodeBlock { .. }) {
                index += 1;
            }
            column = column.child(render_spans(
                &spans[start..index],
                ctx,
                body_color,
                text_size,
            ));
        }
    }
    column.into_any_element()
}

fn render_message_content_with_options(
    msg: &Message,
    ctx: &RowCtx,
    options: ContentRenderOptions,
) -> AnyElement {
    let theme = ctx.theme;
    let body_color = options.body_color;

    if msg.is_deleted_placeholder && !options.mentions_only {
        return render_deleted_placeholder(msg, theme);
    }

    if msg.spans.is_empty() && !msg.is_edited {
        return div().into_any_element();
    }

    if options.mentions_only {
        return render_mention_only_content(msg, ctx, body_color, options.inline);
    }

    let has_tokens = msg.spans.iter().any(|s| !matches!(s, MessageSpan::Text(_)));

    if !has_tokens && !msg.is_edited {
        return render_plain_text_spans(&msg.spans, body_color);
    }

    let needs_chip_path = spans_need_chip_path(&msg.spans);

    if is_link_only(&msg.spans) && !msg.is_edited && !needs_chip_path {
        return render_link_only_spans(&msg.spans, theme);
    }

    let has_code_block = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::CodeBlock { .. }));
    let has_custom_emoji = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::Emoji { emoji_id, .. } if !emoji_id.is_empty()));
    if !options.inline && !has_code_block && !has_custom_emoji && !needs_chip_path {
        return render_rich_styled(msg, ctx, body_color);
    }

    let emoji_size = if msg.is_only_emoji {
        px(EMOJI_JUMBO_SIZE)
    } else {
        px(EMOJI_SIZE)
    };
    if !options.inline
        && !msg.is_edited
        && let Some(inline) = build_inline_content(msg, ctx, body_color)
    {
        return inline;
    }
    let mut row = rich_content_row(body_color, options.inline);
    match msg
        .rich_layout
        .as_deref()
        .and_then(|layout| layout.content_tokens.as_deref())
    {
        Some(tokens) => {
            for token in tokens {
                row = match token {
                    RichToken::Word(word) => row.child(word.clone()),
                    RichToken::LineBreak => row.child(div().w_full().h_0()),
                    RichToken::Span(index) => append_span(
                        row,
                        &msg.spans[*index as usize],
                        ctx,
                        body_color,
                        emoji_size,
                    ),
                };
            }
        }
        None => {
            for span in &msg.spans {
                row = append_span(row, span, ctx, body_color, emoji_size);
            }
        }
    }
    if msg.is_edited {
        row = row.child(edited_marker(theme, ctx.locale));
    }
    row.into_any_element()
}

fn spans_need_chip_path(spans: &[MessageSpan]) -> bool {
    spans.iter().any(|s| match s {
        MessageSpan::Canvas { .. } | MessageSpan::Heading { .. } | MessageSpan::Hashtag { .. } => {
            true
        }
        MessageSpan::Link { kind, .. } => *kind != LinkKind::Plain,
        _ => false,
    })
}

fn render_deleted_placeholder(msg: &Message, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .min_h(px(30.))
        .italic()
        .text_base()
        .line_height(rems(1.375))
        .text_color(theme.tokens.text_theme_primary)
        .child(SharedString::from(msg.content.clone()))
        .into_any_element()
}

fn rich_text_plan_matches(
    plan: &RichTextRenderPlan,
    layout: &std::sync::Arc<mezon_store::RichLayout>,
    colors: [Hsla; 5],
    edited: bool,
) -> bool {
    std::sync::Arc::ptr_eq(&plan.layout, layout) && plan.colors == colors && plan.edited == edited
}

fn render_rich_styled(msg: &Message, ctx: &RowCtx, body_color: gpui::Rgba) -> AnyElement {
    let theme = ctx.theme;
    let mention_color: Hsla = theme.tokens.mention_color.into();
    let mention_bg: Hsla = theme.tokens.mention_primary.into();
    let code_bg: Hsla = theme.tokens.bg_markdown_code.into();
    let link_color: Hsla = theme.tokens.mention_color.into();
    let colors = [
        mention_color,
        mention_bg,
        code_bg,
        link_color,
        theme.text_muted.into(),
    ];
    let Some(layout) = msg.rich_layout.as_ref() else {
        return div().into_any_element();
    };
    let cached = ctx.row_memo.borrow().rich_text.get(&msg.id).cloned();
    let plan = match cached {
        Some(plan) if rich_text_plan_matches(&plan, layout, colors, msg.is_edited) => plan,
        _ => {
            let mut highlights: Vec<(Range<usize>, HighlightStyle)> =
                Vec::with_capacity(layout.runs.len() + usize::from(msg.is_edited));
            let mut font_overrides: Vec<(Range<usize>, SharedString)> = Vec::new();
            let mut click_ranges: Vec<Range<usize>> = Vec::new();
            let mut actions: Vec<RichClick> = Vec::new();
            for run in layout.runs.iter() {
                match run.kind {
                    RichRunKind::Bold => highlights.push((
                        run.range.clone(),
                        HighlightStyle {
                            font_weight: Some(FontWeight::BOLD),
                            ..Default::default()
                        },
                    )),
                    RichRunKind::Code => {
                        highlights.push((
                            run.range.clone(),
                            HighlightStyle {
                                background_color: Some(code_bg),
                                ..Default::default()
                            },
                        ));
                        font_overrides.push((run.range.clone(), "monospace".into()));
                    }
                    RichRunKind::Link => highlights.push((
                        run.range.clone(),
                        HighlightStyle {
                            color: Some(link_color),
                            underline: Some(UnderlineStyle {
                                thickness: px(1.),
                                color: Some(link_color),
                                wavy: false,
                            }),
                            ..Default::default()
                        },
                    )),
                    RichRunKind::Mention | RichRunKind::Hashtag => highlights.push((
                        run.range.clone(),
                        HighlightStyle {
                            color: Some(mention_color),
                            background_color: Some(mention_bg),
                            ..Default::default()
                        },
                    )),
                }
                if let Some(click) = run.click.clone() {
                    click_ranges.push(run.range.clone());
                    actions.push(click);
                }
            }

            let text = if msg.is_edited {
                let marker = mezon_i18n::t(ctx.locale, "message.edited");
                let mut edited = String::with_capacity(layout.text.len() + 1 + marker.len());
                edited.push_str(&layout.text);
                edited.push(' ');
                let start = edited.len();
                edited.push_str(marker);
                highlights.push((
                    start..edited.len(),
                    HighlightStyle {
                        color: Some(colors[4]),
                        ..Default::default()
                    },
                ));
                edited.into()
            } else {
                layout.text.clone()
            };
            let plan = RichTextRenderPlan {
                layout: layout.clone(),
                colors,
                edited: msg.is_edited,
                text,
                highlights: highlights.into(),
                font_overrides: font_overrides.into(),
                click_ranges: click_ranges.into(),
                actions: actions.into(),
                locale: SharedString::from(ctx.locale),
            };
            let mut memo = ctx.row_memo.borrow_mut();
            if memo.rich_text.len() >= RICH_TEXT_PLAN_LIMIT && !memo.rich_text.contains_key(&msg.id)
            {
                memo.rich_text.clear();
            }
            memo.rich_text.insert(msg.id, plan.clone());
            plan
        }
    };

    let mut styled =
        StyledText::new(plan.text.clone()).with_shared_highlights(plan.highlights.clone());
    if !plan.font_overrides.is_empty() {
        styled = styled.with_shared_font_family_overrides(plan.font_overrides.clone());
    }

    if plan.actions.is_empty() {
        return div()
            .w_full()
            .min_w_0()
            .text_base()
            .line_height(rems(1.375))
            .text_color(body_color)
            .child(styled)
            .into_any_element();
    }

    let profile_context = ctx.profile_context;
    let settings = ctx.settings.clone();
    let host = ctx.video_host.clone();
    let avatar_cache = ctx.large_avatar_cache.clone();
    let actions = plan.actions.clone();
    let locale = plan.locale.clone();
    let interactive = InteractiveText::new(("msg-itext", msg.row_anchor_id.0 as usize), styled)
        .on_click_shared(plan.click_ranges.clone(), move |range_ix, window, cx| {
            let Some(action) = actions.get(range_ix) else {
                return;
            };
            match action {
                RichClick::Link(url) => open_message_link(url.to_string(), cx),
                RichClick::Channel(channel_id) => {
                    navigate_to_channel(*channel_id, locale.as_ref(), cx)
                }
                RichClick::Mention(user_id) => {
                    let Some(context) = profile_context else {
                        return;
                    };
                    let position = window.mouse_position();
                    let popover = cx.new(|cx| {
                        UserProfilePopover::new(
                            *user_id,
                            context,
                            settings.clone(),
                            avatar_cache.clone(),
                            window,
                            cx,
                        )
                    });
                    let _ = host.update(cx, move |this, cx| {
                        this.set_mention_popover(popover, position, window, cx);
                    });
                }
            }
        });

    div()
        .w_full()
        .min_w_0()
        .text_base()
        .line_height(rems(1.375))
        .text_color(body_color)
        .child(interactive)
        .into_any_element()
}

fn render_mention_only_content(
    msg: &Message,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    inline: bool,
) -> AnyElement {
    let has_mention = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::Mention { .. }));
    if !has_mention && !msg.is_edited {
        return div().into_any_element();
    }
    let mut row = rich_content_row(body_color, inline);
    for span in msg
        .spans
        .iter()
        .filter(|s| matches!(s, MessageSpan::Mention { .. }))
    {
        row = append_span(row, span, ctx, body_color, px(EMOJI_SIZE));
    }
    if msg.is_edited {
        row = row.child(edited_marker(ctx.theme, ctx.locale));
    }
    row.into_any_element()
}

fn rich_content_row(body_color: gpui::Rgba, inline: bool) -> gpui::Div {
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .min_w_0()
        .text_base()
        .line_height(rems(1.375))
        .text_color(body_color);
    if inline {
        row = row.flex_shrink_0();
    } else {
        row = row.w_full().gap_x(px(4.));
    }
    row
}

fn edited_marker(theme: &Theme, locale: &str) -> AnyElement {
    div()
        .text_color(theme.text_muted)
        .text_size(px(9.))
        .child(mezon_i18n::t(locale, "message.edited"))
        .into_any_element()
}

pub fn append_system_mention_spans(mut row: gpui::Div, msg: &Message, ctx: &RowCtx) -> gpui::Div {
    let body_color = ctx.theme.tokens.text_theme_primary;
    for span in msg
        .spans
        .iter()
        .filter(|s| matches!(s, MessageSpan::Mention { .. }))
    {
        row = append_span(row, span, ctx, body_color, px(EMOJI_SIZE));
    }
    row
}

fn append_span(
    mut row: gpui::Div,
    span: &MessageSpan,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    emoji_size: Pixels,
) -> gpui::Div {
    let theme = ctx.theme;
    match span {
        MessageSpan::Text(text) => {
            for child in text_to_words(text) {
                row = row.child(child);
            }
            row
        }
        MessageSpan::Bold(text) => row.child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(body_color)
                .child(text.clone()),
        ),
        MessageSpan::Code(text) => row.child(
            div()
                .px_2()
                .rounded_md()
                .bg(theme.tokens.bg_active_member_channel)
                .text_size(px(14.))
                .text_color(theme.tokens.text_secondary)
                .child(text.clone()),
        ),
        MessageSpan::CodeBlock { text, .. } => row.child(
            div()
                .flex_basis(relative(1.))
                .w_full()
                .min_w_0()
                .my_1()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.bg_markdown_code)
                .text_size(px(14.))
                .text_color(theme.tokens.text_theme_message)
                .child(text.clone()),
        ),
        MessageSpan::Link { text, url, kind } if *kind != LinkKind::Plain => {
            row.child(render_social_link_card(text, url, *kind, theme))
        }
        MessageSpan::Link { text, url, .. } => {
            let resolved = SharedString::from(resolve_link_url(url, text));
            let segments = link_to_wrap_segments(text, resolved, theme.tokens.mention_color);
            row.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_baseline()
                    .min_w_0()
                    .children(segments),
            )
        }
        MessageSpan::Mention {
            display,
            user_id,
            role_id,
        } => row.child(render_mention_chip(
            display.clone(),
            user_id.as_deref(),
            role_id.as_deref(),
            ctx,
        )),
        MessageSpan::Hashtag {
            display,
            channel_id,
        } => row.child(render_hashtag_chip(
            display.clone(),
            channel_id.as_deref(),
            ctx,
        )),
        MessageSpan::Emoji {
            name,
            emoji_id,
            src,
        } => row.child(render_emoji_span(
            name, emoji_id, src, body_color, ctx, emoji_size,
        )),
        MessageSpan::Canvas { title, .. } => row.child(render_canvas_chip(title.clone())),
        MessageSpan::Heading { level, text } => row.child(render_heading(*level, text.clone())),
    }
}

fn heading_size(level: u8) -> Pixels {
    match level {
        1 => px(36.),
        2 => px(30.),
        3 => px(24.),
        4 => px(20.),
        5 => px(18.),
        _ => px(16.),
    }
}

fn render_heading(level: u8, text: SharedString) -> AnyElement {
    div()
        .w_full()
        .my(px(4.))
        .text_size(heading_size(level))
        .font_weight(FontWeight::BOLD)
        .child(text)
        .into_any_element()
}

/// A stable element id derived from `name` + a hash of `key`, without the
/// per-frame `String` allocation of `format!("{name}-{key}")` ids. Ids only
/// need to be unique among siblings of the same parent element.
pub(crate) fn hashed_element_id(name: &'static str, key: &str) -> gpui::ElementId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    gpui::ElementId::NamedInteger(SharedString::new_static(name), hash)
}

fn render_canvas_chip(title: SharedString) -> AnyElement {
    div()
        .id(hashed_element_id("canvas-chip", &title))
        .flex()
        .flex_none()
        .flex_row()
        .items_center()
        .gap_1()
        .px(px(2.))
        .rounded_sm()
        .font_weight(FontWeight::MEDIUM)
        .bg(rgb(CANVAS_BG))
        .text_color(rgb(CANVAS_TEXT))
        .hover(|s| {
            s.bg(rgb(CANVAS_HOVER_BG))
                .text_color(rgb(CANVAS_HOVER_TEXT))
        })
        .child(
            Icon::new(IconName::CanvasIcon)
                .size_4()
                .text_color(rgb(CANVAS_TEXT)),
        )
        .child(title)
        .into_any_element()
}

fn render_social_link_card(
    text: &SharedString,
    url: &str,
    kind: LinkKind,
    theme: &Theme,
) -> AnyElement {
    let (accent, label) = match kind {
        LinkKind::YouTube => (YOUTUBE_ACCENT, "YouTube"),
        LinkKind::Facebook => (FACEBOOK_ACCENT, "Facebook"),
        LinkKind::TikTok => (TIKTOK_ACCENT, "TikTok"),
        LinkKind::Plain => (SOCIAL_CARD_BG, ""),
    };
    let resolved = resolve_link_url(url, text);
    let display = text.clone();
    let id = hashed_element_id("msg-social", &resolved);
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .max_w(px(400.))
        .my_1()
        .p(px(16.))
        .rounded(px(4.))
        .border_l_4()
        .border_color(rgb(accent))
        .bg(rgb(SOCIAL_CARD_BG))
        .cursor_pointer()
        .on_click(move |_, _, cx| open_message_link(resolved.clone(), cx))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(accent))
                .child(label),
        )
        .child(
            div()
                .text_size(px(14.))
                .text_color(theme.tokens.mention_color)
                .child(display),
        )
        .into_any_element()
}

fn render_emoji_span(
    name: &SharedString,
    emoji_id: &str,
    precomputed_src: &SharedString,
    body_color: gpui::Rgba,
    ctx: &RowCtx,
    size: Pixels,
) -> AnyElement {
    let src: SharedString = if precomputed_src.is_empty() {
        crate::util::imgproxy::emoji_url(ctx.app, emoji_id).into()
    } else {
        precomputed_src.clone()
    };
    if src.is_empty() {
        return div()
            .text_color(body_color)
            .child(name.clone())
            .into_any_element();
    }
    img(src)
        .size(size)
        .flex_none()
        .object_fit(ObjectFit::Contain)
        .with_fallback(super::reaction_detail::emoji_error_fallback(
            size,
            ctx.theme.text_muted,
        ))
        .into_any_element()
}

fn render_mention_chip(
    display: impl Into<SharedString>,
    user_id: Option<&str>,
    role_id: Option<&str>,
    ctx: &RowCtx,
) -> AnyElement {
    let display = display.into();
    let theme = ctx.theme;
    let is_role = role_id.is_some_and(|r| !r.is_empty());
    let (bg, color, hover_bg, hover_color) = if is_role {
        (
            theme.tokens.bg_mention_evryone,
            theme.tokens.color_mention_evryone,
            theme.tokens.bg_mention_everyone_hover,
            theme.tokens.color_mention_everyone_hover,
        )
    } else {
        (
            theme.tokens.mention_primary,
            theme.tokens.mention_color,
            theme.tokens.bg_mention_hover,
            theme.tokens.color_mention_hover,
        )
    };

    let chip = div()
        .flex_none()
        .px(px(1.))
        .rounded_sm()
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(color)
        .hover(move |s| s.bg(hover_bg).text_color(hover_color))
        .child(display.clone());

    if is_role {
        return chip.into_any_element();
    }

    let Some(uid) = user_id.filter(|uid| !uid.is_empty() && *uid != "0" && !is_here_user_id(uid))
    else {
        return chip.into_any_element();
    };

    let Some(user_id) = uid.parse::<i64>().ok().map(UserId) else {
        return chip.into_any_element();
    };

    let Some(profile_ctx) = ctx.profile_context else {
        return chip.cursor_pointer().into_any_element();
    };

    let mention_key = user_id.get() as usize;
    profile_popover_trigger(
        ("msg-mention", mention_key),
        user_id,
        profile_ctx,
        ctx,
        chip.into_any_element(),
    )
    .flex_none()
    .into_any_element()
}

pub(crate) fn profile_popover_trigger(
    id: impl Into<gpui::ElementId>,
    user_id: UserId,
    profile_ctx: ProfileContext,
    ctx: &RowCtx,
    child: AnyElement,
) -> ClickableContainer {
    let settings = ctx.settings.clone();
    let avatar_cache = ctx.large_avatar_cache.clone();
    let host = ctx.video_host.clone();
    ClickableContainer::new(id)
        .cursor_pointer()
        .child(child)
        .on_click(move |_, window, cx| {
            let position = window.mouse_position();
            let popover = cx.new(|cx| {
                UserProfilePopover::new(
                    user_id,
                    profile_ctx,
                    settings.clone(),
                    avatar_cache.clone(),
                    window,
                    cx,
                )
            });
            let _ = host.update(cx, move |this, cx| {
                this.set_mention_popover(popover, position, window, cx);
            });
        })
}

fn render_hashtag_chip(
    display: impl Into<SharedString>,
    channel_id: Option<&str>,
    ctx: &RowCtx,
) -> AnyElement {
    let display: SharedString = display.into();
    let theme = ctx.theme;
    let bg = theme.tokens.mention_primary;
    let color = theme.tokens.mention_color;
    let hover_bg = theme.tokens.bg_mention_hover;
    let hover_color = theme.tokens.color_mention_hover;
    let parsed_channel = channel_id.and_then(parse_channel_id);
    let (resolved_name, icon) = match parsed_channel {
        Some(cid) => hashtag_channel(cid, ctx.app),
        None => (None, IconName::Hashtag),
    };
    let from_channel_link = display.starts_with("http://") || display.starts_with("https://");
    let (label, unresolved_link) = match resolved_name {
        Some(name) => (name, false),
        None if from_channel_link => (
            SharedString::from(mezon_i18n::t(ctx.locale, "message.unknown")),
            true,
        ),
        None => (
            display
                .strip_prefix('#')
                .map(|name| SharedString::from(name.to_owned()))
                .unwrap_or(display),
            false,
        ),
    };

    let inner = div()
        .flex()
        .flex_none()
        .flex_row()
        .items_center()
        .gap_0p5()
        .child(Icon::new(icon).size_4().text_color(color))
        .child(div().when(unresolved_link, |d| d.italic()).child(label));

    match parsed_channel {
        Some(channel_id) => {
            let locale = ctx.locale.to_string();
            div()
                .id(("msg-hashtag", channel_id.get() as usize))
                .flex_none()
                .px(px(1.))
                .rounded_sm()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .bg(bg)
                .text_color(color)
                .on_click(move |_, _, cx| navigate_to_channel(channel_id, &locale, cx))
                .hover(move |s| s.bg(hover_bg).text_color(hover_color))
                .child(inner)
                .into_any_element()
        }
        None => div()
            .flex_none()
            .px(px(1.))
            .rounded_sm()
            .font_weight(FontWeight::MEDIUM)
            .bg(bg)
            .text_color(color)
            .hover(move |s| s.bg(hover_bg).text_color(hover_color))
            .child(inner)
            .into_any_element(),
    }
}

fn build_inline_content(msg: &Message, ctx: &RowCtx, body_color: gpui::Rgba) -> Option<AnyElement> {
    let all_supported = msg.spans.iter().all(|span| {
        matches!(
            span,
            MessageSpan::Text(_) | MessageSpan::Mention { .. } | MessageSpan::Hashtag { .. }
        )
    });
    if !all_supported {
        return None;
    }

    let theme = ctx.theme;
    let mention_color: Hsla = theme.tokens.mention_color.into();
    let mention_bg: Hsla = theme.tokens.mention_primary.into();
    let role_color: Hsla = theme.tokens.color_mention_evryone.into();
    let role_bg: Hsla = theme.tokens.bg_mention_evryone.into();
    let body: Hsla = body_color.into();

    let mut text = String::new();
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut icons: Vec<IconOverlay> = Vec::new();
    let mut clicks: Vec<ClickRegion> = Vec::new();

    for span in &msg.spans {
        match span {
            MessageSpan::Text(chunk) => text.push_str(chunk),
            MessageSpan::Mention {
                display,
                user_id,
                role_id,
            } => {
                let start = text.len();
                text.push_str(display);
                let end = text.len();
                let is_role = role_id.as_deref().is_some_and(|r| !r.is_empty());
                runs.push(StyledRun {
                    range: start..end,
                    color: Some(if is_role { role_color } else { mention_color }),
                    background: Some(if is_role { role_bg } else { mention_bg }),
                });
                if is_role {
                    continue;
                }
                let Some(uid) = user_id
                    .as_deref()
                    .filter(|uid| !uid.is_empty() && *uid != "0" && !is_here_user_id(uid))
                    .and_then(|uid| uid.parse::<i64>().ok())
                    .map(UserId)
                else {
                    continue;
                };
                let Some(profile_ctx) = ctx.profile_context else {
                    continue;
                };
                let settings = ctx.settings.clone();
                let avatar_cache = ctx.large_avatar_cache.clone();
                let host = ctx.video_host.clone();
                clicks.push(ClickRegion {
                    range: start..end,
                    action: Box::new(move |window, cx| {
                        let position = window.mouse_position();
                        let popover = cx.new(|cx| {
                            UserProfilePopover::new(
                                uid,
                                profile_ctx,
                                settings.clone(),
                                avatar_cache.clone(),
                                window,
                                cx,
                            )
                        });
                        let _ = host.update(cx, move |this, cx| {
                            this.set_mention_popover(popover, position, window, cx);
                        });
                    }),
                });
            }
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => {
                let parsed_channel = channel_id.as_deref().and_then(parse_channel_id);
                let (resolved_name, icon) = match parsed_channel {
                    Some(cid) => hashtag_channel(cid, ctx.app),
                    None => (None, IconName::Hashtag),
                };
                let label = hashtag_label(display, resolved_name, ctx.locale);
                let icon_index = text.len();
                text.push(INLINE_ICON_PLACEHOLDER);
                let label_index = text.len();
                text.push_str(&label);
                let end = text.len();
                runs.push(StyledRun {
                    range: icon_index..label_index,
                    color: Some(Hsla {
                        h: 0.,
                        s: 0.,
                        l: 0.,
                        a: 0.,
                    }),
                    background: Some(mention_bg),
                });
                runs.push(StyledRun {
                    range: label_index..end,
                    color: Some(mention_color),
                    background: Some(mention_bg),
                });
                icons.push(IconOverlay {
                    byte_index: icon_index,
                    end_index: label_index,
                    icon,
                    color: mention_color,
                });
                if let Some(cid) = parsed_channel {
                    let locale = ctx.locale.to_string();
                    clicks.push(ClickRegion {
                        range: icon_index..end,
                        action: Box::new(move |_, cx| navigate_to_channel(cid, &locale, cx)),
                    });
                }
            }
            _ => return None,
        }
    }

    if icons.is_empty() {
        return None;
    }

    Some(
        InlineContent::new(
            ("msg-inline", msg.row_anchor_id.0 as usize),
            text.into(),
            runs,
            icons,
            clicks,
            body,
        )
        .into_any_element(),
    )
}

fn hashtag_label(display: &str, resolved_name: Option<SharedString>, locale: &str) -> SharedString {
    if let Some(name) = resolved_name {
        return name;
    }
    if display.starts_with("http://") || display.starts_with("https://") {
        return SharedString::from(mezon_i18n::t(locale, "message.unknown"));
    }
    SharedString::from(display.strip_prefix('#').unwrap_or(display).to_owned())
}

fn hashtag_channel(channel_id: ChannelId, cx: &App) -> (Option<SharedString>, IconName) {
    let channels = ChannelList::global(cx);
    let store = channels.read(cx);
    let resolved = store
        .find_channel_in_active_clan(channel_id)
        .or_else(|| {
            store
                .clan_id_for_channel(channel_id)
                .and_then(|clan_id| store.channel(clan_id, channel_id))
        })
        .or_else(|| store.user_channel(channel_id));
    match resolved {
        Some(channel) => (
            (!channel.name.is_empty()).then(|| SharedString::from(channel.name.clone())),
            channel_type_icon(channel.channel_type, channel.private),
        ),
        None => (None, IconName::Hashtag),
    }
}

fn channel_type_icon(kind: ChannelType, private: bool) -> IconName {
    match kind {
        ChannelType::Voice => {
            if private {
                IconName::SpeakerLocked
            } else {
                IconName::Speaker
            }
        }
        ChannelType::Stream => IconName::Stream,
        ChannelType::App => {
            if private {
                IconName::PrivateAppChannelIcon
            } else {
                IconName::AppChannelIcon
            }
        }
        ChannelType::Thread => {
            if private {
                IconName::ThreadIconLocker
            } else {
                IconName::ThreadIcon
            }
        }
        _ => {
            if private {
                IconName::HashtagLocked
            } else {
                IconName::Hashtag
            }
        }
    }
}

fn parse_channel_id(raw: &str) -> Option<ChannelId> {
    raw.parse::<i64>()
        .ok()
        .map(ChannelId)
        .filter(|id| !id.is_zero())
}

fn navigate_to_channel(channel_id: ChannelId, locale: &str, cx: &mut App) {
    let Some(clan_id) = clan_for_channel(channel_id, cx) else {
        Shell::global(cx).update(cx, |shell, cx| {
            shell.info(mezon_i18n::t(locale, "message.noAccess"), cx);
        });
        return;
    };
    navigate(
        cx,
        Route::Channel {
            clan_id,
            channel_id,
        },
    );
}

fn clan_for_channel(channel_id: ChannelId, cx: &App) -> Option<ClanId> {
    let list = ChannelList::global(cx);
    let store = list.read(cx);
    store.clan_id_for_channel(channel_id).or_else(|| {
        store
            .user_channel(channel_id)
            .map(|channel| channel.clan_id)
    })
}

fn is_link_only(spans: &[MessageSpan]) -> bool {
    spans.iter().any(|s| matches!(s, MessageSpan::Link { .. }))
        && spans.iter().all(|s| match s {
            MessageSpan::Link { .. } => true,
            MessageSpan::Text(text) => text.trim().is_empty(),
            _ => false,
        })
}

fn render_plain_text_spans(spans: &[MessageSpan], color: gpui::Rgba) -> AnyElement {
    let text: SharedString = match spans {
        [MessageSpan::Text(t)] => t.clone(),
        _ => spans
            .iter()
            .filter_map(|s| match s {
                MessageSpan::Text(t) => Some(t.as_ref()),
                _ => None,
            })
            .collect::<String>()
            .into(),
    };
    if text.is_empty() {
        return div().into_any_element();
    }
    if !text.contains('\n') {
        return div()
            .w_full()
            .min_w_0()
            .min_h(px(30.))
            .text_base()
            .line_height(rems(1.375))
            .text_color(color)
            .child(text)
            .into_any_element();
    }
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .text_base()
        .line_height(rems(1.375));
    for line in text.split('\n') {
        col = col.child(
            div()
                .w_full()
                .min_w_0()
                .min_h(px(20.))
                .text_color(color)
                .child(line.to_string()),
        );
    }
    col.into_any_element()
}

fn render_link_only_spans(spans: &[MessageSpan], theme: &Theme) -> AnyElement {
    let link_color = theme.tokens.mention_color;
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .text_base()
        .gap_1();
    for span in spans {
        let MessageSpan::Link { text, url, .. } = span else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let resolved = resolve_link_url(url, text);
        if !text.contains('\n') {
            col = col.child(link_block(resolved, text.clone(), link_color));
            continue;
        }
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            col = col.child(link_block(resolved.clone(), line.to_string(), link_color));
        }
    }
    col.into_any_element()
}

fn resolve_link_url(url: &str, text: &str) -> String {
    if !url.is_empty() {
        return url.to_string();
    }
    text.to_string()
}

/// The first detected link in a message's rich-text spans, if any (for the
/// "···" context menu's copy/open-link items).
pub(crate) fn first_link(msg: &Message) -> Option<String> {
    msg.spans.iter().find_map(|span| match span {
        MessageSpan::Link { text, url, .. } => Some(resolve_link_url(url, text)),
        _ => None,
    })
}

pub(crate) fn open_message_link(url: String, cx: &mut App) {
    if url.is_empty() {
        return;
    }
    if let Some(store) = PlatformStore::try_global(cx) {
        let _ = store.read(cx).open_url_external(&url);
    }
}

fn link_block(url: String, display: impl Into<SharedString>, color: gpui::Rgba) -> AnyElement {
    let id = hashed_element_id("msg-link", &url);
    let display = display.into();
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| open_message_link(url.clone(), cx))
        .child(display)
        .into_any_element()
}

fn link_segment(url: SharedString, display: String, color: gpui::Rgba, index: usize) -> AnyElement {
    div()
        .id((url.clone(), index))
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| open_message_link(url.to_string(), cx))
        .child(display)
        .into_any_element()
}

fn text_to_words(text: &str) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        for word in line.split_whitespace() {
            out.push(word.to_string().into_any_element());
        }
    }
    out
}

fn link_to_wrap_segments(text: &str, url: SharedString, color: gpui::Rgba) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut index = 0usize;
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        if line.chars().any(char::is_whitespace) {
            for word in line.split_whitespace() {
                for segment in split_unbreakable(word) {
                    out.push(link_segment(url.clone(), segment, color, index));
                    index += 1;
                }
            }
        } else {
            for segment in split_unbreakable(line) {
                out.push(link_segment(url.clone(), segment, color, index));
                index += 1;
            }
        }
    }
    out
}

fn split_unbreakable(text: &str) -> Vec<String> {
    const MAX_SEGMENT_LEN: usize = 32;
    let mut parts = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if matches!(
            ch,
            '/' | '-' | '_' | '.' | '?' | '&' | '#' | '=' | '@' | ':'
        ) || buf.chars().count() >= MAX_SEGMENT_LEN
        {
            parts.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::{RichTextRenderPlan, parse_channel_id, rich_text_plan_matches};
    use gpui::{Hsla, SharedString};
    use mezon_store::{ChannelId, MessageSpan, build_rich_layout};

    #[test]
    fn parse_channel_id_rejects_zero() {
        assert_eq!(parse_channel_id("0"), None);
        assert_eq!(parse_channel_id("12345"), Some(ChannelId(12345)));
    }

    #[test]
    fn rich_text_plan_reuses_only_the_same_layout_and_style() {
        let layout = build_rich_layout(&[MessageSpan::Bold("hello".into())]).unwrap();
        let colors = [Hsla::default(); 5];
        let plan = RichTextRenderPlan {
            layout: layout.clone(),
            colors,
            edited: false,
            text: SharedString::from("hello"),
            highlights: Vec::new().into(),
            font_overrides: Vec::new().into(),
            click_ranges: Vec::new().into(),
            actions: Vec::new().into(),
            locale: SharedString::from("en"),
        };

        assert!(rich_text_plan_matches(&plan, &layout, colors, false));
        assert!(!rich_text_plan_matches(&plan, &layout, colors, true));

        let replacement = std::sync::Arc::new((*layout).clone());
        assert!(!rich_text_plan_matches(&plan, &replacement, colors, false));
    }
}

#[cfg(test)]
mod hashtag_label_tests {
    use super::hashtag_label;
    use gpui::SharedString;

    #[test]
    fn resolved_channel_wins_over_the_raw_url() {
        let label = hashtag_label(
            "https://mezon.ai/chat/clans/1/channels/2",
            Some(SharedString::from("general")),
            "en",
        );
        assert_eq!(label, "general");
    }

    #[test]
    fn unresolved_channel_link_never_shows_the_raw_url() {
        let label = hashtag_label("https://mezon.ai/chat/clans/1/channels/2", None, "en");
        assert_eq!(label, "unknown");
    }

    #[test]
    fn typed_hashtag_falls_back_to_the_name_without_the_hash() {
        assert_eq!(hashtag_label("#general", None, "en"), "general");
    }
}
