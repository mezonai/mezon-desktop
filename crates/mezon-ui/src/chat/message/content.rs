use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, FontWeight, HighlightStyle, Hsla, InteractiveText, ObjectFit, Pixels,
    SharedString, StyledText, TextLayout, UnderlineStyle, canvas, div, fill, img, point,
    prelude::*, px, relative, rems, rgb, rgba, size,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, Embed, LinkKind, Message, MessageCode, MessageId,
    MessageSpan, PlatformStore, ProfileContext, RichClick, RichLayout, RichRunKind, RichToken,
    UserId, is_here_user_id,
};

use ui::Clickable;

use super::context::{RichTextRenderPlan, RowCtx};
use super::inline_content::{ClickRegion, IconOverlay, InlineContent, StyledRun};
use super::selection::{
    SelectableRegion, SharedSelection, TextSegment, merge_selection_background,
};
use crate::app::shell::Shell;
use crate::chat::user_profile_popover::{ClickableContainer, UserProfilePopover};
use crate::components::primitives::{CopyButton, Icon, IconName};
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

/// The image proxy is asked for the emoji at the size it will actually be
/// painted on a 2x display. An atlas tile smaller than its box is magnified
/// with a linear filter across a gutterless atlas, which samples the
/// neighbouring tile -- the next animation frame -- into the emoji's edges.
fn emoji_source_px(size: Pixels) -> u32 {
    (f32::from(size) * 2.0).round().max(1.0) as u32
}
pub(crate) const INLINE_ICON_PLACEHOLDER: char = '\u{2800}';
pub(crate) const ATTACHMENT_PLACEHOLDER: char = '\u{fffc}';
const RICH_TEXT_PLAN_LIMIT: usize = 512;
const SELECTABLE_LAYOUT_PLAN_LIMIT: usize = 128;
const SELECTABLE_TEXT_PIECE_LIMIT: usize = 1024;
pub(crate) const SELECTION_BG: u32 = 0x58_65_f2_4d;

struct ContentRenderOptions {
    body_color: gpui::Rgba,
    mentions_only: bool,
    inline: bool,
}

pub fn render_message_content(
    msg: &Message,
    ctx: &RowCtx,
    invite_base: Option<usize>,
    selection_context: Option<&SelectableTextContext>,
) -> AnyElement {
    let body_color = if msg.code == MessageCode::MessageBuzz {
        rgb(BUZZ_RED)
    } else {
        ctx.theme.tokens.text_theme_message
    };
    let content = render_message_content_with_options(
        msg,
        ctx,
        selection_context,
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
            .child(super::invite_card::render_invite_card(
                invite,
                invite_base,
                selection_context,
                ctx,
            ))
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
        None,
        ContentRenderOptions {
            body_color: ctx.theme.tokens.text_theme_primary,
            mentions_only,
            inline: mentions_only,
        },
    )
}

fn render_message_content_with_options(
    msg: &Message,
    ctx: &RowCtx,
    selection_context: Option<&SelectableTextContext>,
    options: ContentRenderOptions,
) -> AnyElement {
    let theme = ctx.theme;
    let body_color = options.body_color;

    if msg.is_deleted_placeholder && !options.mentions_only {
        return render_deleted_placeholder(msg, ctx, theme);
    }

    if msg.spans.is_empty() && !msg.is_edited {
        return div().into_any_element();
    }

    if options.mentions_only {
        return render_mention_only_content(msg, ctx, body_color, options.inline);
    }

    let has_tokens = msg.spans.iter().any(|s| !matches!(s, MessageSpan::Text(_)));

    if !has_tokens && !msg.is_edited {
        return render_plain_text_spans(msg, ctx, body_color);
    }

    let needs_chip_path = spans_need_chip_path(&msg.spans);

    let has_code_block = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::CodeBlock { .. }));
    let has_custom_emoji = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::Emoji { emoji_id, .. } if !emoji_id.is_empty()));
    if !options.inline
        && !has_code_block
        && !has_custom_emoji
        && !needs_chip_path
        && msg.rich_layout.is_some()
    {
        return render_rich_styled(msg, ctx, body_color);
    }

    let emoji_size = if msg.is_only_emoji {
        px(EMOJI_JUMBO_SIZE)
    } else {
        px(EMOJI_SIZE)
    };
    if !options.inline
        && let Some(inline) = build_inline_content(msg, ctx, body_color)
    {
        let mut row = rich_content_row(body_color, false).child(inline);
        if msg.is_edited {
            row = row.child(edited_marker(theme, ctx.locale));
        }
        return row.into_any_element();
    }
    if !options.inline {
        return render_selectable_segmented_content(
            msg,
            ctx,
            body_color,
            emoji_size,
            selection_context,
        );
    }
    let mut row = rich_content_row(body_color, options.inline);
    let mut span_key = 0usize;
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
                        &mut span_key,
                    ),
                };
            }
        }
        None => {
            for span in &msg.spans {
                row = append_span(row, span, ctx, body_color, emoji_size, &mut span_key);
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

fn render_deleted_placeholder(msg: &Message, ctx: &RowCtx, theme: &Theme) -> AnyElement {
    let text: SharedString = msg.content.clone().into();
    let selection_range = ctx.selection.borrow().range_for_message(msg.id, &text);
    let styled = if let Some(range) = selection_range {
        StyledText::new(text).with_highlights(merge_selection_background(
            &[],
            range,
            rgba(SELECTION_BG).into(),
        ))
    } else {
        StyledText::new(text)
    };
    ctx.selection
        .borrow_mut()
        .registry
        .insert(msg.id, styled.layout().clone());
    div()
        .w_full()
        .min_w_0()
        .min_h(px(30.))
        .italic()
        .cursor(gpui::CursorStyle::IBeam)
        .text_base()
        .line_height(rems(1.375))
        .text_color(theme.tokens.text_theme_primary)
        .child(styled)
        .into_any_element()
}

fn rich_text_plan_matches(
    plan: &RichTextRenderPlan,
    layout: &std::sync::Arc<mezon_store::RichLayout>,
    colors: [Hsla; 7],
    edited: bool,
) -> bool {
    std::sync::Arc::ptr_eq(&plan.layout, layout) && plan.colors == colors && plan.edited == edited
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RichRunPalette {
    pub mention: Hsla,
    pub mention_bg: Hsla,
    pub role: Hsla,
    pub role_bg: Hsla,
    pub code_bg: Hsla,
    pub link: Hsla,
}

impl RichRunPalette {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
            mention: theme.tokens.mention_color.into(),
            mention_bg: theme.tokens.mention_primary.into(),
            role: theme.tokens.color_mention_evryone.into(),
            role_bg: theme.tokens.bg_mention_evryone.into(),
            code_bg: theme.tokens.bg_markdown_code.into(),
            link: theme.tokens.mention_color.into(),
        }
    }

    pub(crate) fn memo_key(&self, text_muted: Hsla) -> [Hsla; 7] {
        [
            self.mention,
            self.mention_bg,
            self.role,
            self.role_bg,
            self.code_bg,
            self.link,
            text_muted,
        ]
    }
}

pub(crate) fn rich_run_highlight(kind: RichRunKind, palette: &RichRunPalette) -> HighlightStyle {
    match kind {
        RichRunKind::Bold => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        },
        RichRunKind::Code => HighlightStyle {
            background_color: Some(palette.code_bg),
            ..Default::default()
        },
        RichRunKind::Link => HighlightStyle {
            color: Some(palette.link),
            ..Default::default()
        },
        RichRunKind::Mention | RichRunKind::Hashtag => HighlightStyle {
            color: Some(palette.mention),
            background_color: Some(palette.mention_bg),
            ..Default::default()
        },
        RichRunKind::RoleMention => HighlightStyle {
            color: Some(palette.role),
            background_color: Some(palette.role_bg),
            ..Default::default()
        },
    }
}

pub(crate) fn rich_run_highlight_with_link_underline(
    kind: RichRunKind,
    palette: &RichRunPalette,
) -> HighlightStyle {
    let mut style = rich_run_highlight(kind, palette);
    if kind == RichRunKind::Link {
        style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(palette.link),
            wavy: false,
        });
    }
    style
}

fn rich_highlights_with_link_hover(
    highlights: &[(Range<usize>, HighlightStyle)],
    hovered_link: Option<&Range<usize>>,
    link_color: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(hovered_link) = hovered_link else {
        return highlights.to_vec();
    };
    highlights
        .iter()
        .map(|(range, style)| {
            if range.start >= hovered_link.end || range.end <= hovered_link.start {
                return (range.clone(), *style);
            }
            let mut merged = *style;
            merged.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(style.color.unwrap_or(link_color)),
                wavy: false,
            });
            (range.clone(), merged)
        })
        .collect()
}

fn render_rich_styled(msg: &Message, ctx: &RowCtx, body_color: gpui::Rgba) -> AnyElement {
    let theme = ctx.theme;
    let palette = RichRunPalette::from_theme(theme);
    let colors = palette.memo_key(theme.text_muted.into());
    let Some(layout) = msg.rich_layout.as_ref() else {
        return div().into_any_element();
    };
    let cached = ctx.row_memo.borrow().rich_text.get(&msg.id).cloned();
    let plan = match cached {
        Some(plan) if rich_text_plan_matches(&plan, layout, colors, msg.is_edited) => plan,
        _ => {
            let mut highlights: Vec<(Range<usize>, HighlightStyle)> =
                Vec::with_capacity(layout.runs.len());
            let mut font_overrides: Vec<(Range<usize>, SharedString)> = Vec::new();
            let mut click_ranges: Vec<Range<usize>> = Vec::new();
            let mut actions: Vec<RichClick> = Vec::new();
            for run in layout.runs.iter() {
                highlights.push((run.range.clone(), rich_run_highlight(run.kind, &palette)));
                if run.kind == RichRunKind::Code {
                    font_overrides.push((run.range.clone(), "monospace".into()));
                }
                if let Some(click) = run.click.clone() {
                    click_ranges.push(run.range.clone());
                    actions.push(click);
                }
            }

            let plan = RichTextRenderPlan {
                layout: layout.clone(),
                colors,
                edited: msg.is_edited,
                text: layout.text.clone(),
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

    let selection_range = ctx.selection.borrow().range_for_message(msg.id, &plan.text);
    let hovered_link = ctx
        .row_memo
        .borrow()
        .hovered_rich_link
        .as_ref()
        .filter(|(id, _)| *id == msg.id)
        .map(|(_, range)| range.clone());
    let highlights = hovered_link.as_ref().map(|hovered| {
        rich_highlights_with_link_hover(&plan.highlights, Some(hovered), palette.link)
    });
    let mut styled = if let Some(range) = selection_range {
        let base = highlights
            .as_ref()
            .map_or(plan.highlights.as_ref(), |h| h.as_slice());
        let merged = merge_selection_background(base, range, rgba(SELECTION_BG).into());
        StyledText::new(plan.text.clone()).with_highlights(merged)
    } else if let Some(highlights) = highlights {
        StyledText::new(plan.text.clone()).with_highlights(highlights)
    } else {
        StyledText::new(plan.text.clone()).with_shared_highlights(plan.highlights.clone())
    };
    if !plan.font_overrides.is_empty() {
        styled = styled.with_shared_font_family_overrides(plan.font_overrides.clone());
    }

    ctx.selection
        .borrow_mut()
        .registry
        .insert(msg.id, styled.layout().clone());

    let content = if plan.actions.is_empty() {
        styled.into_any_element()
    } else {
        let profile_context = ctx.profile_context;
        let settings = ctx.settings.clone();
        let host = ctx.video_host.clone();
        let hover_host = ctx.video_host.clone();
        let avatar_cache = ctx.large_avatar_cache.clone();
        let actions = plan.actions.clone();
        let hover_actions = plan.actions.clone();
        let locale = plan.locale.clone();
        let text_selection = ctx.selection.clone();
        let click_ranges = plan.click_ranges.clone();
        let row_memo = ctx.row_memo.clone();
        let message_id = msg.id;
        let clear_row_memo = row_memo.clone();
        let clear_hover_host = hover_host.clone();
        let clear_message_id = message_id;
        let hover_epoch = row_memo.borrow().rich_link_hover_epoch;
        let itext_id = (msg.row_anchor_id.0 as u64) << 32 | u64::from(hover_epoch);
        div()
            .id(("msg-link-hover", msg.row_anchor_id.0 as usize))
            .max_w_full()
            .min_w_0()
            .on_hover(move |hovered, _, cx| {
                if *hovered {
                    return;
                }
                let mut memo = clear_row_memo.borrow_mut();
                if memo
                    .hovered_rich_link
                    .as_ref()
                    .is_some_and(|(id, _)| *id == clear_message_id)
                {
                    memo.hovered_rich_link = None;
                    memo.rich_link_hover_epoch = memo.rich_link_hover_epoch.wrapping_add(1);
                    drop(memo);
                    let _ = clear_hover_host.update(cx, |_, cx| cx.notify());
                }
            })
            .child(
                InteractiveText::new(("msg-itext", itext_id), styled)
                    .on_click_shared(click_ranges.clone(), move |range_ix, window, cx| {
                        if text_selection.borrow().has_selection() {
                            return;
                        }
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
                    })
                    .on_hover(move |index, _, _, cx| {
                        let next = index.and_then(|offset| {
                            click_ranges.iter().zip(hover_actions.iter()).find_map(
                                |(range, action)| {
                                    if !range.contains(&offset) {
                                        return None;
                                    }
                                    match action {
                                        RichClick::Link(_) | RichClick::Channel(_) => {
                                            Some((message_id, range.clone()))
                                        }
                                        RichClick::Mention(_) => None,
                                    }
                                },
                            )
                        });
                        let mut memo = row_memo.borrow_mut();
                        if memo.hovered_rich_link == next {
                            return;
                        }
                        memo.hovered_rich_link = next;
                        drop(memo);
                        let _ = hover_host.update(cx, |_, cx| cx.notify());
                    }),
            )
            .into_any_element()
    };

    let mut row =
        rich_content_row(body_color, false).child(div().max_w_full().min_w_0().child(content));
    if msg.is_edited {
        row = row.child(edited_marker(theme, ctx.locale));
    }
    row.cursor(gpui::CursorStyle::IBeam).into_any_element()
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
    let mut span_key = 0usize;
    for span in msg
        .spans
        .iter()
        .filter(|s| matches!(s, MessageSpan::Mention { .. }))
    {
        row = append_span(row, span, ctx, body_color, px(EMOJI_SIZE), &mut span_key);
    }
    if msg.is_edited {
        row = row.child(edited_marker(ctx.theme, ctx.locale));
    }
    row.into_any_element()
}

fn render_selectable_segmented_content(
    msg: &Message,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    emoji_size: Pixels,
    selection_context: Option<&SelectableTextContext>,
) -> AnyElement {
    let selection_layout = selection_context
        .is_none()
        .then(|| memoized_selectable_message_layout(msg, ctx));
    let canonical = selection_context.map_or_else(
        || selection_layout.as_ref().unwrap().text.clone(),
        |context| context.canonical.clone(),
    );
    render_selectable_segmented_spans(
        &msg.spans,
        msg,
        ctx,
        body_color,
        emoji_size,
        None,
        0,
        &canonical,
        msg.is_edited,
        selection_context,
        false,
    )
}

fn render_selectable_segmented_spans(
    spans: &[MessageSpan],
    msg: &Message,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    emoji_size: Pixels,
    text_size: Option<Pixels>,
    initial_base: usize,
    canonical: &SharedString,
    show_edited: bool,
    selection_context: Option<&SelectableTextContext>,
    block_layout: bool,
) -> AnyElement {
    let selected = selection_context.map_or_else(
        || ctx.selection.borrow().range_for_message(msg.id, canonical),
        |selection| selection.selected.clone(),
    );
    let mut base = initial_base;
    let mut owned_segments = selection_context.is_none().then(|| {
        ctx.selection
            .borrow_mut()
            .take_segment_buffer(msg.id, canonical.clone())
    });
    let mut context_segments = selection_context.map(|context| context.segments.borrow_mut());
    let segments: &mut Vec<TextSegment> = match (&mut owned_segments, &mut context_segments) {
        (Some(segments), None) => segments,
        (None, Some(segments)) => segments,
        _ => unreachable!("exactly one segment buffer is available"),
    };
    let visuals = selected.as_ref().map(|_| Rc::new(RefCell::new(Vec::new())));
    let mut row = rich_content_row(body_color, false).relative().gap_x(px(0.));
    if let (Some(paint_visuals), Some(paint_selection)) = (visuals.clone(), selected.clone()) {
        row = row.child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    paint_continuous_selection(&paint_visuals.borrow(), &paint_selection, window);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
    }
    if let Some(text_size) = text_size {
        row = row.text_size(text_size);
    }
    let mut link_part_index = 0usize;
    for span in spans {
        match span {
            MessageSpan::Text(text) => {
                let pieces = memoized_selectable_text_pieces(text, ctx);
                for piece in pieces.iter() {
                    match piece {
                        CachedSelectableTextPiece::LineBreak => {
                            row = row.child(div().w_full().h_0());
                        }
                        CachedSelectableTextPiece::Text { text, range } => {
                            let chunk_base = base + range.start;
                            let styled = selectable_segment_shared(text.clone(), chunk_base, None);
                            if let Some(visuals) = visuals.as_ref() {
                                visuals.borrow_mut().push(SelectionTextVisual {
                                    layout: styled.layout().clone(),
                                    range: chunk_base..chunk_base + text.len(),
                                });
                            }
                            segments.push(TextSegment::text(
                                styled.layout().clone(),
                                chunk_base..chunk_base + text.len(),
                            ));
                            row = row.child(styled);
                        }
                    }
                }
                base += text.len();
            }
            MessageSpan::Emoji {
                name,
                emoji_id,
                src,
            } => {
                let end = base + name.len();
                let is_selected = selected
                    .as_ref()
                    .is_some_and(|range| range.start < end && range.end > base);
                let bounds = Rc::new(Cell::new(None));
                segments.push(TextSegment::bounded(base..end, bounds.clone()));
                row = row.child(SelectableRegion::new(
                    render_emoji_span(name, emoji_id, src, body_color, ctx, emoji_size, base),
                    bounds,
                    is_selected.then(|| rgba(SELECTION_BG)),
                ));
                base = end;
            }
            MessageSpan::Mention {
                display,
                user_id,
                role_id,
            } => {
                let end = base + display.len();
                let styled = selectable_segment(display, base, selected.as_ref());
                segments.push(TextSegment::text(styled.layout().clone(), base..end));
                row = row.child(render_mention_chip_with_child(
                    user_id.as_deref(),
                    role_id.as_deref(),
                    ctx,
                    styled,
                ));
                base = end;
            }
            MessageSpan::Bold(text) => {
                let end = base + text.len();
                let styled = selectable_segment(text, base, selected.as_ref());
                segments.push(TextSegment::text(styled.layout().clone(), base..end));
                row = row.child(
                    div()
                        .max_w_full()
                        .min_w_0()
                        .font_weight(FontWeight::BOLD)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::Code(text) => {
                let end = base + text.len();
                let styled = selectable_segment(text, base, selected.as_ref());
                segments.push(TextSegment::text(styled.layout().clone(), base..end));
                row = row.child(
                    div()
                        .max_w_full()
                        .min_w_0()
                        .px_2()
                        .rounded_md()
                        .bg(ctx.theme.tokens.bg_active_member_channel)
                        .text_size(px(14.))
                        .text_color(ctx.theme.tokens.text_secondary)
                        .child(styled),
                );
                base = end;
            }
            MessageSpan::CodeBlock {
                text,
                fenced_source,
                ..
            } => {
                let end = base + text.len();
                let styled = selectable_segment(text, base, selected.as_ref());
                segments.push(TextSegment::text(styled.layout().clone(), base..end));
                row = row.child(
                    div()
                        .flex_basis(relative(1.))
                        .w_full()
                        .min_w_0()
                        .my_1()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(ctx.theme.tokens.border_primary)
                        .bg(ctx.theme.tokens.bg_markdown_code)
                        .text_size(px(14.))
                        .text_color(ctx.theme.tokens.text_theme_message)
                        .child(styled)
                        .child(code_block_copy_overlay(
                            SharedString::from(format!("code-copy-{}-{base}", msg.row_anchor_id.0)),
                            fenced_source.clone(),
                            ctx.theme,
                        )),
                );
                base = end;
            }
            MessageSpan::Link { text, url, kind } if *kind == LinkKind::Plain => {
                let resolved = resolve_link_url(url, text);
                let mut line_base = 0usize;
                for (line_index, line) in text.split('\n').enumerate() {
                    if line_index > 0 {
                        row = row.child(div().w_full().h_0());
                    }
                    let mut part_base = 0usize;
                    for part in split_unbreakable(line) {
                        let start = base + line_base + part_base;
                        let end = start + part.len();
                        let styled = selectable_segment(&part, start, selected.as_ref());
                        segments.push(TextSegment::text(styled.layout().clone(), start..end));
                        let selection = ctx.selection.clone();
                        let target = resolved.clone();
                        let link_key = link_part_index;
                        link_part_index += 1;
                        row = row.child(
                            div()
                                .id(("msg-link", link_key))
                                .cursor_pointer()
                                .text_color(ctx.theme.tokens.mention_color)
                                .on_click(move |_, _, cx| {
                                    if !selection.borrow().has_selection() {
                                        open_message_link(target.clone(), cx);
                                    }
                                })
                                .child(styled),
                        );
                        part_base += part.len();
                    }
                    line_base += line.len() + 1;
                }
                base += text.len();
            }
            MessageSpan::Link { text, url, kind } => {
                let resolved = SharedString::from(resolve_link_url(url, text));
                let mut url_col = div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .w_full()
                    .text_size(px(14.))
                    .text_color(ctx.theme.tokens.mention_color)
                    .cursor(gpui::CursorStyle::IBeam);
                let mut line_base = 0usize;
                for line in text.split('\n') {
                    if line.trim().is_empty() {
                        line_base += line.len() + 1;
                        continue;
                    }
                    let mut url_row = div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_baseline()
                        .min_w_0()
                        .w_full();
                    let mut part_base = 0usize;
                    for part in split_unbreakable(line) {
                        let start = base + line_base + part_base;
                        let end = start + part.len();
                        let styled = selectable_segment(&part, start, selected.as_ref());
                        if let Some(visuals) = visuals.as_ref() {
                            visuals.borrow_mut().push(SelectionTextVisual {
                                layout: styled.layout().clone(),
                                range: start..end,
                            });
                        }
                        segments.push(TextSegment::text(styled.layout().clone(), start..end));
                        url_row = url_row.child(styled);
                        part_base += part.len();
                    }
                    url_col = url_col.child(url_row);
                    line_base += line.len() + 1;
                }
                let card_key = link_part_index;
                link_part_index += 1;
                row = row.child(render_social_link_card(
                    *kind,
                    &ctx.selection,
                    resolved,
                    card_key,
                    url_col,
                ));
                base += text.len();
            }
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => {
                let chip = hashtag_chip(display, channel_id.as_deref(), ctx.locale, ctx.app);
                let end = base + INLINE_ICON_PLACEHOLDER.len_utf8() + chip.label.len();
                let is_selected = selected
                    .as_ref()
                    .is_some_and(|range| range.start < end && range.end > base);
                let bounds = Rc::new(Cell::new(None));
                segments.push(TextSegment::bounded(base..end, bounds.clone()));
                row = row.child(SelectableRegion::new(
                    render_hashtag_chip(chip, ctx),
                    bounds,
                    is_selected.then(|| rgba(SELECTION_BG)),
                ));
                base = end;
            }
            MessageSpan::Canvas { title, .. } => {
                let end = base + title.len();
                let is_selected = selected
                    .as_ref()
                    .is_some_and(|range| range.start < end && range.end > base);
                let bounds = Rc::new(Cell::new(None));
                segments.push(TextSegment::bounded(base..end, bounds.clone()));
                row = row.child(SelectableRegion::new(
                    render_canvas_chip(title.clone()),
                    bounds,
                    is_selected.then(|| rgba(SELECTION_BG)),
                ));
                base = end;
            }
            MessageSpan::Heading { level, text } => {
                let end = base + text.len();
                let styled = selectable_segment(text, base, selected.as_ref());
                segments.push(TextSegment::text(styled.layout().clone(), base..end));
                let mut heading = div()
                    .w_full()
                    .min_w_0()
                    .text_size(heading_size(*level))
                    .line_height(heading_line_height(*level))
                    .font_weight(FontWeight::BOLD);
                if !block_layout {
                    heading = heading.my(px(4.));
                }
                row = row.child(heading.child(styled));
                base = end;
            }
        }
    }
    if show_edited {
        row = row.child(edited_marker(ctx.theme, ctx.locale));
    }
    drop(context_segments);
    if let Some(segments) = owned_segments {
        ctx.selection
            .borrow_mut()
            .store_segment_buffer(msg.id, canonical.clone(), segments);
    }
    row.cursor(gpui::CursorStyle::IBeam).into_any_element()
}

pub(crate) fn render_selectable_embed_description(
    spans: &[MessageSpan],
    msg: &Message,
    base: usize,
    selection_context: &SelectableTextContext,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    text_size: Pixels,
) -> AnyElement {
    render_selectable_segmented_spans(
        spans,
        msg,
        ctx,
        body_color,
        px(EMOJI_SIZE),
        Some(text_size),
        base,
        &selection_context.canonical,
        false,
        Some(selection_context),
        true,
    )
}

pub(crate) struct SelectableTextContext {
    message_id: MessageId,
    canonical: SharedString,
    selected: Option<Range<usize>>,
    selection: SharedSelection,
    segments: RefCell<Vec<TextSegment>>,
}

impl SelectableTextContext {
    pub(crate) fn new(
        message_id: MessageId,
        canonical: SharedString,
        selection: SharedSelection,
    ) -> Self {
        let selected = selection.borrow().range_for_message(message_id, &canonical);
        let segments = {
            let mut state = selection.borrow_mut();
            state.take_segment_buffer(message_id, canonical.clone())
        };
        Self {
            message_id,
            canonical,
            selected,
            selection,
            segments: RefCell::new(segments),
        }
    }

    pub(crate) fn text_node(&self, text: &str, range: Range<usize>) -> StyledText {
        let styled = selectable_segment(text, range.start, self.selected.as_ref());
        self.segments
            .borrow_mut()
            .push(TextSegment::text(styled.layout().clone(), range));
        styled
    }

    pub(crate) fn end_truncated_text_node(&self, text: &str, range: Range<usize>) -> StyledText {
        let styled = selectable_segment(text, range.start, self.selected.as_ref());
        self.segments
            .borrow_mut()
            .push(TextSegment::end_truncated_text(
                styled.layout().clone(),
                range,
            ));
        styled
    }

    pub(crate) fn clipped_text_node(
        &self,
        text: &str,
        range: Range<usize>,
        clip: Rc<Cell<Option<Bounds<Pixels>>>>,
    ) -> StyledText {
        let styled = selectable_segment(text, range.start, self.selected.as_ref());
        self.segments
            .borrow_mut()
            .push(TextSegment::text(styled.layout().clone(), range).clipped(clip));
        styled
    }

    pub(crate) fn is_selected(&self, range: &Range<usize>) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|selected| selected.start < range.end && selected.end > range.start)
    }

    pub(crate) fn push_segment(&self, segment: TextSegment) {
        self.segments.borrow_mut().push(segment);
    }

    pub(crate) fn selection(&self) -> SharedSelection {
        self.selection.clone()
    }
}

impl Drop for SelectableTextContext {
    fn drop(&mut self) {
        let segments = self.segments.get_mut();
        self.selection.borrow_mut().store_segment_buffer(
            self.message_id,
            self.canonical.clone(),
            std::mem::take(segments),
        );
    }
}

pub(crate) struct SelectableSectionCursor {
    offset: usize,
    has_section: bool,
}

impl SelectableSectionCursor {
    pub(crate) fn new(offset: usize) -> Self {
        Self {
            offset,
            has_section: false,
        }
    }

    pub(crate) fn section(&mut self, text: &str) -> Option<Range<usize>> {
        if text.is_empty() {
            return None;
        }
        if self.has_section {
            self.offset += 1;
        }
        let range = self.offset..self.offset + text.len();
        self.offset = range.end;
        self.has_section = true;
        Some(range)
    }

    pub(crate) fn inline(&mut self, text: &str) -> Option<Range<usize>> {
        if text.is_empty() {
            return None;
        }
        let range = self.offset..self.offset + text.len();
        self.offset = range.end;
        self.has_section = true;
        Some(range)
    }
}

pub(crate) fn selectable_spans_text(spans: &[MessageSpan], locale: &str, cx: &App) -> String {
    let mut text = String::new();
    for span in spans {
        match span {
            MessageSpan::Text(value) | MessageSpan::Bold(value) | MessageSpan::Code(value) => {
                text.push_str(value)
            }
            MessageSpan::CodeBlock { text: value, .. }
            | MessageSpan::Link { text: value, .. }
            | MessageSpan::Mention { display: value, .. }
            | MessageSpan::Emoji { name: value, .. }
            | MessageSpan::Heading { text: value, .. } => text.push_str(value),
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => {
                text.push(INLINE_ICON_PLACEHOLDER);
                text.push_str(&hashtag_chip(display, channel_id.as_deref(), locale, cx).label);
            }
            MessageSpan::Canvas { title, .. } => text.push_str(title),
        }
    }
    text
}

pub(crate) fn code_block_copy_overlay(
    id: impl Into<gpui::ElementId>,
    text: SharedString,
    theme: &Theme,
) -> gpui::Div {
    let overlay = div().absolute().top(px(8.)).right(px(8.));
    if text.is_empty() {
        return overlay;
    }
    overlay.child(CopyButton::new(id, text, theme.tokens.text_theme_message))
}

fn append_selectable_section(text: &mut String, section: &str) {
    if section.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(section);
}

pub(crate) fn selectable_embed_text(
    embed: &Embed,
    message_code: MessageCode,
    locale: &str,
    cx: &App,
) -> String {
    let mut text = String::new();
    if message_code == MessageCode::ShareContact
        || embed
            .fields
            .first()
            .is_some_and(|field| field.value.as_ref() == "share_contact")
    {
        let field = |name: &str| {
            embed
                .fields
                .iter()
                .find(|field| field.name.as_ref() == name)
                .map(|field| field.value.as_ref())
                .unwrap_or("")
        };
        let user_id = field("user_id");
        let username = field("username");
        if user_id.is_empty() || username.is_empty() {
            return text;
        }
        let display_name = field("display_name");
        append_selectable_section(
            &mut text,
            if display_name.is_empty() {
                username
            } else {
                display_name
            },
        );
        if !username.is_empty() {
            append_selectable_section(&mut text, &format!("@{username}"));
        }
        return text;
    }
    if let Some(author) = embed.author.as_ref() {
        append_selectable_section(&mut text, &author.name);
    }
    append_selectable_section(&mut text, &embed.title);
    let description = selectable_spans_text(&embed.description_spans, locale, cx);
    append_selectable_section(&mut text, &description);
    for field in embed.fields.iter() {
        append_selectable_section(&mut text, &field.name);
        append_selectable_section(&mut text, &field.value);
    }
    if let Some(footer) = embed.footer.as_ref() {
        append_selectable_section(&mut text, &footer.text);
    }
    append_selectable_section(&mut text, &embed.footer_date);
    text
}

pub(crate) fn selectable_message_body_text(msg: &Message, locale: &str, cx: &App) -> String {
    let mut text = if msg.is_deleted_placeholder || msg.spans.is_empty() {
        let mut text = String::new();
        text.push_str(&msg.content);
        text
    } else {
        selectable_spans_text(&msg.spans, locale, cx)
    };
    if !msg.attachments.is_empty() {
        text.push(ATTACHMENT_PLACEHOLDER);
    }
    text
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SelectableMessageLayoutIdentity {
    update_time: i64,
    content_ptr: usize,
    content_len: usize,
    raw_content_ptr: usize,
    raw_content_len: usize,
    sender_name_ptr: usize,
    sender_name_len: usize,
    spans_ptr: usize,
    spans_len: usize,
    ogp_ptr: usize,
    poll_ptr: usize,
    embeds_ptr: usize,
    embeds_len: usize,
    invite_ptr: usize,
    token_transaction_ptr: usize,
    code: MessageCode,
    call_log: Option<(bool, i32, bool)>,
    has_attachments: bool,
    is_deleted_placeholder: bool,
    sender_is_current_user: bool,
}

pub(crate) struct SelectableMessageLayoutCacheEntry {
    identity: SelectableMessageLayoutIdentity,
    locale: SharedString,
    layout: Rc<SelectableMessageLayout>,
}

#[derive(Default)]
pub(crate) struct SelectableMessageLayout {
    pub(crate) text: SharedString,
    pub(crate) primary: Option<Range<usize>>,
    pub(crate) invite: Option<Range<usize>>,
    pub(crate) ogp: Option<Range<usize>>,
    pub(crate) attachment: Option<Range<usize>>,
    pub(crate) embeds: Vec<Option<Range<usize>>>,
}

fn selectable_message_layout_identity(
    msg: &Message,
    current_user_id: &str,
) -> SelectableMessageLayoutIdentity {
    SelectableMessageLayoutIdentity {
        update_time: msg.update_time,
        content_ptr: msg.content.as_ptr() as usize,
        content_len: msg.content.len(),
        raw_content_ptr: msg
            .raw_content
            .as_deref()
            .map_or(0, |value| value.as_ptr() as usize),
        raw_content_len: msg.raw_content.as_deref().map_or(0, str::len),
        sender_name_ptr: msg.sender_name.as_ref().as_ptr() as usize,
        sender_name_len: msg.sender_name.len(),
        spans_ptr: msg.spans.as_ptr() as usize,
        spans_len: msg.spans.len(),
        ogp_ptr: msg
            .ogp
            .as_deref()
            .map_or(0, |value| value as *const _ as usize),
        poll_ptr: msg
            .poll
            .as_deref()
            .map_or(0, |value| value as *const _ as usize),
        embeds_ptr: msg.embeds.as_ptr() as usize,
        embeds_len: msg.embeds.len(),
        invite_ptr: msg
            .invite
            .as_deref()
            .map_or(0, |value| value as *const _ as usize),
        token_transaction_ptr: msg
            .token_transaction
            .as_deref()
            .map_or(0, |value| value as *const _ as usize),
        code: msg.code,
        call_log: msg
            .call_log
            .map(|log| (log.is_video, log.log_type.raw(), log.show_call_back)),
        has_attachments: !msg.attachments.is_empty(),
        is_deleted_placeholder: msg.is_deleted_placeholder,
        sender_is_current_user: msg.sender_id == current_user_id,
    }
}

pub(crate) fn memoized_selectable_message_layout(
    msg: &Message,
    ctx: &RowCtx,
) -> Rc<SelectableMessageLayout> {
    let identity = selectable_message_layout_identity(msg, ctx.current_user_id);
    if let Some(layout) = {
        let memo = ctx.row_memo.borrow();
        memo.selection_layouts.get(&msg.id).and_then(|cached| {
            (cached.identity == identity && cached.locale.as_ref() == ctx.locale)
                .then(|| cached.layout.clone())
        })
    } {
        return layout;
    }

    let layout = Rc::new(selectable_message_layout(
        msg,
        ctx.locale,
        ctx.current_user_id,
        ctx.app,
    ));
    let mut memo = ctx.row_memo.borrow_mut();
    if memo.selection_layouts.len() >= SELECTABLE_LAYOUT_PLAN_LIMIT
        && !memo.selection_layouts.contains_key(&msg.id)
    {
        memo.selection_layouts.clear();
    }
    memo.selection_layouts.insert(
        msg.id,
        SelectableMessageLayoutCacheEntry {
            identity,
            locale: SharedString::from(ctx.locale),
            layout: layout.clone(),
        },
    );
    layout
}

fn append_layout_section(text: &mut String, section: &str) -> Option<Range<usize>> {
    if section.is_empty() {
        return None;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    let start = text.len();
    text.push_str(section);
    Some(start..text.len())
}

pub(crate) fn selectable_message_layout(
    msg: &Message,
    locale: &str,
    current_user_id: &str,
    cx: &App,
) -> SelectableMessageLayout {
    let primary_text = if let Some(call_log) = msg.call_log.as_ref() {
        super::call_log_card::selectable_call_log_text(
            msg,
            call_log,
            msg.sender_id.as_str() == current_user_id,
            locale,
        )
    } else if msg.code == MessageCode::SendToken {
        super::token_transaction_card::selectable_token_transaction_text(msg)
    } else if let Some(poll) = msg.poll.as_ref() {
        super::poll_card::selectable_poll_text(poll)
    } else if matches!(
        msg.code,
        MessageCode::CreatePin | MessageCode::CreateThread | MessageCode::DeleteThread
    ) {
        super::system_row::selectable_system_message_text(msg, locale)
    } else {
        let mut text = selectable_message_body_text(msg, locale, cx);
        if !msg.attachments.is_empty() {
            text.truncate(text.len().saturating_sub(ATTACHMENT_PLACEHOLDER.len_utf8()));
        }
        text
    };

    let mut text = String::new();
    let primary = append_layout_section(&mut text, &primary_text);
    let invite =
        if msg.call_log.is_none() && msg.code != MessageCode::SendToken && msg.poll.is_none() {
            msg.invite.as_deref().and_then(|invite| {
                append_layout_section(
                    &mut text,
                    &super::invite_card::selectable_invite_text(invite, locale),
                )
            })
        } else {
            None
        };
    let ogp = msg.ogp.as_deref().and_then(|ogp| {
        append_layout_section(&mut text, &super::ogp_embed::selectable_ogp_text(ogp))
    });
    let attachment = if msg.attachments.is_empty() {
        None
    } else {
        let start = text.len();
        text.push(ATTACHMENT_PLACEHOLDER);
        Some(start..text.len())
    };
    let mut embeds = Vec::with_capacity(msg.embeds.len());
    for embed in msg.embeds.iter() {
        let embed_text = selectable_embed_text(embed, msg.code, locale, cx);
        embeds.push(append_layout_section(&mut text, &embed_text));
    }
    SelectableMessageLayout {
        text: text.into(),
        primary,
        invite,
        ogp,
        attachment,
        embeds,
    }
}

pub(crate) fn selectable_message_text(
    msg: &Message,
    locale: &str,
    current_user_id: &str,
    cx: &App,
) -> String {
    selectable_message_layout(msg, locale, current_user_id, cx)
        .text
        .to_string()
}

struct SelectionTextVisual {
    layout: TextLayout,
    range: Range<usize>,
}

#[derive(Clone)]
pub(crate) enum CachedSelectableTextPiece {
    Text {
        text: SharedString,
        range: Range<usize>,
    },
    LineBreak,
}

fn memoized_selectable_text_pieces(
    text: &SharedString,
    ctx: &RowCtx,
) -> Rc<[CachedSelectableTextPiece]> {
    if let Some(pieces) = ctx
        .row_memo
        .borrow()
        .selection_text_pieces
        .get(text)
        .cloned()
    {
        return pieces;
    }

    let mut pieces = Vec::new();
    let mut line_base = 0usize;
    for (line_index, line) in text.split('\n').enumerate() {
        if line_index > 0 {
            pieces.push(CachedSelectableTextPiece::LineBreak);
        }
        for range in selectable_text_chunks(line) {
            let chunk = &line[range.clone()];
            let trimmed = chunk.trim();
            if trimmed.is_empty() {
                continue;
            }
            if chunk.chars().any(char::is_whitespace) || trimmed.chars().count() <= 32 {
                pieces.push(CachedSelectableTextPiece::Text {
                    text: SharedString::from(chunk),
                    range: line_base + range.start..line_base + range.end,
                });
                continue;
            }
            let leading = chunk.len() - chunk.trim_start().len();
            let mut part_offset = leading;
            for part in split_unbreakable(trimmed) {
                let part_len = part.len();
                let start = line_base + range.start + part_offset;
                let end = start + part_len;
                pieces.push(CachedSelectableTextPiece::Text {
                    text: SharedString::from(part),
                    range: start..end,
                });
                part_offset += part_len;
            }
        }
        line_base += line.len() + 1;
    }
    let pieces: Rc<[CachedSelectableTextPiece]> = pieces.into();
    let mut memo = ctx.row_memo.borrow_mut();
    if memo.selection_text_pieces.len() >= SELECTABLE_TEXT_PIECE_LIMIT
        && !memo.selection_text_pieces.contains_key(text)
    {
        memo.selection_text_pieces.clear();
    }
    memo.selection_text_pieces
        .insert(text.clone(), pieces.clone());
    pieces
}

fn paint_continuous_selection(
    visuals: &[SelectionTextVisual],
    selection: &Range<usize>,
    window: &mut gpui::Window,
) {
    let mut current: Option<Bounds<Pixels>> = None;
    for visual in visuals {
        let start = selection.start.max(visual.range.start);
        let end = selection.end.min(visual.range.end);
        if start >= end {
            continue;
        }
        let Some(start_position) = visual.layout.position_for_index(start - visual.range.start)
        else {
            continue;
        };
        let Some(end_position) = visual.layout.position_for_index(end - visual.range.start) else {
            continue;
        };
        if end_position.x <= start_position.x {
            continue;
        }
        let next = Bounds {
            origin: point(start_position.x, start_position.y),
            size: size(
                end_position.x - start_position.x,
                visual.layout.line_height(),
            ),
        };
        if let Some(active) = current.as_mut()
            && active.top() == next.top()
            && active.bottom() == next.bottom()
            && next.left() <= active.right() + px(2.)
        {
            active.size.width = active.size.width.max(next.right() - active.left());
            continue;
        }
        if let Some(active) = current.replace(next) {
            window.paint_quad(fill(active, rgba(SELECTION_BG)));
        }
    }
    if let Some(active) = current {
        window.paint_quad(fill(active, rgba(SELECTION_BG)));
    }
}

fn selectable_text_chunks(line: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0usize;
    std::iter::from_fn(move || {
        if start >= line.len() {
            return None;
        }

        let mut has_non_whitespace = false;
        let mut previous_was_whitespace = false;
        for (relative_index, character) in line[start..].char_indices() {
            let is_whitespace = character.is_whitespace();
            if !is_whitespace && previous_was_whitespace && has_non_whitespace {
                let end = start + relative_index;
                let range = start..end;
                start = end;
                return Some(range);
            }
            has_non_whitespace |= !is_whitespace;
            previous_was_whitespace = is_whitespace;
        }

        let range = start..line.len();
        start = line.len();
        Some(range)
    })
}

fn selectable_segment(text: &str, base: usize, selection: Option<&Range<usize>>) -> StyledText {
    selectable_segment_shared(SharedString::from(text), base, selection)
}

fn selectable_segment_shared(
    text: SharedString,
    base: usize,
    selection: Option<&Range<usize>>,
) -> StyledText {
    let Some(selection) = selection else {
        return StyledText::new(text);
    };
    let start = selection.start.max(base);
    let end = selection.end.min(base + text.len());
    if start >= end {
        return StyledText::new(text);
    }
    StyledText::new(text).with_highlights(merge_selection_background(
        &[],
        start - base..end - base,
        rgba(SELECTION_BG).into(),
    ))
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

pub fn append_selectable_system_mention_spans(
    mut row: gpui::Div,
    msg: &Message,
    ctx: &RowCtx,
    cursor: &mut SelectableSectionCursor,
    selection_context: &SelectableTextContext,
) -> gpui::Div {
    for span in msg
        .spans
        .iter()
        .filter(|s| matches!(s, MessageSpan::Mention { .. }))
    {
        if let MessageSpan::Mention {
            display,
            user_id,
            role_id,
        } = span
            && let Some(range) = cursor.inline(display)
        {
            row = row.child(render_mention_chip_with_child(
                user_id.as_deref(),
                role_id.as_deref(),
                ctx,
                selection_context.text_node(display, range),
            ));
        }
    }
    row
}

fn append_span(
    mut row: gpui::Div,
    span: &MessageSpan,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    emoji_size: Pixels,
    span_key: &mut usize,
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
                .max_w_full()
                .min_w_0()
                .font_weight(FontWeight::BOLD)
                .text_color(body_color)
                .child(text.clone()),
        ),
        MessageSpan::Code(text) => row.child(
            div()
                .max_w_full()
                .min_w_0()
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
            let key = *span_key;
            *span_key += 1;
            row.child(render_social_link_card(
                *kind,
                &ctx.selection,
                SharedString::from(resolve_link_url(url, text)),
                key,
                render_social_link_url_row(text, theme),
            ))
        }
        MessageSpan::Link { text, url, .. } => {
            let key = *span_key;
            *span_key += 1;
            row.child(message_link_element(
                text,
                &resolve_link_url(url, text),
                theme.tokens.mention_color,
                ctx.selection.clone(),
                key,
            ))
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
            hashtag_chip(display, channel_id.as_deref(), ctx.locale, ctx.app),
            ctx,
        )),
        MessageSpan::Emoji {
            name,
            emoji_id,
            src,
        } => {
            let key = *span_key;
            *span_key += 1;
            row.child(render_emoji_span(
                name, emoji_id, src, body_color, ctx, emoji_size, key,
            ))
        }
        MessageSpan::Canvas { title, .. } => row.child(render_canvas_chip(title.clone())),
        MessageSpan::Heading { level, text } => row.child(render_heading(*level, text.clone())),
    }
}

pub(crate) fn heading_size(level: u8) -> Pixels {
    match level {
        1 => px(36.),
        2 => px(30.),
        3 => px(24.),
        4 => px(20.),
        5 => px(18.),
        _ => px(16.),
    }
}

pub(crate) fn heading_line_height(level: u8) -> impl Into<gpui::DefiniteLength> {
    match level {
        1 => rems(2.5),
        2 => rems(2.25),
        3 => rems(2.),
        4 => rems(1.75),
        5 => rems(1.75),
        _ => rems(1.5),
    }
}

fn render_heading(level: u8, text: SharedString) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .my(px(4.))
        .text_size(heading_size(level))
        .line_height(heading_line_height(level))
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
        .flex_row()
        .max_w_full()
        .min_w_0()
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
        .child(div().min_w_0().child(title))
        .into_any_element()
}

fn render_social_link_url_row(text: &SharedString, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .min_w_0()
        .w_full()
        .text_size(px(14.))
        .text_color(theme.tokens.mention_color)
        .cursor(gpui::CursorStyle::IBeam)
        .children(link_to_display_segments(
            text.as_ref(),
            theme.tokens.mention_color,
        ))
        .into_any_element()
}

fn render_social_link_card(
    kind: LinkKind,
    selection: &SharedSelection,
    resolved: SharedString,
    // Keyed by position, not by url: the same social link twice in one message would otherwise
    // hash to one id and the two cards would share their interactive state.
    key: usize,
    url_row: impl IntoElement,
) -> AnyElement {
    let (accent, label) = match kind {
        LinkKind::YouTube => (YOUTUBE_ACCENT, "YouTube"),
        LinkKind::Facebook => (FACEBOOK_ACCENT, "Facebook"),
        LinkKind::TikTok => (TIKTOK_ACCENT, "TikTok"),
        LinkKind::Plain => return gpui::Empty.into_any_element(),
    };
    let id = ("msg-social", key);
    let selection = selection.clone();
    div()
        .flex()
        .flex_row()
        .flex_basis(relative(1.))
        .w_full()
        .min_w_0()
        .child(
            div()
                .id(id)
                .flex()
                .flex_col()
                .gap_1()
                .w_full()
                .min_w_0()
                .max_w(px(400.))
                .my_1()
                .p(px(16.))
                .rounded(px(4.))
                .border_l_4()
                .border_color(rgb(accent))
                .bg(rgb(SOCIAL_CARD_BG))
                .overflow_hidden()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        open_message_link(resolved.to_string(), cx);
                    }
                })
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(accent))
                        .child(label),
                )
                .child(url_row),
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
    key: usize,
) -> AnyElement {
    let src: SharedString = if precomputed_src.is_empty() {
        crate::util::imgproxy::emoji_url_sized(ctx.app, emoji_id, emoji_source_px(size)).into()
    } else {
        precomputed_src.clone()
    };
    if src.is_empty() {
        return div()
            .text_color(body_color)
            .child(name.clone())
            .into_any_element();
    }
    div()
        .flex_none()
        .size(size)
        .image_cache(ctx.icon_cache.clone())
        .child(
            img(src)
                .id(("msg-emoji-frames", key))
                .size(size)
                .object_fit(ObjectFit::Contain)
                .with_fallback(super::reaction_detail::emoji_error_fallback(
                    size,
                    ctx.theme.text_muted,
                )),
        )
        .into_any_element()
}

fn render_mention_chip(
    display: impl Into<SharedString>,
    user_id: Option<&str>,
    role_id: Option<&str>,
    ctx: &RowCtx,
) -> AnyElement {
    render_mention_chip_with_child(user_id, role_id, ctx, display.into())
}

fn render_mention_chip_with_child(
    user_id: Option<&str>,
    role_id: Option<&str>,
    ctx: &RowCtx,
    child: impl IntoElement,
) -> AnyElement {
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
        .max_w_full()
        .min_w_0()
        .px(px(1.))
        .rounded_sm()
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(color)
        .hover(move |s| s.bg(hover_bg).text_color(hover_color))
        .child(child);

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
    .max_w_full()
    .min_w_0()
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
    let selection = ctx.selection.clone();
    ClickableContainer::new(id)
        .cursor_pointer()
        .child(child)
        .on_click(move |_, window, cx| {
            if selection.borrow().has_selection() {
                return;
            }
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

fn render_hashtag_chip(chip: HashtagChip, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let bg = theme.tokens.mention_primary;
    let color = theme.tokens.mention_color;
    let hover_bg = theme.tokens.bg_mention_hover;
    let hover_color = theme.tokens.color_mention_hover;

    let inner = div()
        .flex()
        .flex_row()
        .max_w_full()
        .min_w_0()
        .items_center()
        .gap_0p5()
        .child(Icon::new(chip.icon).size_4().text_color(color))
        .child(
            div()
                .min_w_0()
                .when(chip.italic, |d| d.italic())
                .child(chip.label),
        );

    let base = div()
        .max_w_full()
        .min_w_0()
        .px(px(1.))
        .rounded_sm()
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(color)
        .hover(move |s| s.bg(hover_bg).text_color(hover_color))
        .child(inner);

    match chip.channel_id {
        Some(channel_id) => {
            let locale = ctx.locale.to_string();
            let selection = ctx.selection.clone();
            base.id(("msg-hashtag", channel_id.get() as usize))
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        navigate_to_channel(channel_id, &locale, cx);
                    }
                })
                .into_any_element()
        }
        None => base.into_any_element(),
    }
}

struct HashtagChip {
    label: SharedString,
    icon: IconName,
    italic: bool,
    channel_id: Option<ChannelId>,
}

struct ResolvedHashtag {
    name: Option<SharedString>,
    icon: IconName,
}

fn hashtag_chip(display: &str, channel_id: Option<&str>, locale: &str, cx: &App) -> HashtagChip {
    let parsed_channel = channel_id.and_then(parse_channel_id);
    let resolved = parsed_channel.and_then(|cid| hashtag_channel(cid, cx));
    hashtag_chip_for(display, parsed_channel, resolved, locale)
}

fn hashtag_chip_for(
    display: &str,
    parsed_channel: Option<ChannelId>,
    resolved: Option<ResolvedHashtag>,
    locale: &str,
) -> HashtagChip {
    if let Some(resolved) = resolved {
        return HashtagChip {
            label: resolved
                .name
                .unwrap_or_else(|| hashtag_display_label(display)),
            icon: resolved.icon,
            italic: false,
            channel_id: parsed_channel,
        };
    }
    if display.starts_with("http://") || display.starts_with("https://") {
        return HashtagChip {
            label: SharedString::new_static(mezon_i18n::t(locale, "message.unknown")),
            icon: IconName::Hashtag,
            italic: true,
            channel_id: None,
        };
    }
    if parsed_channel.is_some() {
        return HashtagChip {
            label: SharedString::new_static(mezon_i18n::t(locale, "message.noAccess")),
            icon: IconName::LockedPrivate,
            italic: false,
            channel_id: None,
        };
    }
    HashtagChip {
        label: hashtag_display_label(display),
        icon: IconName::Hashtag,
        italic: false,
        channel_id: None,
    }
}

fn hashtag_display_label(display: &str) -> SharedString {
    SharedString::from(display.strip_prefix('#').unwrap_or(display))
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
                let chip = hashtag_chip(display, channel_id.as_deref(), ctx.locale, ctx.app);
                let icon_index = text.len();
                text.push(INLINE_ICON_PLACEHOLDER);
                let label_index = text.len();
                text.push_str(&chip.label);
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
                    icon: chip.icon,
                    color: mention_color,
                });
                if let Some(cid) = chip.channel_id {
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

    let selection = ctx
        .selection
        .borrow()
        .range_for_message(msg.id, &text)
        .map(|range| (range, rgba(SELECTION_BG).into()));
    let inline = InlineContent::new(
        ("msg-inline", msg.row_anchor_id.0 as usize),
        text.into(),
        runs,
        icons,
        clicks,
        body,
        selection,
        ctx.selection.clone(),
    );
    ctx.selection
        .borrow_mut()
        .registry
        .insert(msg.id, inline.text_layout());
    Some(
        div()
            .max_w_full()
            .min_w_0()
            .child(inline)
            .into_any_element(),
    )
}

fn hashtag_channel(channel_id: ChannelId, cx: &App) -> Option<ResolvedHashtag> {
    let channels = ChannelList::global(cx);
    let store = channels.read(cx);
    store
        .find_channel_in_active_clan(channel_id)
        .or_else(|| {
            store
                .clan_id_for_channel(channel_id)
                .and_then(|clan_id| store.channel(clan_id, channel_id))
        })
        .or_else(|| store.user_channel(channel_id))
        .map(|channel| ResolvedHashtag {
            name: (!channel.name.is_empty()).then(|| SharedString::from(channel.name.as_str())),
            icon: channel_type_icon(channel.channel_type, channel.private),
        })
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

fn render_plain_text_spans(msg: &Message, ctx: &RowCtx, color: gpui::Rgba) -> AnyElement {
    let text: SharedString = match msg.spans.as_slice() {
        [MessageSpan::Text(t)] => t.clone(),
        _ => msg
            .spans
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

    let selection_range = ctx.selection.borrow().range_for_message(msg.id, &text);
    let styled = if let Some(range) = selection_range {
        let merged = merge_selection_background(&[], range, rgba(SELECTION_BG).into());
        StyledText::new(text.clone()).with_highlights(merged)
    } else {
        StyledText::new(text.clone())
    };
    ctx.selection
        .borrow_mut()
        .registry
        .insert(msg.id, styled.layout().clone());

    div()
        .w_full()
        .min_w_0()
        .min_h(px(30.))
        .cursor(gpui::CursorStyle::IBeam)
        .text_base()
        .line_height(rems(1.375))
        .text_color(color)
        .child(styled)
        .into_any_element()
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

pub(crate) fn resolve_message_link_url(url: &str, text: &str) -> String {
    resolve_link_url(url, text)
}

pub(crate) fn text_wrap_children(text: &str, color: gpui::Rgba) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        for word in line.split_whitespace() {
            for segment in split_unbreakable(word) {
                out.push(
                    div()
                        .text_sm()
                        .line_height(rems(1.25))
                        .text_color(color)
                        .child(segment)
                        .into_any_element(),
                );
            }
        }
    }
    out
}

pub(crate) fn render_pin_rich_layout_element(layout: &RichLayout, theme: &Theme) -> AnyElement {
    let palette = RichRunPalette::from_theme(theme);
    let body_color = theme.tokens.text_theme_message;

    if layout.runs.is_empty() {
        return div()
            .w_full()
            .min_w_0()
            .max_w_full()
            .text_sm()
            .line_height(rems(1.25))
            .text_color(body_color)
            .gap_x(px(4.))
            .children(text_wrap_children(layout.text.as_ref(), body_color))
            .into_any_element();
    }

    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(layout.runs.len());
    let mut click_ranges: Vec<Range<usize>> = Vec::new();
    let mut actions: Vec<RichClick> = Vec::new();
    for run in layout.runs.iter() {
        highlights.push((
            run.range.clone(),
            rich_run_highlight_with_link_underline(run.kind, &palette),
        ));
        if let Some(click) = run.click.clone() {
            click_ranges.push(run.range.clone());
            actions.push(click);
        }
    }

    let styled = StyledText::new(layout.text.clone()).with_highlights(highlights);
    let content = if actions.is_empty() {
        styled.into_any_element()
    } else {
        let actions: Arc<[RichClick]> = actions.into();
        InteractiveText::new(("pin-rich", layout.text.len()), styled)
            .on_click_shared(click_ranges.into(), move |range_ix, _, cx| {
                let Some(RichClick::Link(url)) = actions.get(range_ix) else {
                    return;
                };
                open_message_link(url.to_string(), cx);
            })
            .into_any_element()
    };

    div()
        .w_full()
        .min_w_0()
        .max_w_full()
        .text_sm()
        .line_height(rems(1.25))
        .text_color(body_color)
        .child(div().max_w_full().min_w_0().child(content))
        .into_any_element()
}

pub(crate) fn pin_link_element(
    text: &str,
    url: &str,
    color: gpui::Rgba,
    full_width: bool,
    link_key: usize,
) -> AnyElement {
    link_element(text, url, color, full_width, true, None, link_key)
}

fn message_link_element(
    text: &str,
    url: &str,
    color: gpui::Rgba,
    selection: SharedSelection,
    link_key: usize,
) -> AnyElement {
    link_element(text, url, color, false, false, Some(selection), link_key)
}

fn link_element(
    text: &str,
    url: &str,
    color: gpui::Rgba,
    full_width: bool,
    pin_typography: bool,
    selection: Option<SharedSelection>,
    link_key: usize,
) -> AnyElement {
    let resolved = SharedString::from(resolve_link_url(url, text));
    let group_name = SharedString::from(format!("msg-link-{link_key}"));
    let url_for_click = resolved.clone();
    let mut container = div()
        .id(("msg-link", link_key))
        .group(group_name.clone())
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| {
            if selection
                .as_ref()
                .is_some_and(|state| state.borrow().has_selection())
            {
                return;
            }
            open_message_link(url_for_click.to_string(), cx);
        })
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .min_w_0()
        .children(hover_link_text_segments(
            text, group_name, color, color, false,
        ));
    if pin_typography {
        container = container.text_sm().line_height(rems(1.25));
    }
    if full_width {
        container = container.w_full();
    }
    container.into_any_element()
}

pub(crate) fn hover_link_text_segments(
    text: &str,
    group_name: SharedString,
    color: gpui::Rgba,
    hover_color: gpui::Rgba,
    clickable: bool,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
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
                    out.push(hover_link_text_segment(
                        group_name.clone(),
                        segment,
                        color,
                        hover_color,
                        index,
                        true,
                        clickable,
                    ));
                    index += 1;
                }
            }
        } else {
            for segment in split_unbreakable(line) {
                out.push(hover_link_text_segment(
                    group_name.clone(),
                    segment,
                    color,
                    hover_color,
                    index,
                    true,
                    clickable,
                ));
                index += 1;
            }
        }
    }
    out
}

fn hover_link_text_segment(
    group_name: SharedString,
    display: String,
    color: gpui::Rgba,
    hover_color: gpui::Rgba,
    index: usize,
    hover_color_change: bool,
    clickable: bool,
) -> AnyElement {
    let mut segment = div()
        .id((group_name.clone(), index))
        .text_color(color)
        .group_hover(group_name, move |s| {
            if hover_color_change {
                s.underline().text_color(hover_color)
            } else {
                s.underline()
            }
        });
    if clickable {
        segment = segment.cursor_pointer();
    }
    segment.child(display).into_any_element()
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
            for segment in split_unbreakable(word) {
                out.push(segment.into_any_element());
            }
        }
    }
    out
}

fn link_to_display_segments(text: &str, color: gpui::Rgba) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        if line.chars().any(char::is_whitespace) {
            for word in line.split_whitespace() {
                for segment in split_unbreakable(word) {
                    out.push(div().text_color(color).child(segment).into_any_element());
                }
            }
        } else {
            for segment in split_unbreakable(line) {
                out.push(div().text_color(color).child(segment).into_any_element());
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
    use super::{
        RichRunPalette, RichTextRenderPlan, SelectableSectionCursor, parse_channel_id,
        rich_highlights_with_link_hover, rich_run_highlight,
        rich_run_highlight_with_link_underline, rich_text_plan_matches,
        selectable_message_layout_identity, selectable_text_chunks,
    };
    use gpui::{Hsla, SharedString};
    use mezon_store::{ChannelId, Message, MessageId, MessageSpan, RichRunKind, build_rich_layout};

    #[test]
    fn parse_channel_id_rejects_zero() {
        assert_eq!(parse_channel_id("0"), None);
        assert_eq!(parse_channel_id("12345"), Some(ChannelId(12345)));
    }

    fn test_palette() -> RichRunPalette {
        RichRunPalette {
            mention: gpui::rgb(0x3298ff).into(),
            mention_bg: gpui::rgb(0x4361ee).into(),
            role: gpui::rgb(0x2eb08c).into(),
            role_bg: gpui::rgb(0x2eb08b).into(),
            code_bg: gpui::rgb(0x111111).into(),
            link: gpui::rgb(0x3298ff).into(),
        }
    }

    #[test]
    fn role_mention_runs_use_the_everyone_palette() {
        let palette = test_palette();
        let role = rich_run_highlight(RichRunKind::RoleMention, &palette);
        assert_eq!(role.color, Some(palette.role));
        assert_eq!(role.background_color, Some(palette.role_bg));
    }

    #[test]
    fn role_and_user_mention_runs_never_share_a_colour() {
        let palette = test_palette();
        let role = rich_run_highlight(RichRunKind::RoleMention, &palette);
        let user = rich_run_highlight(RichRunKind::Mention, &palette);
        let hashtag = rich_run_highlight(RichRunKind::Hashtag, &palette);
        assert_eq!(user.color, Some(palette.mention));
        assert_eq!(user.background_color, Some(palette.mention_bg));
        assert_eq!(hashtag.color, user.color);
        assert_ne!(role.color, user.color);
        assert_ne!(role.background_color, user.background_color);
    }

    #[test]
    fn palette_memo_key_changes_when_a_role_colour_changes() {
        let palette = test_palette();
        let muted: Hsla = gpui::rgb(0x808080).into();
        let mut recoloured = palette;
        recoloured.role = gpui::rgb(0x3d8bff).into();
        assert_ne!(palette.memo_key(muted), recoloured.memo_key(muted));
    }

    #[test]
    fn rich_text_plan_reuses_only_the_same_layout_and_style() {
        let layout = build_rich_layout(&[MessageSpan::Bold("hello".into())]).unwrap();
        let colors = [Hsla::default(); 7];
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

    #[test]
    fn link_runs_do_not_paint_a_permanent_underline() {
        let palette = test_palette();
        let link = rich_run_highlight(RichRunKind::Link, &palette);
        assert_eq!(link.color, Some(palette.link));
        assert!(link.underline.is_none());
    }

    #[test]
    fn pinned_link_runs_paint_a_permanent_underline() {
        let palette = test_palette();
        let link = rich_run_highlight_with_link_underline(RichRunKind::Link, &palette);
        assert_eq!(link.color, Some(palette.link));
        assert!(link.underline.is_some());
    }

    #[test]
    fn hovered_rich_link_underlines_the_whole_range() {
        let palette = test_palette();
        let base = [(0..32, rich_run_highlight(RichRunKind::Link, &palette))];
        let hovered = rich_highlights_with_link_hover(&base, Some(&(0..32)), palette.link);
        assert_eq!(hovered.len(), 1);
        assert_eq!(hovered[0].0, 0..32);
        assert_eq!(hovered[0].1.color, Some(palette.link));
        assert!(hovered[0].1.underline.is_some());
    }

    #[test]
    fn selectable_chunks_keep_whitespace_attached_to_the_previous_word() {
        let text = "  nhìn a  Nhân ";
        let chunks = selectable_text_chunks(text)
            .map(|range| &text[range])
            .collect::<Vec<_>>();

        assert_eq!(chunks, ["  nhìn ", "a  ", "Nhân "]);
    }

    #[test]
    fn selectable_layout_identity_tracks_message_replacements() {
        let mut message = Message::new(MessageId::new(7), "hello", "1", "Alice", 1);
        let initial = selectable_message_layout_identity(&message, "1");
        assert!(initial == selectable_message_layout_identity(&message, "1"));

        message.update_time = 2;
        assert!(initial != selectable_message_layout_identity(&message, "1"));

        message.update_time = 0;
        message.sender_name = SharedString::from("Bob");
        assert!(initial != selectable_message_layout_identity(&message, "1"));
        assert!(
            selectable_message_layout_identity(&message, "1")
                != selectable_message_layout_identity(&message, "2")
        );
    }

    #[test]
    fn selectable_section_cursor_matches_newline_joined_embed_text() {
        let mut cursor = SelectableSectionCursor::new(10);

        assert_eq!(cursor.section(""), None);
        assert_eq!(cursor.section("abc"), Some(10..13));
        assert_eq!(cursor.section(""), None);
        assert_eq!(cursor.section("đỏ"), Some(14..19));
    }

    #[test]
    fn selectable_section_cursor_keeps_inline_card_fragments_contiguous() {
        let mut cursor = SelectableSectionCursor::new(4);

        assert_eq!(cursor.inline("abc"), Some(4..7));
        assert_eq!(cursor.inline(" "), Some(7..8));
        assert_eq!(cursor.inline("đỏ"), Some(8..13));
    }
}

#[cfg(test)]
mod hashtag_label_tests {
    use super::{ChannelId, HashtagChip, IconName, ResolvedHashtag, hashtag_chip_for};
    use gpui::SharedString;

    const LINK: &str = "https://mezon.ai/chat/clans/1/channels/2";

    fn resolved(name: Option<&str>) -> Option<ResolvedHashtag> {
        Some(ResolvedHashtag {
            name: name.map(SharedString::from),
            icon: IconName::Hashtag,
        })
    }

    fn chip(
        display: &str,
        channel_id: Option<i64>,
        resolved: Option<ResolvedHashtag>,
    ) -> HashtagChip {
        hashtag_chip_for(display, channel_id.map(ChannelId), resolved, "en")
    }

    #[test]
    fn resolved_channel_wins_over_the_raw_url() {
        assert_eq!(
            chip(LINK, Some(2), resolved(Some("general"))).label,
            "general"
        );
    }

    #[test]
    fn unresolved_channel_link_shows_unknown_like_react() {
        let chip = chip(LINK, Some(2), None);

        assert_eq!(chip.label, "unknown");
        assert!(chip.italic);
        assert_eq!(chip.icon.path(), IconName::Hashtag.path());
    }

    #[test]
    fn unresolved_channel_link_without_id_shows_unknown() {
        assert_eq!(chip(LINK, None, None).label, "unknown");
    }

    #[test]
    fn typed_hashtag_falls_back_to_the_name_without_the_hash() {
        assert_eq!(chip("#general", None, None).label, "general");
    }

    #[test]
    fn inaccessible_channel_shows_no_access() {
        let chip = chip("#secret", Some(999), None);

        assert_eq!(chip.label, "No Access");
        assert!(!chip.italic);
        assert_eq!(chip.icon.path(), IconName::LockedPrivate.path());
    }

    #[test]
    fn a_resolved_channel_without_a_name_keeps_the_typed_label() {
        assert_eq!(chip("#general", Some(2), resolved(None)).label, "general");
    }

    #[test]
    fn an_unreachable_channel_is_not_clickable() {
        assert!(chip("#secret", Some(999), None).channel_id.is_none());
        assert_eq!(
            chip("#g", Some(2), resolved(Some("g"))).channel_id,
            Some(ChannelId(2))
        );
    }
}
