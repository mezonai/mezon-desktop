use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, FontWeight, HighlightStyle, Hsla, InteractiveText, Pixels, SharedString,
    StrikethroughStyle, StyledText, UnderlineStyle, div, prelude::*, px,
};
use mezon_store::PlatformStore;

use crate::editor::italic_font_family;
use crate::image::render_canvas_image;
use crate::view::{CANVAS_BODY_FONT_SIZE, CANVAS_BODY_LINE_HEIGHT, TipTapNode};
use mezon_theme::Theme;
use mezon_widgets::{h_flex, v_flex};

const CANVAS_HEADING1_LINE_HEIGHT: Pixels = px(36.);
const CANVAS_HEADING2_LINE_HEIGHT: Pixels = px(30.);
const CANVAS_HEADING3_LINE_HEIGHT: Pixels = px(24.);
const CANVAS_CODE_LINE_HEIGHT: Pixels = px(24.);

fn paragraph_is_empty(content: Option<&Vec<TipTapNode>>) -> bool {
    let Some(content) = content else {
        return true;
    };
    if content.is_empty() {
        return true;
    }
    !content
        .iter()
        .any(|node| node.kind == "text" && node.text.as_ref().is_some_and(|text| !text.is_empty()))
}

pub fn render_tiptap_node(doc: TipTapNode, theme: &Theme, cx: &App) -> gpui::AnyElement {
    if doc.kind == "doc" {
        let children = doc.content.unwrap_or_default();
        if children.is_empty() {
            return div().into_any_element();
        }
        return v_flex()
            .w_full()
            .children(
                children
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| render_node(n, theme, cx, 0, i as u64)),
            )
            .into_any_element();
    }
    div()
        .text_sm()
        .text_color(theme.tokens.text_theme_message)
        .child(doc.kind)
        .into_any_element()
}

fn inline_scope(parent: u64, index: usize) -> u64 {
    parent.wrapping_mul(31).wrapping_add(index as u64 + 1)
}

fn render_node(
    node: TipTapNode,
    theme: &Theme,
    cx: &App,
    depth: usize,
    scope_id: u64,
) -> gpui::AnyElement {
    let tokens = &theme.tokens;
    match node.kind.as_str() {
        "paragraph" => {
            let empty = paragraph_is_empty(node.content.as_ref());
            div()
                .w_full()
                .min_w_0()
                .when(empty, |el| el.h(CANVAS_BODY_LINE_HEIGHT))
                .text_size(CANVAS_BODY_FONT_SIZE)
                .line_height(CANVAS_BODY_LINE_HEIGHT)
                .text_color(tokens.text_theme_message)
                .child(render_inline_children(node.content, theme, scope_id))
                .into_any_element()
        }
        "heading" => {
            let level = node
                .attrs
                .as_ref()
                .and_then(|a| a.get("level"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1);
            let (size, weight, line_height) = match level {
                1 => (px(24.), FontWeight::BOLD, CANVAS_HEADING1_LINE_HEIGHT),
                2 => (px(20.), FontWeight::SEMIBOLD, CANVAS_HEADING2_LINE_HEIGHT),
                _ => (px(16.), FontWeight::SEMIBOLD, CANVAS_HEADING3_LINE_HEIGHT),
            };
            div()
                .w_full()
                .min_w_0()
                .text_size(size)
                .line_height(line_height)
                .font_weight(weight)
                .text_color(tokens.text_theme_message)
                .child(render_inline_children(node.content, theme, scope_id))
                .into_any_element()
        }
        "bulletList" | "taskList" => v_flex()
            .w_full()
            .pl(px(8. + depth as f32 * 8.))
            .children(
                node.content
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| {
                        render_list_item(n, theme, cx, depth, false, inline_scope(scope_id, i))
                    }),
            )
            .into_any_element(),
        "orderedList" => {
            let items = node.content.unwrap_or_default();
            v_flex()
                .w_full()
                .pl(px(8. + depth as f32 * 8.))
                .children(items.into_iter().enumerate().map(|(i, n)| {
                    render_list_item_numbered(n, theme, cx, depth, i + 1, inline_scope(scope_id, i))
                }))
                .into_any_element()
        }
        "blockquote" => div()
            .w_full()
            .min_w_0()
            .pl_3()
            .border_l_2()
            .border_color(tokens.button_theme_primary)
            .text_color(tokens.text_secondary)
            .child(render_block_children(
                node.content,
                theme,
                cx,
                depth,
                scope_id,
            ))
            .into_any_element(),
        "codeBlock" => {
            let text = tip_tap_to_plain_text_node(&node);
            div()
                .w_full()
                .min_w_0()
                .p_2()
                .rounded_md()
                .bg(tokens.theme_input)
                .font_family("monospace")
                .text_size(CANVAS_BODY_FONT_SIZE)
                .line_height(CANVAS_CODE_LINE_HEIGHT)
                .text_color(tokens.text_theme_message)
                .child(StyledText::new(text))
                .into_any_element()
        }
        "image" => {
            let src = node
                .attrs
                .as_ref()
                .and_then(|a| a.get("src"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            render_canvas_image(src, theme, cx)
        }
        "horizontalRule" => div()
            .w_full()
            .h(px(1.))
            .my_2()
            .bg(tokens.border_primary)
            .into_any_element(),
        _ => {
            if let Some(children) = node.content {
                v_flex()
                    .w_full()
                    .children(
                        children.into_iter().enumerate().map(|(i, n)| {
                            render_node(n, theme, cx, depth, inline_scope(scope_id, i))
                        }),
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            }
        }
    }
}

fn render_block_children(
    content: Option<Vec<TipTapNode>>,
    theme: &Theme,
    cx: &App,
    depth: usize,
    scope_id: u64,
) -> gpui::AnyElement {
    let Some(children) = content else {
        return div().into_any_element();
    };
    if children.is_empty() {
        return div().into_any_element();
    }
    v_flex()
        .w_full()
        .children(
            children
                .into_iter()
                .enumerate()
                .map(|(i, n)| render_node(n, theme, cx, depth, inline_scope(scope_id, i))),
        )
        .into_any_element()
}

fn render_list_item(
    node: TipTapNode,
    theme: &Theme,
    cx: &App,
    depth: usize,
    checked: bool,
    scope_id: u64,
) -> gpui::AnyElement {
    let prefix = if node.kind == "taskItem" {
        let done = node
            .attrs
            .as_ref()
            .and_then(|a| a.get("checked"))
            .and_then(|v| v.as_bool())
            .unwrap_or(checked);
        if done { "☑ " } else { "☐ " }
    } else {
        "• "
    };
    h_flex()
        .w_full()
        .items_start()
        .gap_1()
        .child(
            div()
                .text_size(CANVAS_BODY_FONT_SIZE)
                .line_height(CANVAS_BODY_LINE_HEIGHT)
                .text_color(theme.tokens.text_theme_message)
                .child(prefix),
        )
        .child(
            v_flex().flex_1().min_w_0().children(
                node.content
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| render_node(n, theme, cx, depth + 1, inline_scope(scope_id, i))),
            ),
        )
        .into_any_element()
}

fn render_list_item_numbered(
    node: TipTapNode,
    theme: &Theme,
    cx: &App,
    depth: usize,
    number: usize,
    scope_id: u64,
) -> gpui::AnyElement {
    h_flex()
        .w_full()
        .items_start()
        .gap_1()
        .child(
            div()
                .text_size(CANVAS_BODY_FONT_SIZE)
                .line_height(CANVAS_BODY_LINE_HEIGHT)
                .text_color(theme.tokens.text_theme_message)
                .child(format!("{number}. ")),
        )
        .child(
            v_flex().flex_1().min_w_0().children(
                node.content
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| render_node(n, theme, cx, depth + 1, inline_scope(scope_id, i))),
            ),
        )
        .into_any_element()
}

struct InlineTextParts {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    font_overrides: Vec<(Range<usize>, SharedString)>,
    link_ranges: Vec<(Range<usize>, String)>,
}

fn collect_inline_parts(nodes: &[TipTapNode], theme: &Theme, parts: &mut InlineTextParts) {
    for node in nodes {
        match node.kind.as_str() {
            "text" => append_text_node(node, theme, parts),
            "hardBreak" => parts.text.push('\n'),
            _ => {}
        }
    }
}

fn append_text_node(node: &TipTapNode, theme: &Theme, parts: &mut InlineTextParts) {
    let text = node.text.as_deref().unwrap_or("");
    if text.is_empty() {
        return;
    }
    let start = parts.text.len();
    parts.text.push_str(text);
    let range = start..parts.text.len();
    let tokens = &theme.tokens;
    let link_color: Hsla = tokens.button_theme_primary.into();
    let code_bg: Hsla = tokens.theme_input.into();
    let mut style = HighlightStyle::default();
    let mut monospace = false;
    let mut href = None;
    for mark in node.marks.as_deref().unwrap_or(&[]) {
        match mark.kind.as_str() {
            "bold" => style.font_weight = Some(FontWeight::BOLD),
            "italic" => {
                parts
                    .font_overrides
                    .push((range.clone(), italic_font_family()));
            }
            "underline" => {
                style.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                });
            }
            "strike" => {
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                });
            }
            "code" => {
                style.background_color = Some(code_bg);
                monospace = true;
            }
            "link" => {
                href = mark
                    .attrs
                    .as_ref()
                    .and_then(|a| a.get("href"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    if monospace {
        parts
            .font_overrides
            .push((range.clone(), SharedString::from("monospace")));
    }
    if let Some(url) = href {
        style.color = Some(link_color);
        style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(link_color),
            wavy: false,
        });
        parts.link_ranges.push((range.clone(), url));
    }
    if style != HighlightStyle::default() {
        parts.highlights.push((range, style));
    }
}

fn render_inline_children(
    content: Option<Vec<TipTapNode>>,
    theme: &Theme,
    scope_id: u64,
) -> gpui::AnyElement {
    let Some(children) = content else {
        return div().into_any_element();
    };
    if children.is_empty() {
        return div().into_any_element();
    }
    let mut parts = InlineTextParts {
        text: String::new(),
        highlights: Vec::new(),
        font_overrides: Vec::new(),
        link_ranges: Vec::new(),
    };
    collect_inline_parts(&children, theme, &mut parts);
    if parts.text.is_empty() {
        return div().into_any_element();
    }
    let mut styled =
        StyledText::new(SharedString::from(parts.text.clone())).with_highlights(parts.highlights);
    if !parts.font_overrides.is_empty() {
        styled = styled.with_font_family_overrides(parts.font_overrides);
    }
    let container = div().w_full().min_w_0();
    if parts.link_ranges.is_empty() {
        return container.child(styled).into_any_element();
    }
    let click_ranges: Arc<[Range<usize>]> = parts
        .link_ranges
        .iter()
        .map(|(range, _)| range.clone())
        .collect();
    let urls: Arc<[String]> = parts.link_ranges.into_iter().map(|(_, url)| url).collect();
    let interactive = InteractiveText::new(
        SharedString::from(format!("canvas-view-inline-{scope_id}")),
        styled,
    )
    .on_click_shared(click_ranges, move |range_ix, _, cx| {
        let Some(url) = urls.get(range_ix) else {
            return;
        };
        if let Some(store) = PlatformStore::try_global(cx) {
            let _ = store.read(cx).open_url_external(url);
        }
    });
    container.child(interactive).into_any_element()
}

fn tip_tap_to_plain_text_node(node: &TipTapNode) -> String {
    let mut out = String::new();
    collect_plain(node, &mut out);
    out
}

fn collect_plain(node: &TipTapNode, out: &mut String) {
    if node.kind == "text" {
        if let Some(text) = &node.text {
            out.push_str(text);
        }
        return;
    }
    if let Some(children) = &node.content {
        for child in children {
            collect_plain(child, out);
        }
    }
}
