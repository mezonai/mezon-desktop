use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, Context, Task};
use mezon_voice::VideoFrameStore;

use crate::screen_draw::{DrawTool, ScreenDrawState};

const VANISH_TICK: Duration = Duration::from_millis(33);

pub struct DesktopDraw {
    state: ScreenDrawState,
    canvas: (f32, f32),
    window_share: bool,
    minimized: bool,
    frame_store: Option<Arc<VideoFrameStore>>,
    windows: Vec<AnyWindowHandle>,
    vanish_ticking: bool,
    _vanish_tick: Option<Task<()>>,
}

impl DesktopDraw {
    pub fn new(window_share: bool, frame_store: Option<Arc<VideoFrameStore>>) -> Self {
        let mut state = ScreenDrawState::default();
        state.set_active(true);
        Self {
            state,
            canvas: (0.0, 0.0),
            window_share,
            minimized: false,
            frame_store,
            windows: Vec::new(),
            vanish_ticking: false,
            _vanish_tick: None,
        }
    }

    pub fn state(&self) -> &ScreenDrawState {
        &self.state
    }

    pub fn is_window_share(&self) -> bool {
        self.window_share
    }

    pub fn is_minimized(&self) -> bool {
        self.minimized
    }

    pub fn in_drawing_mode(&self) -> bool {
        !self.minimized && self.state.active_tool != DrawTool::Pointer
    }

    pub fn set_minimized(&mut self, minimized: bool, cx: &mut Context<Self>) {
        if self.minimized == minimized {
            return;
        }
        self.minimized = minimized;
        if minimized {
            self.state.commit_editing_text();
            self.state.cancel_stroke();
            self.state.selected = None;
            self.state.selection.clear();
        }
        self.sync(cx);
        cx.notify();
    }

    pub fn edit<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ScreenDrawState) -> R,
    ) -> R {
        let result = f(&mut self.state);
        self.sync(cx);
        cx.notify();
        result
    }

    pub fn set_canvas(&mut self, width: f32, height: f32, cx: &mut Context<Self>) {
        if self.canvas == (width, height) {
            return;
        }
        self.canvas = (width, height);
        self.sync(cx);
    }

    pub fn attach_window(&mut self, handle: AnyWindowHandle) {
        self.windows.push(handle);
    }

    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        for handle in self.windows.drain(..) {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.state = ScreenDrawState::default();
        self._vanish_tick = None;
        self.vanish_ticking = false;
        self.publish_scene();
        cx.notify();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        self.publish_scene();
        if !self.state.vanishing.is_empty() && !self.vanish_ticking {
            self.vanish_ticking = true;
            self._vanish_tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(VANISH_TICK).await;
                    let ticking = this
                        .update(cx, |this, cx| {
                            this.state.prune_vanishing(Instant::now());
                            this.vanish_ticking = !this.state.vanishing.is_empty();
                            this.publish_scene();
                            cx.notify();
                            this.vanish_ticking
                        })
                        .unwrap_or(false);
                    if !ticking {
                        break;
                    }
                }
            }));
        }
    }

    fn publish_scene(&self) {
        if !self.window_share {
            return;
        }
        let Some(frame_store) = &self.frame_store else {
            return;
        };
        let scene = self.state.has_content().then(|| {
            self.state
                .annotation_scene(self.canvas.0, self.canvas.1, Instant::now())
        });
        frame_store.screen_annotations.set_scene(scene);
    }
}
