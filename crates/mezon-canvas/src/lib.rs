mod editor;
mod image;
mod navigation;
mod popover;
mod quill;
mod view;

pub use editor::{CanvasEditor, CanvasEditorState, init as init_editor};
pub use image::reset_canvas_image_caches;
pub use navigation::{
    CanvasNavigationHooks, CanvasRoute, confirm_delete_canvas, navigate_to_canvas,
    navigate_to_channel, set_navigation,
};
pub use popover::{CanvasPopoverPanel, canvas_popover_on_open};
pub use view::{
    CanvasView, TipTapMark, TipTapNode, canvas_can_delete, canvas_can_edit,
    is_tiptap_content_empty, parse_tiptap_doc,
};

pub fn init(cx: &mut gpui::App) {
    init_editor(cx);
}
