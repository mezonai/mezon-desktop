use std::time::Duration;

use gpui::Context;

const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const BLINK_PAUSE: Duration = Duration::from_millis(500);

pub trait HasCaretBlink: 'static {
    fn caret_blink_mut(&mut self) -> &mut CaretBlink;
}

pub struct CaretBlink {
    blink_epoch: usize,
    visible: bool,
    enabled: bool,
}

impl Default for CaretBlink {
    fn default() -> Self {
        Self::new()
    }
}

impl CaretBlink {
    pub fn new() -> Self {
        Self {
            blink_epoch: 0,
            visible: true,
            enabled: false,
        }
    }

    pub fn visible(&self) -> bool {
        self.enabled && self.visible
    }

    pub fn sync_focused<T: HasCaretBlink>(&mut self, cx: &mut Context<T>) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        self.visible = true;
        cx.notify();
        self.schedule_blink(cx);
    }

    pub fn sync_blurred<T: HasCaretBlink>(&mut self, cx: &mut Context<T>) {
        if !self.enabled {
            return;
        }
        self.enabled = false;
        self.visible = false;
        self.bump_epoch();
        cx.notify();
    }

    pub fn pause_blinking<T: HasCaretBlink>(&mut self, cx: &mut Context<T>) {
        if !self.enabled {
            return;
        }
        self.visible = true;
        cx.notify();

        let epoch = self.bump_epoch();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLINK_PAUSE).await;
            this.update(cx, |this, cx| {
                let blink = this.caret_blink_mut();
                if blink.enabled && blink.blink_epoch == epoch {
                    blink.schedule_blink(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn schedule_blink<T: HasCaretBlink>(&mut self, cx: &mut Context<T>) {
        let epoch = self.bump_epoch();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLINK_INTERVAL).await;
            this.update(cx, |this, cx| this.caret_blink_mut().blink_tick(epoch, cx))
                .ok();
        })
        .detach();
    }

    fn blink_tick<T: HasCaretBlink>(&mut self, epoch: usize, cx: &mut Context<T>) {
        if !self.enabled || epoch != self.blink_epoch {
            return;
        }
        self.visible = !self.visible;
        cx.notify();
        self.schedule_blink(cx);
    }

    fn bump_epoch(&mut self) -> usize {
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        self.blink_epoch
    }
}
