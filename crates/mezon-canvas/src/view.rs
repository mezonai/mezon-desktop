use gpui::{
    App, ClickEvent, Context, Entity, FontWeight, Hsla, Pixels, ScrollHandle, SharedString, Window,
    div, prelude::*, px,
};
use mezon_store::{BadgeService, CanvasDetail, CanvasStore, ChannelId, ClanId, Settings, UserId};
use serde::Deserialize;
use serde_json::Value;
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::doc_view::render_tiptap_node;
use crate::editor::{CanvasEditor, CanvasEditorState};
use crate::navigation::{CanvasRoute, navigate_to_canvas, navigate_to_channel};
use crate::quill::{is_quill_delta, quill_delta_to_tiptap_json};
use mezon_theme::ActiveTheme;
use mezon_widgets::{
    Button, ButtonVariants, Icon, IconName, Input, InputState, Sizable, Size, Spinner, h_flex,
    v_flex,
};

const CANVAS_CONTENT_HORIZONTAL_PADDING: Pixels = px(128.);
pub(crate) const CANVAS_BODY_FONT_SIZE: Pixels = px(15.);
pub(crate) const CANVAS_BODY_LINE_HEIGHT: Pixels = px(22.5);

pub struct CanvasView {
    settings: Entity<Settings>,
    clan_id: ClanId,
    channel_id: ChannelId,
    canvas_id: ChannelId,
    title_input: Entity<InputState>,
    content_editor: Entity<CanvasEditorState>,
    original_title: SharedString,
    original_content: SharedString,
    loaded_content: SharedString,
    is_default: bool,
    loading: bool,
    saving: bool,
    error: Option<SharedString>,
    editing: bool,
    creator_id: UserId,
    content_scroll: ScrollHandle,
    pending_detail: Option<CanvasDetail>,
    view_doc_source: SharedString,
    view_doc: Option<TipTapNode>,
    _load_task: Option<gpui::Task<()>>,
}

impl CanvasView {
    pub fn new(
        settings: Entity<Settings>,
        clan_id: ClanId,
        channel_id: ChannelId,
        canvas_id: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = settings.read(cx).language.clone();
        let title_ph = mezon_i18n::t(&locale, "canvas.title.placeholder");
        let content_ph = mezon_i18n::t(&locale, "canvas.editor.placeholder");
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(title_ph)
                .embedded(true)
                .text_size(px(28.))
                .font_weight(FontWeight::BOLD)
                .height(px(34.))
        });
        let content_editor =
            cx.new(|cx| CanvasEditorState::new(window, cx, content_ph, locale.clone()));

        let mut view = Self {
            settings,
            clan_id,
            channel_id,
            canvas_id,
            title_input,
            content_editor,
            original_title: SharedString::default(),
            original_content: SharedString::default(),
            loaded_content: SharedString::default(),
            is_default: false,
            loading: true,
            saving: false,
            error: None,
            editing: false,
            creator_id: UserId(0),
            content_scroll: ScrollHandle::new(),
            pending_detail: None,
            view_doc_source: SharedString::default(),
            view_doc: None,
            _load_task: None,
        };
        view.start_load(window, cx);
        view
    }

    pub fn sync_route(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        canvas_id: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.clan_id == clan_id && self.channel_id == channel_id && self.canvas_id == canvas_id {
            return;
        }
        self.clan_id = clan_id;
        self.channel_id = channel_id;
        self.canvas_id = canvas_id;
        self.start_load(window, cx);
    }

    fn start_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.pending_detail = None;
        self._load_task = None;

        if self.canvas_id.is_zero() {
            self.loading = false;
            self.is_default = false;
            self.creator_id = UserId(0);
            self.title_input.update(cx, |input, cx| {
                input.set_value(String::new(), window, cx);
            });
            self.content_editor.update(cx, |editor, cx| {
                editor.set_doc("", cx);
            });
            self.original_title = SharedString::default();
            self.original_content = self.content_editor.read(cx).doc_json().into();
            self.loaded_content = self.original_content.clone();
            self.editing = true;
            self.title_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
            cx.notify();
            return;
        }

        self.loading = true;
        self.editing = false;
        cx.notify();

        let canvas_id = self.canvas_id.to_string();
        let task =
            CanvasStore::global(cx).update(cx, |store, cx| store.fetch_detail(&canvas_id, cx));
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(detail) => {
                        this.pending_detail = Some(detail);
                    }
                    Err(e) => {
                        tracing::error!("fetch canvas detail failed: {e}");
                        this.error = Some("Failed to load canvas".into());
                    }
                }
                cx.notify();
            });
        }));
    }

    fn flush_pending_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.pending_detail.take() else {
            return;
        };
        self.apply_detail(detail, window, cx);
    }

    fn apply_detail(&mut self, detail: CanvasDetail, window: &mut Window, cx: &mut Context<Self>) {
        self.is_default = detail.is_default;
        self.creator_id = detail.creator_id;
        self.original_title = detail.title.clone().into();
        self.original_content = detail.content.clone().into();
        self.loaded_content = detail.content.clone().into();
        self.title_input.update(cx, |input, cx| {
            input.set_value(detail.title.clone(), window, cx);
        });
        self.content_editor.update(cx, |editor, cx| {
            editor.set_doc(&detail.content, cx);
        });
        let can_edit = self.can_edit(cx);
        self.editing = false;
        if can_edit && detail.title.trim().is_empty() && is_tiptap_content_empty(&detail.content) {
            self.editing = true;
            self.title_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }
    }

    fn can_edit(&self, cx: &App) -> bool {
        canvas_can_edit(
            self.creator_id,
            BadgeService::global(cx).read(cx).current_user_id(cx),
        )
    }

    fn has_changes(&self, cx: &App) -> bool {
        let title_changed = self.title_input.read(cx).value() != self.original_title.as_ref();
        if title_changed {
            return true;
        }
        let editor = self.content_editor.read(cx);
        if !editor.is_content_dirty() {
            return false;
        }
        editor.doc_json() != self.original_content.as_ref()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || !self.has_changes(cx) || !self.can_edit(cx) {
            return;
        }
        let title = self.title_input.read(cx).value().to_string();
        let content = self.content_editor.read(cx).doc_json();
        self.saving = true;
        cx.notify();

        if self.canvas_id.is_zero() {
            let clan_id = self.clan_id;
            let channel_id = self.channel_id;
            let task = CanvasStore::global(cx).update(cx, |store, cx| {
                store.create(title.clone(), content.clone(), cx)
            });
            self._load_task = Some(cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(id) => {
                            if let Ok(canvas_id) = id.parse::<ChannelId>() {
                                this.canvas_id = canvas_id;
                                navigate_to_canvas(
                                    CanvasRoute {
                                        clan_id,
                                        channel_id,
                                        canvas_id,
                                    },
                                    cx,
                                );
                            }
                            this.original_title = title.into();
                            this.original_content = content.clone().into();
                            this.loaded_content = content.into();
                            this.content_editor.update(cx, |editor, cx| {
                                editor.mark_content_saved(cx);
                            });
                            this.editing = false;
                        }
                        Err(e) => {
                            tracing::error!("create canvas failed: {e}");
                            this.error = Some("Failed to save canvas".into());
                        }
                    }
                    cx.notify();
                });
            }));
            return;
        }

        let canvas_id = self.canvas_id.to_string();
        let is_default = self.is_default;
        let task = CanvasStore::global(cx).update(cx, |store, cx| {
            store.save(&canvas_id, title.clone(), content.clone(), is_default, cx)
        });
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(_) => {
                        this.original_title = title.into();
                        this.original_content = content.clone().into();
                        this.loaded_content = content.into();
                        this.content_editor.update(cx, |editor, cx| {
                            editor.mark_content_saved(cx);
                        });
                        this.editing = false;
                    }
                    Err(e) => {
                        tracing::error!("save canvas failed: {e}");
                        this.error = Some("Failed to save canvas".into());
                    }
                }
                cx.notify();
            });
        }));
    }
}

impl gpui::Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.flush_pending_detail(window, cx);
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let tokens = &theme.tokens;
        let toolbar_bg = tokens.bg_theme_contexify;

        if let Some(err) = &self.error {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_lg()
                        .text_color(tokens.text_theme_message)
                        .child(mezon_i18n::t(&locale, "canvas.error.title")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.text_secondary)
                        .child(err.clone()),
                )
                .into_any_element();
        }

        if self.loading {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Spinner::new().with_size(Size::Small))
                .child(
                    div()
                        .mt_2()
                        .text_sm()
                        .text_color(tokens.text_secondary)
                        .child(mezon_i18n::t(&locale, "common.canvas.loadingCanvas")),
                )
                .into_any_element();
        }

        let dirty = self.has_changes(cx);
        let can_edit = self.can_edit(cx);
        let editing = self.editing && can_edit;
        let read_only = !editing;
        self.content_editor.update(cx, |editor, cx| {
            editor.set_read_only(read_only, cx);
            editor.set_page_scroll(true, self.content_scroll.clone(), cx);
        });
        let show_save_bar = can_edit && editing && dirty;
        let save_label = if self.saving {
            mezon_i18n::t(&locale, "canvas.actions.saving")
        } else {
            mezon_i18n::t(&locale, "canvas.actions.save")
        };

        let top_bar = h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .justify_end()
            .gap_2()
            .px_4()
            .py_2()
            .child(
                Button::new("canvas-back")
                    .label(mezon_i18n::t(&locale, "common.close"))
                    .with_size(Size::Small)
                    .icon(Icon::new(IconName::Close).size_4())
                    .on_click({
                        let clan_id = self.clan_id;
                        let channel_id = self.channel_id;
                        move |_: &ClickEvent, _window, cx| {
                            navigate_to_channel(clan_id, channel_id, cx);
                        }
                    }),
            )
            .when(can_edit, |el| {
                el.child(
                    Button::new("canvas-toggle-edit")
                        .label(if editing { "View" } else { "Edit" })
                        .with_size(Size::Small)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if !this.can_edit(cx) {
                                return;
                            }
                            let entering_edit = !this.editing;
                            this.editing = !this.editing;
                            if entering_edit {
                                let focus_content = !this.original_title.trim().is_empty();
                                if focus_content {
                                    this.content_editor.update(cx, |editor, cx| {
                                        editor.focus(window, cx);
                                    });
                                } else {
                                    this.title_input.update(cx, |input, cx| {
                                        input.focus(window, cx);
                                    });
                                }
                            }
                            cx.notify();
                        })),
                )
            });

        let save_bar = show_save_bar.then(|| {
            h_flex()
                .gap(px(10.))
                .px_3()
                .py_2()
                .rounded(px(10.))
                .bg(toolbar_bg)
                .border_1()
                .border_color(Hsla {
                    h: 0.,
                    s: 0.,
                    l: 1.,
                    a: 0.08,
                })
                .shadow_lg()
                .child(
                    Button::new("canvas-discard")
                        .label(mezon_i18n::t(&locale, "canvas.actions.discardChanges"))
                        .with_size(Size::Small)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            let title = this.original_title.to_string();
                            this.title_input.update(cx, |input, cx| {
                                input.set_value(title, window, cx);
                            });
                            this.content_editor.update(cx, |editor, cx| {
                                editor.set_doc(this.original_content.as_ref(), cx);
                            });
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("canvas-save")
                        .label(save_label)
                        .primary()
                        .with_size(Size::Small)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(cx))),
                )
                .into_any_element()
        });

        let title_el = div()
            .w_full()
            .pt(px(25.))
            .pb_2()
            .text_size(px(28.))
            .font_weight(FontWeight::BOLD)
            .text_color(tokens.text_theme_message)
            .line_height(px(34.))
            .child(
                div()
                    .relative()
                    .w_full()
                    .when(editing, |el| {
                        el.child(
                            Input::new(&self.title_input).text_color(tokens.text_theme_message),
                        )
                    })
                    .when(!editing, |el| {
                        let title = self.title_input.read(cx).value();
                        let display = if title.is_empty() {
                            mezon_i18n::t(&locale, "common.canvas.untitled").to_string()
                        } else {
                            title.to_string()
                        };
                        el.min_h(px(34.)).child(display)
                    }),
            )
            .into_any_element();

        let body: gpui::AnyElement = if editing {
            div()
                .id("canvas-body")
                .w_full()
                .min_w_0()
                .py_2()
                .text_size(CANVAS_BODY_FONT_SIZE)
                .line_height(CANVAS_BODY_LINE_HEIGHT)
                .child(CanvasEditor::new(&self.content_editor))
                .into_any_element()
        } else {
            if self.view_doc_source.as_ref() != self.loaded_content.as_ref() {
                self.view_doc = parse_tiptap_doc(self.loaded_content.as_ref());
                self.view_doc_source = self.loaded_content.clone();
            }
            div()
                .id("canvas-body")
                .w_full()
                .min_w_0()
                .py_2()
                .when_some(self.view_doc.clone(), |el, doc| {
                    el.child(render_tiptap_node(doc, theme.as_ref(), cx))
                })
                .into_any_element()
        };

        let scroll_content = canvas_scroll_viewport(
            self.content_scroll.clone(),
            v_flex()
                .w_full()
                .pb(if show_save_bar { px(88.) } else { px(24.) })
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px(CANVAS_CONTENT_HORIZONTAL_PADDING)
                        .child(v_flex().w_full().min_w_0().child(title_el).child(body)),
                )
                .into_any_element(),
            window,
            cx,
        );

        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(theme.bg_primary)
            .child(top_bar)
            .child(scroll_content)
            .when_some(save_bar, |el, bar| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(24.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .occlude()
                        .child(bar),
                )
            })
            .into_any_element()
    }
}

fn canvas_scroll_viewport(
    scroll: ScrollHandle,
    content: gpui::AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyElement {
    div()
        .id("canvas-main-scroll")
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .id("canvas-main-scroll-inner")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .child(content),
        )
        .custom_scrollbars(
            Scrollbars::always_visible(ScrollAxes::Vertical).tracked_scroll_handle(&scroll),
            window,
            cx,
        )
        .into_any_element()
}

pub fn canvas_can_edit(creator_id: UserId, current_user: Option<UserId>) -> bool {
    creator_id.0 == 0 || current_user == Some(creator_id)
}

pub fn canvas_can_delete(
    canvas_creator_id: UserId,
    channel_creator_id: Option<UserId>,
    current_user: Option<UserId>,
) -> bool {
    let Some(uid) = current_user else {
        return false;
    };
    if canvas_creator_id.0 == 0 {
        return true;
    }
    canvas_creator_id == uid || channel_creator_id == Some(uid)
}

#[derive(Debug, Clone, Deserialize)]
pub struct TipTapNode {
    #[serde(rename = "type")]
    pub kind: String,
    pub attrs: Option<Value>,
    pub content: Option<Vec<TipTapNode>>,
    pub text: Option<String>,
    pub marks: Option<Vec<TipTapMark>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TipTapMark {
    #[serde(rename = "type")]
    pub kind: String,
    pub attrs: Option<Value>,
}

pub fn parse_tiptap_doc(raw: &str) -> Option<TipTapNode> {
    if raw.trim().is_empty() {
        return None;
    }
    if let Ok(doc) = serde_json::from_str::<TipTapNode>(raw)
        && doc.kind == "doc"
    {
        return Some(doc);
    }
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return None;
    };
    if is_quill_delta(&value) {
        return serde_json::from_value(quill_delta_to_tiptap_json(&value)).ok();
    }
    serde_json::from_str::<TipTapNode>(raw).ok()
}

pub fn is_tiptap_content_empty(raw: &str) -> bool {
    if raw.trim().is_empty() {
        return true;
    }
    let Some(doc) = parse_tiptap_doc(raw) else {
        return raw.is_empty();
    };
    if doc.kind != "doc" {
        return false;
    }
    let Some(content) = &doc.content else {
        return true;
    };
    content.is_empty()
        || (content.len() == 1
            && content[0].kind == "paragraph"
            && content[0]
                .content
                .as_ref()
                .map(|c| c.is_empty())
                .unwrap_or(true))
}

#[cfg(test)]
fn tip_tap_to_plain_text(raw: &str) -> String {
    let Some(doc) = parse_tiptap_doc(raw) else {
        return raw.to_string();
    };
    let mut out = String::new();
    collect_plain(&doc, &mut out);
    out
}

#[cfg(test)]
fn collect_plain(node: &TipTapNode, out: &mut String) {
    if node.kind == "text" {
        if let Some(text) = &node.text {
            out.push_str(text);
        }
        return;
    }
    if let Some(children) = &node.content {
        for (i, child) in children.iter().enumerate() {
            if i > 0 && is_block(&child.kind) && !out.ends_with('\n') {
                out.push('\n');
            }
            collect_plain(child, out);
        }
    }
}

#[cfg(test)]
fn is_block(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph"
            | "heading"
            | "bulletList"
            | "orderedList"
            | "listItem"
            | "taskList"
            | "taskItem"
            | "blockquote"
            | "codeBlock"
            | "image"
    )
}

#[cfg(test)]
fn plain_text_to_tiptap(text: &str) -> String {
    if text.is_empty() {
        return r#"{"type":"doc","content":[{"type":"paragraph"}]}"#.to_string();
    }
    let paragraphs: Vec<Value> = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                serde_json::json!({ "type": "paragraph" })
            } else {
                serde_json::json!({
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": line }]
                })
            }
        })
        .collect();
    serde_json::json!({ "type": "doc", "content": paragraphs }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quill_delta_canvas_from_electron() {
        let raw = r#"{"ops":[{"insert":"Noti when only mentions die\nCheck remember channel"}]}"#;
        let doc = parse_tiptap_doc(raw).expect("parse quill");
        assert_eq!(doc.kind, "doc");
        let content = doc.content.as_ref().expect("content");
        assert!(!content.is_empty());
        assert_eq!(content[0].kind, "paragraph");
    }

    #[test]
    fn parse_simple_doc() {
        let raw = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Hello","marks":[{"type":"bold"}]}]},{"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Title"}]}]}"#;
        let doc = parse_tiptap_doc(raw).expect("parse");
        assert_eq!(doc.kind, "doc");
        assert_eq!(doc.content.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn plain_roundtrip_basic() {
        let text = "Hello\nWorld";
        let json = plain_text_to_tiptap(text);
        let back = tip_tap_to_plain_text(&json);
        assert_eq!(back, text);
    }

    #[test]
    fn plain_roundtrip_has_no_trailing_newline() {
        let raw = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Hello"}]}]}"#;
        let back = tip_tap_to_plain_text(raw);
        assert_eq!(back, "Hello");
        assert!(!back.ends_with('\n'));
    }

    #[test]
    fn empty_plain_to_tiptap() {
        let json = plain_text_to_tiptap("");
        assert!(json.contains("paragraph"));
        assert_eq!(tip_tap_to_plain_text(&json), "");
    }

    #[test]
    fn is_tiptap_content_empty_detects_blank_doc() {
        assert!(is_tiptap_content_empty(
            r#"{"type":"doc","content":[{"type":"paragraph"}]}"#
        ));
        assert!(!is_tiptap_content_empty(
            r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Hi"}]}]}"#
        ));
    }

    #[test]
    fn canvas_edit_permission_matches_react() {
        let creator = UserId(42);
        assert!(canvas_can_edit(creator, Some(creator)));
        assert!(canvas_can_edit(UserId(0), Some(UserId(99))));
        assert!(!canvas_can_edit(creator, Some(UserId(99))));
        assert!(!canvas_can_edit(creator, None));
    }

    #[test]
    fn canvas_delete_permission_matches_react() {
        let creator = UserId(42);
        let channel_creator = UserId(7);
        assert!(canvas_can_delete(
            creator,
            Some(channel_creator),
            Some(creator)
        ));
        assert!(canvas_can_delete(
            creator,
            Some(channel_creator),
            Some(channel_creator)
        ));
        assert!(canvas_can_delete(
            UserId(0),
            Some(channel_creator),
            Some(UserId(99))
        ));
        assert!(!canvas_can_delete(
            creator,
            Some(channel_creator),
            Some(UserId(99))
        ));
        assert!(!canvas_can_delete(creator, Some(channel_creator), None));
    }
}
