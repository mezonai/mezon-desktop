use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, FontWeight, HighlightStyle, Hsla, InteractiveText, ObjectFit, Pixels,
    SharedString, StyledText, TextLayout, UnderlineStyle, canvas, div, fill, img, point,
    prelude::*, px, relative, rems, rgb, rgba, size,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, Embed, LinkKind, Message, MessageCode, MessageId,
    MessageSpan, PlatformStore, ProfileContext, RichClick, RichRunKind, RichToken, UserId,
    is_here_user_id,
};

use ui::Clickable;

use super::context::{RichTextRenderPlan, RowCtx};
use super::inline_content::{ClickRegion, IconOverlay, InlineContent, StyledRun};
use super::selection::{
    SelectableRegion, SharedSelection, TextSegment, merge_selection_background,
};
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

    if is_link_only(&msg.spans) && !msg.is_edited && !needs_chip_path {
        return render_link_only_spans(msg, ctx, theme, selection_context);
    }

    let has_code_block = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::CodeBlock { .. }));
    let has_custom_emoji = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::Emoji { emoji_id, .. } if !emoji_id.is_empty()));
    if !options.inline && !msg.is_edited && !has_code_block && !has_custom_emoji && !needs_chip_path
    {
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
                Vec::with_capacity(layout.runs.len());
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
    let mut styled = if let Some(range) = selection_range {
        let merged = merge_selection_background(&plan.highlights, range, rgba(SELECTION_BG).into());
        StyledText::new(plan.text.clone()).with_highlights(merged)
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
        let avatar_cache = ctx.large_avatar_cache.clone();
        let actions = plan.actions.clone();
        let locale = plan.locale.clone();
        let text_selection = ctx.selection.clone();
        InteractiveText::new(("msg-itext", msg.row_anchor_id.0 as usize), styled)
            .on_click_shared(plan.click_ranges.clone(), move |range_ix, window, cx| {
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
                    render_emoji_span(name, emoji_id, src, body_color, ctx, emoji_size),
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
            MessageSpan::CodeBlock { text, .. } => {
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
                        .child(styled),
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
                        row = row.child(
                            div()
                                .id((SharedString::from(resolved.clone()), link_part_index))
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
                        link_part_index += 1;
                    }
                    line_base += line.len() + 1;
                }
                base += text.len();
            }
            MessageSpan::Link { text, url, kind } => {
                let resolved = SharedString::from(resolve_link_url(url, text));
                let mut url_row = div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_baseline()
                    .min_w_0()
                    .w_full()
                    .text_size(px(14.))
                    .text_color(ctx.theme.tokens.mention_color)
                    .cursor(gpui::CursorStyle::IBeam);
                let mut line_base = 0usize;
                for (line_index, line) in text.split('\n').enumerate() {
                    if line_index > 0 {
                        url_row = url_row.child(div().w_full().h_0());
                    }
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
                    line_base += line.len() + 1;
                }
                row = row.child(render_social_link_card(
                    *kind,
                    &ctx.selection,
                    resolved,
                    url_row,
                ));
                base += text.len();
            }
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => {
                let resolved = channel_id
                    .as_deref()
                    .and_then(parse_channel_id)
                    .and_then(|channel_id| hashtag_channel(channel_id, ctx.app).0);
                let label = hashtag_label(display, resolved, ctx.locale);
                let end = base + INLINE_ICON_PLACEHOLDER.len_utf8() + label.len();
                let is_selected = selected
                    .as_ref()
                    .is_some_and(|range| range.start < end && range.end > base);
                let bounds = Rc::new(Cell::new(None));
                segments.push(TextSegment::bounded(base..end, bounds.clone()));
                row = row.child(SelectableRegion::new(
                    render_hashtag_chip(display.clone(), channel_id.as_deref(), ctx),
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
                row = row.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .my(px(4.))
                        .text_size(heading_size(*level))
                        .line_height(heading_line_height(*level))
                        .font_weight(FontWeight::BOLD)
                        .child(styled),
                );
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
                let resolved = channel_id
                    .as_deref()
                    .and_then(parse_channel_id)
                    .and_then(|channel_id| hashtag_channel(channel_id, cx).0);
                text.push(INLINE_ICON_PLACEHOLDER);
                text.push_str(&hashtag_label(display, resolved, locale));
            }
            MessageSpan::Canvas { title, .. } => text.push_str(title),
        }
    }
    text
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

fn selectable_message_body_shared(msg: &Message, locale: &str, cx: &App) -> SharedString {
    let rich_layout_matches = msg.attachments.is_empty()
        && msg.spans.iter().all(|span| {
            !matches!(
                span,
                MessageSpan::CodeBlock { .. } | MessageSpan::Hashtag { .. }
            )
        });
    if rich_layout_matches && let Some(layout) = msg.rich_layout.as_ref() {
        return layout.text.clone();
    }
    selectable_message_body_text(msg, locale, cx).into()
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
            row.child(render_social_link_card(
                *kind,
                &ctx.selection,
                SharedString::from(resolve_link_url(url, text)),
                render_social_link_url_row(text, theme),
            ))
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

fn heading_line_height(level: u8) -> impl Into<gpui::DefiniteLength> {
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
    url_row: impl IntoElement,
) -> AnyElement {
    let (accent, label) = match kind {
        LinkKind::YouTube => (YOUTUBE_ACCENT, "YouTube"),
        LinkKind::Facebook => (FACEBOOK_ACCENT, "Facebook"),
        LinkKind::TikTok => (TIKTOK_ACCENT, "TikTok"),
        LinkKind::Plain => (SOCIAL_CARD_BG, ""),
    };
    let id = hashed_element_id("msg-social", &resolved);
    let selection = selection.clone();
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
        .child(url_row)
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
    div()
        .flex_none()
        .size(size)
        .image_cache(ctx.icon_cache.clone())
        .child(
            img(src)
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
        .flex_row()
        .max_w_full()
        .min_w_0()
        .items_center()
        .gap_0p5()
        .child(Icon::new(icon).size_4().text_color(color))
        .child(
            div()
                .min_w_0()
                .when(unresolved_link, |d| d.italic())
                .child(label),
        );

    match parsed_channel {
        Some(channel_id) => {
            let locale = ctx.locale.to_string();
            let selection = ctx.selection.clone();
            div()
                .id(("msg-hashtag", channel_id.get() as usize))
                .max_w_full()
                .min_w_0()
                .px(px(1.))
                .rounded_sm()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .bg(bg)
                .text_color(color)
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        navigate_to_channel(channel_id, &locale, cx);
                    }
                })
                .hover(move |s| s.bg(hover_bg).text_color(hover_color))
                .child(inner)
                .into_any_element()
        }
        None => div()
            .max_w_full()
            .min_w_0()
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
    Some(inline.into_any_element())
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

fn render_link_only_spans(
    msg: &Message,
    ctx: &RowCtx,
    theme: &Theme,
    selection_context: Option<&SelectableTextContext>,
) -> AnyElement {
    let link_color = theme.tokens.mention_color;
    let text = selection_context.map_or_else(
        || selectable_message_body_shared(msg, ctx.locale, ctx.app),
        |context| context.canonical.clone(),
    );
    let selected = selection_context.map_or_else(
        || ctx.selection.borrow().range_for_message(msg.id, &text),
        |context| context.selected.clone(),
    );
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .text_base()
        .gap_1();
    let mut base = 0usize;
    let mut link_index = 0usize;
    let mut owned_segments = selection_context.is_none().then(|| {
        ctx.selection
            .borrow_mut()
            .take_segment_buffer(msg.id, text.clone())
    });
    let mut context_segments = selection_context.map(|context| context.segments.borrow_mut());
    let segments: &mut Vec<TextSegment> = match (&mut owned_segments, &mut context_segments) {
        (Some(segments), None) => segments,
        (None, Some(segments)) => segments,
        _ => unreachable!("exactly one segment buffer is available"),
    };
    for span in &msg.spans {
        match span {
            MessageSpan::Text(text) => base += text.len(),
            MessageSpan::Link { text, url, .. } => {
                let resolved = resolve_link_url(url, text);
                let mut line_base = 0usize;
                for line in text.split('\n') {
                    if !line.is_empty() {
                        let styled = selectable_segment(line, base + line_base, selected.as_ref());
                        segments.push(TextSegment::text(
                            styled.layout().clone(),
                            base + line_base..base + line_base + line.len(),
                        ));
                        col = col.child(link_block(
                            link_index,
                            resolved.clone(),
                            styled,
                            link_color,
                            ctx.selection.clone(),
                        ));
                        link_index += 1;
                    }
                    line_base += line.len() + 1;
                }
                base += text.len();
            }
            _ => {}
        }
    }
    drop(context_segments);
    if let Some(segments) = owned_segments {
        ctx.selection
            .borrow_mut()
            .store_segment_buffer(msg.id, text, segments);
    }
    div()
        .w_full()
        .min_w_0()
        .cursor(gpui::CursorStyle::IBeam)
        .child(col)
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

fn link_block(
    index: usize,
    url: String,
    display: StyledText,
    color: gpui::Rgba,
    selection: SharedSelection,
) -> AnyElement {
    div()
        .id(("msg-link", index))
        .w_full()
        .min_w_0()
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| {
            if !selection.borrow().has_selection() {
                open_message_link(url.clone(), cx);
            }
        })
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
            for segment in split_unbreakable(word) {
                out.push(segment.into_any_element());
            }
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
        RichTextRenderPlan, SelectableSectionCursor, parse_channel_id, rich_text_plan_matches,
        selectable_message_layout_identity, selectable_text_chunks,
    };
    use gpui::{Hsla, SharedString};
    use mezon_store::{ChannelId, Message, MessageId, MessageSpan, build_rich_layout};

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
