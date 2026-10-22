use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, DragMoveEvent, Empty, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, Hsla, MouseButton, MouseDownEvent, PathPromptOptions, Pixels,
    SharedString, Subscription, Task, Window, canvas, div, prelude::*, px,
};
use mezon_audio::{
    AudioPlayer, DecodedPcm, estimated_mono_wav_bytes, process_pcm, process_pcm_to_wav,
};
use mezon_store::{
    ClanId, EMOTICON_SHORTNAME_MAX, MAX_SOUND_BYTES, Settings, StickerStore, VoiceStore,
    is_valid_emoticon_shortname, validate_sound_file,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Slider, SliderEvent,
    SliderState, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const FORM_CONTROL_H: f32 = 34.0;
const PREVIEW_TICK: Duration = Duration::from_millis(250);
const WAVEFORM_BARS: usize = 144;
const MIN_TRIM_PERCENT: f32 = 1.0;
const DEFAULT_VOLUME_PERCENT: f32 = 80.0;
const MAX_VOLUME_PERCENT: f32 = 100.0;
const UNITY_VOLUME_PERCENT: f32 = 100.0;
const MAX_TRIM_SECS: f64 = 10.0;

#[derive(Clone)]
struct TrimThumb((EntityId, bool));

impl Render for TrimThumb {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

enum TrimRangeEvent {
    Change,
}

struct TrimRangeState {
    start: f32,
    end: f32,
    bounds: Bounds<Pixels>,
}

impl TrimRangeState {
    fn new() -> Self {
        Self {
            start: 0.0,
            end: 1.0,
            bounds: Bounds::default(),
        }
    }

    fn value(&self) -> (f32, f32) {
        (self.start * 100.0, self.end * 100.0)
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.start = 0.0;
        self.end = 1.0;
        cx.emit(TrimRangeEvent::Change);
        cx.notify();
    }

    fn update_from_position(&mut self, position: Pixels, is_start: bool, cx: &mut Context<Self>) {
        let width = self.bounds.size.width;
        if width <= px(0.0) {
            return;
        }
        let percentage = ((position - self.bounds.left()) / width).clamp(0.0, 1.0);
        let min_range = MIN_TRIM_PERCENT / 100.0;
        if is_start {
            self.start = percentage.min(self.end - min_range);
        } else {
            self.end = percentage.max(self.start + min_range);
        }
        cx.emit(TrimRangeEvent::Change);
        cx.notify();
    }
}

impl EventEmitter<TrimRangeEvent> for TrimRangeState {}

impl Render for TrimRangeState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(IntoElement)]
struct WaveformTrimSlider {
    state: Entity<TrimRangeState>,
    disabled: bool,
}

impl WaveformTrimSlider {
    fn new(state: &Entity<TrimRangeState>) -> Self {
        Self {
            state: state.clone(),
            disabled: false,
        }
    }

    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    fn thumb(
        &self,
        position: f32,
        is_start: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let entity_id = self.state.entity_id();
        div()
            .id(("waveform-trim-thumb", is_start as u32))
            .absolute()
            .top_0()
            .left(gpui::relative(position))
            .ml(-px(2.0))
            .w(px(4.0))
            .h_full()
            .rounded_sm()
            .bg(Hsla::from(cx.theme().tokens.text_theme_message))
            .when(!self.disabled, |this| {
                this.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_drag(TrimThumb((entity_id, is_start)), |drag, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| drag.clone())
                    })
                    .on_drag_move(window.listener_for(
                        &self.state,
                        move |state, event: &DragMoveEvent<TrimThumb>, _, cx| {
                            let TrimThumb((id, dragged_start)) = event.drag(cx);
                            if *id == entity_id && *dragged_start == is_start {
                                state.update_from_position(event.event.position.x, is_start, cx);
                            }
                        },
                    ))
            })
    }
}

impl RenderOnce for WaveformTrimSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.read(cx);
        let start = state.start;
        let end = state.end;
        let entity_id = self.state.entity_id();
        div()
            .id(("waveform-trim-slider", entity_id))
            .relative()
            .size_full()
            .when(!self.disabled, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    window.listener_for(
                        &self.state,
                        move |state, event: &MouseDownEvent, _, cx| {
                            let center = (state.start + state.end) / 2.0;
                            let percentage = ((event.position.x - state.bounds.left())
                                / state.bounds.size.width)
                                .clamp(0.0, 1.0);
                            state.update_from_position(event.position.x, percentage < center, cx);
                        },
                    ),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui::relative(start))
                    .bg(Hsla::from(cx.theme().bg_primary).opacity(0.62)),
            )
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui::relative(1.0 - end))
                    .bg(Hsla::from(cx.theme().bg_primary).opacity(0.62)),
            )
            .when(!self.disabled, |this| {
                this.child(self.thumb(start, true, window, cx))
                    .child(self.thumb(end, false, window, cx))
            })
            .child({
                let state = self.state.clone();
                canvas(
                    move |bounds, _, cx| state.update(cx, |state, _| state.bounds = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
    }
}

fn default_voice_shortname() -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("voice_{timestamp}")
}

fn format_mmss(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn audio_time_label(current: f64, duration: f64) -> SharedString {
    if duration <= 0.0 {
        return format_mmss(current).into();
    }
    format!("{} / {}", format_mmss(current), format_mmss(duration)).into()
}

fn audio_needs_processing(start_percent: f32, end_percent: f32, volume_percent: f32) -> bool {
    start_percent > f32::EPSILON
        || end_percent < 100.0 - f32::EPSILON
        || volume_percent < UNITY_VOLUME_PERCENT
}

#[derive(Clone)]
pub struct SoundEditTarget {
    pub id: String,
    pub shortname: String,
    pub source: String,
}

pub enum SoundPickerEvent {
    Saved,
    Cancelled,
}

pub struct SoundPicker {
    clan_id: ClanId,
    editing: Option<SoundEditTarget>,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    name_input: Entity<InputState>,
    picked_path: Option<PathBuf>,
    file_label: SharedString,
    preview_url: Option<SharedString>,
    local_pcm: Option<DecodedPcm>,
    waveform_peaks: Vec<f32>,
    decode_pending: bool,
    decode_failed: bool,
    local_player: Option<AudioPlayer>,
    trim_slider: Entity<TrimRangeState>,
    volume_slider: Entity<SliderState>,
    submitting: bool,
    _name_sub: Subscription,
    _trim_sub: Subscription,
    _volume_sub: Subscription,
    _voice_observe: Subscription,
    _pick_task: Option<Task<()>>,
    _submit_task: Option<Task<()>>,
    _decode_task: Option<Task<()>>,
    _preview_tick: Option<Task<()>>,
}

impl EventEmitter<SoundPickerEvent> for SoundPicker {}

impl Focusable for SoundPicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SoundPicker {
    pub fn new(
        clan_id: ClanId,
        editing: Option<SoundEditTarget>,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clanSoundSetting.modal.placeholder")
                .to_string()
                .into();
        let initial_name = editing
            .as_ref()
            .map(|e| e.shortname.clone())
            .unwrap_or_default();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(FORM_CONTROL_H))
        });
        if !initial_name.is_empty() {
            name_input.update(cx, |state, cx| {
                state.set_value(initial_name, window, cx);
            });
        }
        let name_sub = cx.subscribe(&name_input, |_, _, _: &InputEvent, cx| cx.notify());
        let file_label = editing
            .as_ref()
            .and_then(|e| e.source.split('/').next_back())
            .map(SharedString::from)
            .unwrap_or_else(|| {
                mezon_i18n::t(&locale, "clanSoundSetting.modal.chooseOrDrop")
                    .to_string()
                    .into()
            });
        let preview_url = editing
            .as_ref()
            .map(|e| SharedString::from(e.source.clone()));
        let voice_observe = cx.observe(&VoiceStore::global(cx), |_, _, cx| cx.notify());
        let trim_slider = cx.new(|_| TrimRangeState::new());
        let trim_sub = cx.subscribe(&trim_slider, |this, _, _: &TrimRangeEvent, cx| {
            this.stop_local_preview(cx);
        });
        let volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(MAX_VOLUME_PERCENT)
                .step(1.0)
                .default_value(DEFAULT_VOLUME_PERCENT)
        });
        let volume_sub = cx.subscribe(&volume_slider, |this, _, event: &SliderEvent, cx| {
            let SliderEvent::Change(value) = event;
            if let Some(player) = &this.local_player {
                player.set_volume(value.end() / UNITY_VOLUME_PERCENT);
            }
            cx.notify();
        });
        Self {
            clan_id,
            editing,
            settings,
            focus_handle: cx.focus_handle(),
            name_input,
            picked_path: None,
            file_label,
            preview_url,
            local_pcm: None,
            waveform_peaks: Vec::new(),
            decode_pending: false,
            decode_failed: false,
            local_player: None,
            trim_slider,
            volume_slider,
            submitting: false,
            _name_sub: name_sub,
            _trim_sub: trim_sub,
            _volume_sub: volume_sub,
            _voice_observe: voice_observe,
            _pick_task: None,
            _submit_task: None,
            _decode_task: None,
            _preview_tick: None,
        }
    }

    fn can_preview(&self) -> bool {
        self.picked_path.is_some() && self.local_pcm.is_some()
            || self
                .preview_url
                .as_ref()
                .is_some_and(|url| !url.starts_with("file://"))
    }

    fn is_previewing(&self, cx: &App) -> bool {
        if self.picked_path.is_some() {
            return self
                .local_player
                .as_ref()
                .is_some_and(|player| player.is_playing());
        }
        self.preview_url.as_ref().is_some_and(|url| {
            !url.starts_with("file://")
                && VoiceStore::global(cx).read(cx).previewing_sound() == Some(url.as_ref())
        })
    }

    fn preview_timeline(&self, cx: &App) -> (f64, f64) {
        if self.picked_path.is_some() {
            if let Some(player) = &self.local_player {
                return (player.position_secs(), player.duration_secs());
            }
            if let Some(pcm) = &self.local_pcm {
                let (start, end) = self.trim_times(cx);
                return (0.0, (end - start).max(0.0).min(pcm.duration_secs()));
            }
            return (0.0, 0.0);
        }
        if let Some(url) = self
            .preview_url
            .as_ref()
            .filter(|url| !url.starts_with("file://"))
        {
            let voice = VoiceStore::global(cx).read(cx);
            return voice
                .sound_preview_timeline(url)
                .unwrap_or((0.0, voice.cached_sound_duration(url).unwrap_or(0.0)));
        }
        (0.0, 0.0)
    }

    fn trim_times(&self, cx: &App) -> (f64, f64) {
        let Some(pcm) = &self.local_pcm else {
            return (0.0, 0.0);
        };
        let duration = pcm.duration_secs();
        let (start, end) = self.trim_slider.read(cx).value();
        (
            duration * f64::from(start / 100.0),
            duration * f64::from(end / 100.0),
        )
    }

    fn selected_duration(&self, cx: &App) -> f64 {
        let (start, end) = self.trim_times(cx);
        (end - start).max(0.0)
    }

    fn needs_processing(&self, cx: &App) -> bool {
        let (start, end) = self.trim_slider.read(cx).value();
        audio_needs_processing(start, end, self.volume_slider.read(cx).value().end())
    }

    fn volume_gain(&self, cx: &App) -> f32 {
        self.volume_slider.read(cx).value().end() / UNITY_VOLUME_PERCENT
    }

    fn processed_pcm(&self, cx: &App) -> Option<DecodedPcm> {
        let pcm = self.local_pcm.as_ref()?;
        let (start, end) = self.trim_times(cx);
        process_pcm(pcm, start, end, 1.0).ok()
    }

    fn stop_local_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.local_player {
            player.pause();
        }
        self.local_player = None;
        self._preview_tick = None;
        cx.notify();
    }

    fn start_preview_tick(&mut self, cx: &mut Context<Self>) {
        self._preview_tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PREVIEW_TICK).await;
                let keep = this
                    .update(cx, |this, cx| {
                        let Some(player) = &this.local_player else {
                            return false;
                        };
                        if player.finished() {
                            player.pause();
                            cx.notify();
                            return false;
                        }
                        if player.is_playing() {
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .ok()
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn toggle_local_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.local_player {
            if player.is_playing() {
                player.pause();
                self._preview_tick = None;
                cx.notify();
                return;
            }
            if player.is_ready() {
                player.play();
                self.start_preview_tick(cx);
                cx.notify();
                return;
            }
        }
        let Some(pcm) = self.processed_pcm(cx) else {
            return;
        };
        let Ok(player) = AudioPlayer::new() else {
            tracing::warn!("audio output unavailable");
            return;
        };
        player.set_data(pcm);
        player.set_volume(self.volume_gain(cx));
        player.play();
        self.local_player = Some(player);
        self.start_preview_tick(cx);
        cx.notify();
    }

    fn decode_picked_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let check_path = path.clone();
        let locale = self.settings.read(cx).language.clone();
        self._decode_task = Some(cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_spawn(async move {
                    let bytes = std::fs::read(&path).map_err(|_| "invalid_file".to_string())?;
                    let pcm = mezon_audio::decode_audio(bytes).map_err(|err| err.to_string())?;
                    let peaks = waveform_amplitudes(&pcm);
                    Ok::<_, String>((pcm, peaks))
                })
                .await;
            let error_code = match &decoded {
                Err(err) if err == "invalid_file" => Some("invalid_file"),
                Err(_) => Some("decode_failed"),
                Ok(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                if this.picked_path.as_ref() != Some(&check_path) {
                    return;
                }
                match decoded {
                    Ok((pcm, peaks)) => {
                        this.local_pcm = Some(pcm);
                        this.waveform_peaks = peaks;
                        this.decode_failed = false;
                    }
                    Err(err) => {
                        tracing::warn!("sound picker decode failed: {err}");
                        this.local_pcm = None;
                        this.waveform_peaks.clear();
                        this.decode_failed = true;
                    }
                }
                this.decode_pending = false;
                cx.notify();
            });
            if let Some(code) = error_code {
                show_error(cx, sound_error_message(&locale, code));
            }
        }));
    }

    fn render_waveform(&self, theme: &Theme) -> impl IntoElement {
        let amplitudes = self.waveform_peaks.clone();
        h_flex()
            .w_full()
            .h(px(52.0))
            .items_center()
            .gap(px(0.75))
            .children(
                amplitudes
                    .into_iter()
                    .enumerate()
                    .map(|(index, amplitude)| {
                        div()
                            .id(("sound-waveform-bar", index))
                            .flex_1()
                            .min_w(px(0.25))
                            .h(px(3.0 + amplitude * 47.0))
                            .rounded_full()
                            .bg(theme.tokens.text_theme_message)
                    }),
            )
    }

    fn render_preview_playbar(
        &self,
        previewing: bool,
        theme: &Theme,
        cx: &App,
    ) -> impl IntoElement {
        let (position, duration) = self.preview_timeline(cx);
        let progress = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let time_label = audio_time_label(if previewing { position } else { 0.0 }, duration);

        div()
            .flex_1()
            .min_w_0()
            .h(px(36.0))
            .px(px(10.0))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_tertiary)
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme.border)
                    .overflow_hidden()
                    .when(progress > 0.0, |track| {
                        track.child(
                            div()
                                .h_full()
                                .w(gpui::relative(progress))
                                .rounded_full()
                                .bg(theme.brand),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(time_label),
            )
    }

    fn can_submit(&self, cx: &App) -> bool {
        if self.submitting {
            return false;
        }
        let name = self.name_input.read(cx).value().trim().to_string();
        if !is_valid_emoticon_shortname(&name) {
            return false;
        }
        if let Some(editing) = &self.editing {
            return editing.shortname != name;
        }
        if self.picked_path.is_none() || self.decode_pending {
            return false;
        }
        if self.decode_failed {
            return true;
        }
        self.local_pcm.is_some()
            && self.selected_duration(cx) > 0.0
            && self.selected_duration(cx) <= MAX_TRIM_SECS
    }

    fn pick_file(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let prompt: SharedString =
            mezon_i18n::t(&locale, "clanSoundSetting.content.chooseAudioFile")
                .to_string()
                .into();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        self._pick_task = Some(cx.spawn(async move |this, cx| {
            let Some(paths) = crate::util::file_dialog::resolve(rx, cx).await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path_buf = path.clone();
            let validated = cx
                .background_spawn(async move { validate_sound_file(&path_buf, MAX_SOUND_BYTES) })
                .await;
            if let Err(code) = validated {
                show_error(cx, sound_error_message(&locale, &code));
                return;
            }
            let label = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("audio")
                .to_string();
            let preview = format!("file://{}", path.display());
            let default_name = default_voice_shortname();
            let _ = this.update_in(cx, |this, window, cx| {
                this.picked_path = Some(path.clone());
                this.file_label = label.into();
                this.preview_url = Some(preview.into());
                this.local_pcm = None;
                this.waveform_peaks.clear();
                this.decode_pending = true;
                this.decode_failed = false;
                this.stop_local_preview(cx);
                this.trim_slider.update(cx, |slider, cx| slider.reset(cx));
                this.volume_slider.update(cx, |slider, cx| {
                    slider.set_value(DEFAULT_VOLUME_PERCENT, window, cx);
                });
                if this.editing.is_none() {
                    this.name_input.update(cx, |state, cx| {
                        state.set_value(default_name, window, cx);
                    });
                }
                this.decode_picked_file(path, cx);
                cx.notify();
            });
        }));
    }

    fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        if !self.can_preview() {
            return;
        }
        VoiceStore::global(cx).update(cx, |store, cx| store.stop_sound_preview(cx));
        if self.picked_path.is_some() {
            self.toggle_local_preview(cx);
            return;
        }
        let Some(url) = self.preview_url.as_ref().map(|url| url.to_string()) else {
            return;
        };
        VoiceStore::global(cx).update(cx, |store, cx| {
            store.toggle_sound_preview(url, cx);
        });
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let name = self.name_input.read(cx).value().trim().to_string();
        if !is_valid_emoticon_shortname(&name) {
            let message = mezon_i18n::t(&locale, "clanSoundSetting.modal.errorName")
                .replace("{{min}}", "3")
                .replace("{{max}}", "64");
            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
            return;
        }

        let needs_processing = self.needs_processing(cx);
        if needs_processing
            && estimated_mono_wav_bytes(self.selected_duration(cx))
                .is_none_or(|size| size > MAX_SOUND_BYTES)
        {
            Shell::global(cx).update(cx, |shell, cx| {
                shell.error(sound_error_message(&locale, "size_limit"), cx)
            });
            return;
        }

        self.submitting = true;
        cx.notify();
        let clan_id = self.clan_id;
        let editing = self.editing.clone();
        let picked_path = self.picked_path.clone();
        let processing = self.local_pcm.as_ref().and_then(|pcm| {
            if !needs_processing {
                return None;
            }
            let (start, end) = self.trim_times(cx);
            Some((
                DecodedPcm {
                    samples: Arc::clone(&pcm.samples),
                    channels: pcm.channels,
                    sample_rate: pcm.sample_rate,
                },
                start,
                end,
                self.volume_gain(cx),
            ))
        });

        self._submit_task = Some(cx.spawn(async move |this, cx| {
            let processed = match processing {
                Some((pcm, start, end, gain)) => Some(
                    cx.background_spawn(async move {
                        process_pcm_to_wav(&pcm, start, end, gain)
                            .map_err(|error| error.to_string())
                    })
                    .await,
                ),
                None => None,
            };
            let processed_bytes = match processed {
                Some(Ok(bytes)) if bytes.len() as u64 <= MAX_SOUND_BYTES => Some(bytes),
                Some(Ok(_)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.notify();
                    });
                    show_error(cx, sound_error_message(&locale, "size_limit"));
                    return;
                }
                Some(Err(error)) => {
                    tracing::warn!("sound processing failed: {error}");
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.notify();
                    });
                    show_error(cx, sound_error_message(&locale, "decode_failed"));
                    return;
                }
                None => None,
            };
            let task = match (editing.as_ref(), processed_bytes, picked_path.as_ref()) {
                (Some(target), _, _) => cx.update(|cx| {
                    StickerStore::global(cx).update(cx, |store, cx| {
                        store.update_sound(&target.id, clan_id, &target.source, &name, cx)
                    })
                }),
                (None, Some(bytes), _) => cx.update(|cx| {
                    StickerStore::global(cx).update(cx, |store, cx| {
                        store.create_sound_from_bytes(clan_id, bytes, "wav", &name, cx)
                    })
                }),
                (None, None, Some(path)) => cx.update(|cx| {
                    StickerStore::global(cx)
                        .update(cx, |store, cx| store.create_sound(clan_id, path, &name, cx))
                }),
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.notify();
                    });
                    show_error(cx, "missing file".into());
                    return;
                }
            };
            let result = task.await;

            match result {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.emit(SoundPickerEvent::Saved);
                        cx.notify();
                    });
                    cx.update(|cx| {
                        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                    });
                }
                Err(e) => {
                    tracing::error!("sound save failed: {e}");
                    let message = sound_error_message(&locale, &e.to_string());
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.notify();
                    });
                    show_error(cx, message);
                }
            }
        }));
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.stop_local_preview(cx);
        VoiceStore::global(cx).update(cx, |store, cx| store.stop_sound_preview(cx));
        cx.emit(SoundPickerEvent::Cancelled);
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

fn waveform_amplitudes(pcm: &DecodedPcm) -> Vec<f32> {
    let frames = pcm.frames();
    if frames == 0 {
        return vec![0.08; WAVEFORM_BARS];
    }
    (0..WAVEFORM_BARS)
        .map(|bar| {
            let start = bar * frames / WAVEFORM_BARS;
            let end = ((bar + 1) * frames / WAVEFORM_BARS)
                .max(start + 1)
                .min(frames);
            let peak = (start..end)
                .flat_map(|frame| {
                    let offset = frame * pcm.channels;
                    pcm.samples[offset..offset + pcm.channels].iter()
                })
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            peak.clamp(0.08, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_peaks_are_cached_at_a_bounded_height() {
        let pcm = DecodedPcm {
            samples: vec![2.0, -3.0, 0.0, 0.5].into(),
            channels: 1,
            sample_rate: 4,
        };
        let peaks = waveform_amplitudes(&pcm);
        assert_eq!(peaks.len(), WAVEFORM_BARS);
        assert!(peaks.iter().all(|peak| (0.08..=1.0).contains(peak)));
        assert_eq!(peaks[0], 1.0);
    }

    #[test]
    fn untouched_audio_bypasses_processing() {
        assert!(!audio_needs_processing(0.0, 100.0, 100.0));
        assert!(audio_needs_processing(1.0, 100.0, 100.0));
        assert!(audio_needs_processing(0.0, 99.0, 100.0));
        assert!(audio_needs_processing(0.0, 100.0, 80.0));
    }

    #[test]
    fn volume_defaults_to_eighty_percent_of_unity_gain() {
        assert_eq!(DEFAULT_VOLUME_PERCENT, 80.0);
        assert_eq!(DEFAULT_VOLUME_PERCENT / UNITY_VOLUME_PERCENT, 0.8);
        assert_eq!(MAX_VOLUME_PERCENT, 100.0);
    }
}

fn sound_error_message(locale: &str, code: &str) -> String {
    match code {
        "size_limit" => mezon_i18n::t(locale, "clanSoundSetting.toast.errorSizeLimit").to_string(),
        "unsupported_type" | "empty" | "invalid_file" | "decode_failed" => {
            mezon_i18n::t(locale, "clanSoundSetting.toast.errorFileType").to_string()
        }
        "invalid_name" => mezon_i18n::t(locale, "clanSoundSetting.modal.errorName")
            .replace("{{min}}", "3")
            .replace("{{max}}", "64"),
        _ => mezon_i18n::t(locale, "clanSoundSetting.modal.errorUploadFailed").to_string(),
    }
}

fn show_error(cx: &mut AsyncApp, message: String) {
    cx.update(|cx| {
        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
    });
}

impl Render for SoundPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let is_edit = self.editing.is_some();
        let title = mezon_i18n::t(
            &locale,
            if is_edit {
                "clanSoundSetting.modal.titleEdit"
            } else {
                "clanSoundSetting.modal.titleUpload"
            },
        );
        let subtitle = mezon_i18n::t(&locale, "clanSoundSetting.modal.subtitle");
        let preview_label = mezon_i18n::t(&locale, "clanSoundSetting.modal.preview");
        let audio_label = mezon_i18n::t(&locale, "clanSoundSetting.modal.audioFile");
        let name_label = mezon_i18n::t(&locale, "clanSoundSetting.modal.soundName");
        let volume_label = mezon_i18n::t(&locale, "clanSoundSetting.modal.soundVolume");
        let browse = mezon_i18n::t(&locale, "clanSoundSetting.modal.browse");
        let cancel = mezon_i18n::t(&locale, "clanSoundSetting.modal.cancel");
        let submit = mezon_i18n::t(
            &locale,
            if self.submitting {
                "clanSoundSetting.modal.uploading"
            } else if is_edit {
                "clanSoundSetting.modal.update"
            } else {
                "clanSoundSetting.modal.upload"
            },
        );
        let can_submit = self.can_submit(cx);
        let can_preview = self.can_preview();
        let name_len = self.name_input.read(cx).value().trim().chars().count();
        let previewing = self.is_previewing(cx);
        let (trim_start, trim_end) = self.trim_times(cx);
        let trim_too_long = !is_edit && self.selected_duration(cx) > MAX_TRIM_SECS;
        let (preview_position, preview_duration) = self.preview_timeline(cx);
        let volume_percent = self.volume_slider.read(cx).value().end().round() as u32;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| this.close(cx)))
            .w(px(720.))
            .gap_4()
            .p(px(24.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .w_full()
                    .relative()
                    .flex()
                    .justify_center()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_theme_message)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        div()
                            .id("sound-picker-close")
                            .absolute()
                            .top_0()
                            .right_0()
                            .p_1()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(div().w_full().h(px(1.0)).bg(theme.border))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_message)
                            .child(preview_label.to_uppercase()),
                    )
                    .when(can_preview, |section| {
                        section.child(
                            h_flex()
                                .w_full()
                                .p(px(16.0))
                                .rounded_lg()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.bg_secondary)
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .id("sound-picker-play")
                                        .size(px(36.0))
                                        .rounded_full()
                                        .bg(theme.brand)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(can_preview, |el| {
                                            el.cursor_pointer().on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.toggle_preview(cx)
                                                }),
                                            )
                                        })
                                        .when(!can_preview, |el| el.opacity(0.5))
                                        .child(
                                            Icon::new(if previewing {
                                                IconName::AudioPause
                                            } else {
                                                IconName::AudioPlay
                                            })
                                            .size(px(16.0))
                                            .text_color(gpui::white()),
                                        ),
                                )
                                .when(is_edit, |row| {
                                    row.child(self.render_preview_playbar(previewing, &theme, cx))
                                })
                                .when(!is_edit, |row| {
                                    row.child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .h(px(64.0))
                                                    .flex()
                                                    .items_center()
                                                    .child(self.render_waveform(&theme))
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .top_0()
                                                            .right_0()
                                                            .bottom_0()
                                                            .left_0()
                                                            .child(
                                                                WaveformTrimSlider::new(
                                                                    &self.trim_slider,
                                                                )
                                                                .disabled(!can_preview),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .w_full()
                                                    .justify_between()
                                                    .text_xs()
                                                    .text_color(if trim_too_long {
                                                        theme.danger_text
                                                    } else {
                                                        theme.tokens.text_theme_primary
                                                    })
                                                    .child(format_mmss(trim_start))
                                                    .child(if previewing {
                                                        format!(
                                                            "{} / {}",
                                                            format_mmss(preview_position),
                                                            format_mmss(preview_duration)
                                                        )
                                                    } else {
                                                        format_mmss(trim_end)
                                                    }),
                                            ),
                                    )
                                }),
                        )
                    })
                    .when(!can_preview, |section| {
                        section.child(
                            div()
                                .w_full()
                                .h(px(90.0))
                                .rounded_lg()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.bg_secondary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::UploadSoundIcon)
                                        .size(px(48.0))
                                        .text_color(theme.tokens.text_theme_primary),
                                ),
                        )
                    }),
            )
            .when(can_preview && !is_edit, |root| {
                root.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(theme.tokens.text_theme_message)
                                        .child(volume_label.to_uppercase()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(format!("{volume_percent}%")),
                                ),
                        )
                        .child(Slider::new(&self.volume_slider).percentage_positioning()),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.tokens.text_theme_message)
                                    .child(audio_label.to_uppercase()),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .h(px(FORM_CONTROL_H))
                                            .flex()
                                            .items_center()
                                            .pl(px(12.))
                                            .pr(px(16.))
                                            .rounded_md()
                                            .bg(theme.bg_secondary)
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_sm()
                                                    .text_color(theme.tokens.text_theme_message)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(self.file_label.clone()),
                                            ),
                                    )
                                    .child({
                                        let mut browse_btn =
                                            Button::new("sound-browse").label(browse);
                                        if is_edit || self.submitting {
                                            browse_btn = browse_btn.disabled(true);
                                        }
                                        browse_btn.on_click(
                                            cx.listener(|this, _, _, cx| this.pick_file(cx)),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(theme.tokens.text_theme_message)
                                            .child(name_label.to_uppercase()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(if name_len > 40 {
                                                theme.danger_text
                                            } else {
                                                theme.tokens.text_theme_primary
                                            })
                                            .child(format!("{name_len}/{EMOTICON_SHORTNAME_MAX}")),
                                    ),
                            )
                            .child(Input::new(&self.name_input)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("sound-cancel")
                            .label(cancel)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    )
                    .child({
                        let mut submit_btn = Button::new("sound-upload").label(submit).primary();
                        if !can_submit {
                            submit_btn = submit_btn.disabled(true);
                        }
                        submit_btn.on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                    }),
            )
    }
}
