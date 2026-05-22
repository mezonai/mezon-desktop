use crate::theme::Theme;
use gpui::{
    App, ClickEvent, Context, Entity, FontWeight, Subscription, Task, WeakEntity, Window, div,
    prelude::*, px,
};
use gpui_component::{
    h_flex,
    label::Label,
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use mezon_native::audio::{
    AudioDeviceInfo, MicCapture, enumerate_input_devices, enumerate_output_devices,
};
use mezon_store::Settings;

pub struct VoicePage {
    settings: Entity<Settings>,
    mic_slider: Entity<SliderState>,
    speaker_slider: Entity<SliderState>,
    _subs: Vec<Subscription>,
    input_devices: Vec<AudioDeviceInfo>,
    output_devices: Vec<AudioDeviceInfo>,
    selected_input_id: Option<String>,
    selected_output_id: Option<String>,
    input_dropdown_open: bool,
    output_dropdown_open: bool,
    is_testing: bool,
    mic_level: f32,
    error_text: Option<String>,
    mic_capture: Option<MicCapture>,
    _test_task: Option<Task<()>>,
}

impl VoicePage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let mic_vol = settings.read(cx).mic_volume;
        let speaker_vol = settings.read(cx).speaker_volume;

        let mic_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.05)
                .default_value(mic_vol)
        });

        let speaker_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.05)
                .default_value(speaker_vol)
        });

        let mut subs = Vec::new();

        let mic_settings = settings.clone();
        subs.push(cx.subscribe(
            &mic_slider,
            move |_this, _slider: Entity<SliderState>, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event;
                mic_settings.update(cx, |s, _| {
                    s.mic_volume = value.end();
                    s.save_sync();
                });
            },
        ));

        let speaker_settings = settings.clone();
        subs.push(cx.subscribe(
            &speaker_slider,
            move |_this, _slider: Entity<SliderState>, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event;
                speaker_settings.update(cx, |s, _| {
                    s.speaker_volume = value.end();
                    s.save_sync();
                });
            },
        ));

        let saved_input = settings.read(cx).input_device_id.clone();
        let saved_output = settings.read(cx).output_device_id.clone();

        let input_devices = enumerate_input_devices();
        let output_devices = enumerate_output_devices();

        let selected_input_id = if input_devices
            .iter()
            .any(|d| Some(d.id.as_str()) == saved_input.as_deref())
        {
            saved_input
        } else {
            None
        };

        let selected_output_id = if output_devices
            .iter()
            .any(|d| Some(d.id.as_str()) == saved_output.as_deref())
        {
            saved_output
        } else {
            None
        };

        Self {
            settings,
            mic_slider,
            speaker_slider,
            _subs: subs,
            input_devices,
            output_devices,
            selected_input_id,
            selected_output_id,
            input_dropdown_open: false,
            output_dropdown_open: false,
            is_testing: false,
            mic_level: 0.0,
            error_text: None,
            mic_capture: None,
            _test_task: None,
        }
    }
}

impl Render for VoicePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        let mic_vol_pct = (self.mic_slider.read(cx).value().end() * 100.0) as u32;
        let speaker_vol_pct = (self.speaker_slider.read(cx).value().end() * 100.0) as u32;

        let num_bars: usize = 84;

        let this_handle = cx.entity();
        let settings = self.settings.clone();

        let input_devices = self.input_devices.clone();
        let output_devices = self.output_devices.clone();
        let selected_input_id = self.selected_input_id.clone();
        let selected_output_id = self.selected_output_id.clone();

        let is_testing = self.is_testing;
        let mic_level = self.mic_level;
        v_flex()
            .gap_6()
            .child(
                Label::new("Voice & Video")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_4()
                    .rounded_lg()
                    .bg(theme.bg_secondary)
                    .border_1()
                    .border_color(theme.border)
                    // 2-column grid
                    .child(
                        h_flex()
                            .gap_8()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_2()
                                    // Input Device
                                    .child(Label::new("Input Device").text_color(theme.text_primary))
                                    .child({
                                        Self::render_selector(
                                            &input_devices,
                                            &selected_input_id,
                                            self.input_dropdown_open,
                                            "No input devices",
                                            &theme,
                                            this_handle.clone(),
                                            settings.clone(),
                                            true,
                                        )
                                    })
                                    // Mic Volume
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .child(Label::new("Mic Volume").text_color(theme.text_primary))
                                                    .child(
                                                        Label::new(format!("{mic_vol_pct}%"))
                                                            .text_sm()
                                                            .text_color(theme.text_muted),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .px_3()
                                                    .py_2()
                                                    .rounded_lg()
                                                    .bg(theme.bg_primary)
                                                    .child(Slider::new(&self.mic_slider).horizontal()),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_2()
                                    // Output Device
                                    .child(Label::new("Output Device").text_color(theme.text_primary))
                                    .child({
                                        Self::render_selector(
                                            &output_devices,
                                            &selected_output_id,
                                            self.output_dropdown_open,
                                            "No output devices",
                                            &theme,
                                            this_handle.clone(),
                                            settings.clone(),
                                            false,
                                        )
                                    })
                                    // Speaker Volume
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .child(Label::new("Speaker Volume").text_color(theme.text_primary))
                                                    .child(
                                                        Label::new(format!("{speaker_vol_pct}%"))
                                                            .text_sm()
                                                            .text_color(theme.text_muted),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .px_3()
                                                    .py_2()
                                                    .rounded_lg()
                                                    .bg(theme.bg_primary)
                                                    .child(Slider::new(&self.speaker_slider).horizontal()),
                                            ),
                                    ),
                            ),
                    )
                    // Mic Test section
                    .child(
                        div()
                            .mt_4()
                            .pt_4()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .gap_4()
                                            .items_center()
                                            .child(
                                                div()
                                                    .id("mic-test-btn")
                                                    .flex()
                                                    .items_center()
                                                    .px_4()
                                                    .py_1()
                                                    .rounded_lg()
                                                    .bg(if is_testing { theme.status_dnd } else { theme.brand })
                                                    .cursor_pointer()
                                                    .child(
                                                        Label::new(if is_testing { "Stop" } else { "Let's Check" })
                                                            .text_sm()
                                                            .text_color(theme.text_primary),
                                                    )
                                                    .on_click({
                                                        let handle = this_handle.clone();
                                                        move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
                                                            handle.update(cx, |this, cx| {
                                                                this.toggle_mic_test(cx);
                                                            });
                                                        }
                                                    }),
                                            )
                                            .child(
                                                Label::new("Mic Test")
                                                    .text_sm()
                                                    .text_color(theme.text_muted),
                                            ),
                                    )
                                    // Level meter
                                    .when(is_testing, |el| {
                                        let level = mic_level;
                                        el.child(
                                            h_flex()
                                                .gap_1()
                                                .children((0..num_bars).map(move |i| {
                                                    let active_pct = (level * 2.2).clamp(0.0, 1.0);
                                                    let threshold = (i as f32) / (num_bars as f32) * 72.0 / 84.0;
                                                    let active = threshold <= active_pct;
                                                    let pct = i as f32 / num_bars as f32;
                                                    let color = if pct < 0.5 {
                                                        rgba(80, 200, 80, 1.0)
                                                    } else if pct < 0.75 {
                                                        rgba(200, 200, 80, 1.0)
                                                    } else {
                                                        rgba(200, 80, 80, 1.0)
                                                    };
                                                    div()
                                                        .w(px(3.0))
                                                        .h(px(16.0))
                                                        .rounded_sm()
                                                        .bg(if active { color } else { theme.bg_tertiary })
                                                })),
                                        )
                                    })
                                    .when(is_testing, |el| {
                                        el.child(
                                            h_flex()
                                                .gap_2()
                                                .child(Label::new("Silent").text_xs().text_color(theme.text_muted))
                                                .child(Label::new("Loud").text_xs().text_color(theme.text_muted)),
                                        )
                                    })
                                    // Error text
                                    .when_some(self.error_text.clone(), |el, err| {
                                        el.child(
                                            Label::new(err)
                                                .text_sm()
                                                .text_color(theme.status_dnd),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                Label::new("Audio input/output requires native integration.")
                    .text_sm()
                    .text_color(theme.text_muted),
            )
    }
}

impl VoicePage {
    #[allow(clippy::too_many_arguments)]
    fn render_selector(
        devices: &[AudioDeviceInfo],
        selected_id: &Option<String>,
        is_open: bool,
        empty_label: &str,
        theme: &Theme,
        entity: Entity<VoicePage>,
        settings: Entity<Settings>,
        is_input: bool,
    ) -> impl IntoElement {
        let is_empty = devices.is_empty();
        let selected_name = selected_id
            .as_ref()
            .and_then(|id| devices.iter().find(|d| d.id == *id))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| empty_label.to_string());

        div()
            .relative()
            .child(
                div()
                    .id(if is_input {
                        "input-device-trigger"
                    } else {
                        "output-device-trigger"
                    })
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(theme.bg_primary)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .child(Label::new(selected_name).text_sm().text_color(if is_empty {
                        theme.text_muted
                    } else {
                        theme.text_primary
                    }))
                    .child(div().text_color(theme.text_muted).child(if is_open {
                        "▲"
                    } else {
                        "▼"
                    }))
                    .when(!is_empty, |el| {
                        let entity = entity.clone();
                        el.on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            entity.update(cx, |this, _| {
                                if is_input {
                                    this.toggle_dropdown(true);
                                } else {
                                    this.toggle_dropdown(false);
                                }
                            });
                        })
                    }),
            )
            .when(is_open && !is_empty, |el| {
                el.child(
                    div()
                        .flex_col()
                        .mt_1()
                        .rounded_lg()
                        .bg(theme.bg_primary)
                        .border_1()
                        .border_color(theme.border)
                        .overflow_hidden()
                        .children(devices.iter().map(move |device| {
                            let device_id = device.id.clone();
                            let device_name = device.name.clone();
                            let is_selected = selected_id.as_deref() == Some(&device_id);
                            let entity = entity.clone();
                            let settings = settings.clone();
                            div()
                                .id(device_id.clone())
                                .flex()
                                .items_center()
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .when(is_selected, |el| el.bg(theme.bg_tertiary))
                                .when(!is_selected, |el| el.hover(|el| el.bg(theme.bg_tertiary)))
                                .child(
                                    Label::new(device_name)
                                        .text_sm()
                                        .text_color(theme.text_primary),
                                )
                                .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                    entity.update(cx, |this, _cx| {
                                        if is_input {
                                            this.select_input_device(
                                                device_id.clone(),
                                                settings.clone(),
                                                _cx,
                                            );
                                        } else {
                                            this.select_output_device(
                                                device_id.clone(),
                                                settings.clone(),
                                                _cx,
                                            );
                                        }
                                    });
                                })
                        })),
                )
            })
    }

    fn toggle_dropdown(&mut self, is_input: bool) {
        if is_input {
            self.input_dropdown_open = !self.input_dropdown_open;
            self.output_dropdown_open = false;
        } else {
            self.output_dropdown_open = !self.output_dropdown_open;
            self.input_dropdown_open = false;
        }
    }

    fn select_input_device(
        &mut self,
        id: String,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) {
        self.selected_input_id = Some(id.clone());
        self.input_dropdown_open = false;
        settings.update(cx, |s, _| {
            s.input_device_id = Some(id);
            s.save_sync();
        });
    }

    fn select_output_device(
        &mut self,
        id: String,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) {
        self.selected_output_id = Some(id.clone());
        self.output_dropdown_open = false;
        settings.update(cx, |s, _| {
            s.output_device_id = Some(id);
            s.save_sync();
        });
    }

    fn toggle_mic_test(&mut self, cx: &mut Context<Self>) {
        if self.is_testing {
            self.mic_capture = None;
            self._test_task = None;
            self.is_testing = false;
            self.mic_level = 0.0;
            self.error_text = None;
            cx.notify();
        } else {
            let device_id = match &self.selected_input_id {
                Some(id) => id.clone(),
                None => {
                    self.error_text = Some("No input device selected.".to_string());
                    cx.notify();
                    return;
                }
            };

            let (tx, rx) = flume::unbounded();

            match MicCapture::start(&device_id, tx) {
                Ok(capture) => {
                    self.mic_capture = Some(capture);
                    self.is_testing = true;
                    self.error_text = None;
                    self.mic_level = 0.0;

                    let this = cx.weak_entity();
                    let task = cx.spawn(async move |_handle: WeakEntity<VoicePage>, cx| {
                        while let Ok(level) = rx.recv_async().await {
                            if this
                                .update(cx, |this, cx| {
                                    this.mic_level = level;
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                    self._test_task = Some(task);
                    cx.notify();
                }
                Err(e) => {
                    self.error_text = Some(e.to_string());
                    cx.notify();
                }
            }
        }
    }
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> gpui::Rgba {
    gpui::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}
