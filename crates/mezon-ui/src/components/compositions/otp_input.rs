//! OtpInput — 6-digit OTP input with auto-advance, paste support, and backspace navigation.
//!
//! A reusable composition that wraps 6 `FormField` widgets. Owns the input behavior
//! (auto-advance, paste handling, backspace navigation) but NOT the OTP flow
//! state (`otp_req_id`, API calls) — that stays with the parent view.

use std::sync::Arc;

use gpui::{App, Context, Entity, Window, div, prelude::*};

use crate::components::compositions::FormField;
use crate::components::{KeyHandler, TextChangeHandler};

type OnComplete = Arc<dyn Fn(String, &mut Window, &mut App) + Send + Sync>;

pub struct OtpInput {
    fields: Vec<Entity<FormField>>,
    on_complete: Option<OnComplete>,
}

impl OtpInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let fields: Vec<Entity<FormField>> = (0..6)
            .map(|i| cx.new(move |cx| FormField::new(cx, format!("{}", i))))
            .collect();

        let entity = cx.entity().clone();

        // Wire on_change for auto-advance and paste handling.
        for (i, field) in fields.iter().enumerate() {
            let entity_clone = entity.clone();
            let on_change: TextChangeHandler =
                Arc::new(move |val: &str, window: &mut Window, cx: &mut App| {
                    entity_clone.update(cx, |this: &mut OtpInput, cx| {
                        if val.len() == 1 && i < 5 {
                            // Auto-advance: focus next field
                            let next_handle = this.fields[i + 1]
                                .read(cx)
                                .input_entity()
                                .read(cx)
                                .focus_handle
                                .clone();
                            window.focus(&next_handle, cx);
                        } else if val.chars().count() > 1 {
                            // Paste handling: extract digits and distribute
                            let digits: Vec<char> =
                                val.chars().filter(|c| c.is_ascii_digit()).collect();
                            if !digits.is_empty() {
                                for (j, digit) in digits.iter().enumerate() {
                                    let target_idx = i + j;
                                    if target_idx >= 6 {
                                        break;
                                    }
                                    this.fields[target_idx].update(cx, |field, cx| {
                                        field.input_entity().update(cx, |input, cx| {
                                            input.set_value(digit.to_string(), cx);
                                        });
                                    });
                                }
                                // Focus the field after the last pasted digit
                                let focus_idx = (i + digits.len()).min(5);
                                let focus_handle = this.fields[focus_idx]
                                    .read(cx)
                                    .input_entity()
                                    .read(cx)
                                    .focus_handle
                                    .clone();
                                window.focus(&focus_handle, cx);

                                // Check if all 6 fields are filled
                                let mut all_filled = true;
                                for k in 0..6 {
                                    if this.fields[k].read(cx).value(cx).chars().count() != 1 {
                                        all_filled = false;
                                        break;
                                    }
                                }
                                if all_filled {
                                    if let Some(ref cb) = this.on_complete {
                                        let code: String = this.code(cx);
                                        cb(code, window, cx);
                                    }
                                }
                            }
                        }
                    });
                });
            field.update(cx, |f, cx| f.set_on_change(on_change, cx));
        }

        // Wire on_key for backspace navigation.
        for (i, field) in fields.iter().enumerate() {
            let entity_clone = entity.clone();
            field.update(cx, |f, cx| {
                let input_ent = f.input_entity().clone();
                input_ent.update(cx, |input, _cx| {
                    let on_key: KeyHandler = Arc::new(
                        move |keystroke: &gpui::Keystroke, window: &mut Window, cx: &mut App| {
                            if keystroke.key == "backspace" {
                                entity_clone.update(cx, |this: &mut OtpInput, cx| {
                                    let current_val = this.fields[i].read(cx).value(cx);
                                    if current_val.is_empty() && i > 0 {
                                        let prev_handle = this.fields[i - 1]
                                            .read(cx)
                                            .input_entity()
                                            .read(cx)
                                            .focus_handle
                                            .clone();
                                        window.focus(&prev_handle, cx);
                                    }
                                });
                            }
                        },
                    );
                    input.set_on_key(on_key, _cx);
                });
            });
        }

        Self {
            fields,
            on_complete: None,
        }
    }

    /// Read the current 6-digit code from the input fields.
    pub fn code(&self, cx: &App) -> String {
        self.fields.iter().map(|f| f.read(cx).value(cx)).collect()
    }

    /// Reset all fields (used on authentication failure).
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        for field in &self.fields {
            field.update(cx, |f, cx| {
                f.set_error(Some(String::new()), cx);
            });
        }
    }

    /// Set the callback to invoke when a paste fills all 6 digits (auto-submit).
    pub fn set_on_complete(&mut self, cb: OnComplete) {
        self.on_complete = Some(cb);
    }
}

impl Render for OtpInput {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut otp_row = div().flex().flex_row().gap_2().justify_center();
        for field in &self.fields {
            otp_row = otp_row.child(div().w(gpui::px(44.0)).child(field.clone()));
        }
        otp_row
    }
}
