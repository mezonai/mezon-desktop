use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::AsyncReadExt;
use gpui::{
    App, Bounds, ClickEvent, Context, DragMoveEvent, ElementId, Empty, EntityId, FontFeatures,
    MouseButton, MouseDownEvent, Pixels, Rgba, SharedString, Task, Window, canvas, div,
    http_client::HttpClient, prelude::*, px, relative,
};
use mezon_audio::{AudioPlayer, DecodedPcm, PcmStream};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, Sizable, Size, Spinner};

const AUDIO_OUTPUT_TOAST_KEY: &str = "audio-output-unavailable";
const AUDIO_FETCH_MAX_BYTES: usize = 64 * 1024 * 1024;
const AUDIO_TICK_INTERVAL: Duration = Duration::from_millis(200);
const AUDIO_TICK_IDLE: Duration = Duration::from_secs(1);
const SEEK_TRACK_WIDTH: f32 = 112.0;
const AUDIO_FETCH_CHUNK: usize = 64 * 1024;
const AUDIO_FETCH_QUEUE: usize = 8;

static TABULAR_FIGURES: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".to_string(), 1)])));

const PILL_BG: Rgba = Rgba {
    r: 0x50 as f32 / 255.,
    g: 0x5c as f32 / 255.,
    b: 0xdc as f32 / 255.,
    a: 1.0,
};
const PLAY_HOVER_BG: Rgba = Rgba {
    r: 0xef as f32 / 255.,
    g: 0xf6 as f32 / 255.,
    b: 0xff as f32 / 255.,
    a: 1.0,
};
const PILL_MIN_WIDTH: f32 = 208.0;
const TRACK_BG: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.35,
};

#[derive(Clone)]
struct SeekDrag(EntityId);

impl Render for SeekDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct PreviewSeekDrag {
    message_id: i64,
    index: usize,
}

impl Render for PreviewSeekDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct AudioActivation {
    pub url: SharedString,
    pub duration: f64,
    pub start_secs: f64,
    pub download_url: SharedString,
    pub download_name: SharedString,
}

enum LoadState {
    Loading,
    Ready,
    Failed,
}

pub struct AudioPlayerView {
    url: SharedString,
    download_url: SharedString,
    download_name: SharedString,
    player: Option<AudioPlayer>,
    state: LoadState,
    want_play: bool,
    pending_seek: Option<f64>,
    server_duration: f64,
    playhead: f64,
    track_bounds: Bounds<Pixels>,
    time_label: SharedString,
    last_label_seconds: (u64, u64),
    _load_task: Option<Task<()>>,
    tick_task: Option<Task<()>>,
}

impl AudioPlayerView {
    pub fn new(activation: AudioActivation, cx: &mut Context<Self>) -> Self {
        let AudioActivation {
            url,
            duration,
            start_secs,
            download_url,
            download_name,
        } = activation;
        let mut view = Self {
            url: url.clone(),
            download_url,
            download_name,
            player: None,
            state: LoadState::Loading,
            want_play: true,
            pending_seek: (start_secs > 0.0).then_some(start_secs),
            server_duration: duration,
            playhead: start_secs.max(0.0),
            track_bounds: Bounds::default(),
            time_label: SharedString::from(time_label(start_secs.max(0.0), duration)),
            last_label_seconds: (whole_seconds(start_secs), whole_seconds(duration)),
            _load_task: None,
            tick_task: None,
        };
        if view.ensure_player(cx) {
            view.start_loading(url, cx);
        }
        view
    }

    fn ensure_player(&mut self, cx: &mut Context<Self>) -> bool {
        if self.player.is_some() {
            return true;
        }
        match AudioPlayer::new() {
            Ok(player) => {
                self.player = Some(player);
                true
            }
            Err(err) => {
                tracing::warn!("audio output unavailable: {err}");
                self.state = LoadState::Failed;
                cx.defer(report_audio_output_unavailable);
                false
            }
        }
    }

    fn start_loading(&mut self, url: SharedString, cx: &mut Context<Self>) {
        if self.player.is_none() {
            self.state = LoadState::Failed;
            return;
        }
        let client = cx.http_client();
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let (byte_tx, byte_rx) = flume::bounded(AUDIO_FETCH_QUEUE);
            let ready = mezon_audio::spawn_stream_decode(byte_rx);
            let fetch = cx
                .background_executor()
                .spawn(async move { fetch_audio(client, url, byte_tx).await });

            if let Ok(Ok(stream)) = ready.recv_async().await {
                let _ = this.update(cx, |view, cx| view.on_stream_ready(stream, cx));
                if let Err(err) = fetch.await {
                    tracing::warn!("audio download failed: {err}");
                }
                return;
            }

            let bytes = match fetch.await {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::warn!("audio download failed: {err}");
                    let _ = this.update(cx, |view, cx| view.on_load_failed(cx));
                    return;
                }
            };
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(bytes) })
                .await;
            let _ = this.update(cx, |view, cx| match decoded {
                Ok(pcm) => view.on_pcm_ready(pcm, cx),
                Err(err) => {
                    tracing::warn!("audio decode failed: {err}");
                    view.on_load_failed(cx);
                }
            });
        }));
    }

    fn on_stream_ready(&mut self, stream: Arc<PcmStream>, cx: &mut Context<Self>) {
        self.state = LoadState::Ready;
        if let Some(player) = &self.player {
            player.set_stream(stream);
            self.begin_playback(cx);
        }
        cx.notify();
    }

    fn on_pcm_ready(&mut self, pcm: DecodedPcm, cx: &mut Context<Self>) {
        self.state = LoadState::Ready;
        if let Some(player) = &self.player {
            player.set_data(pcm);
            self.begin_playback(cx);
        }
        cx.notify();
    }

    fn begin_playback(&mut self, cx: &mut Context<Self>) {
        if self.pending_seek.is_some() && !self.apply_pending_seek() {
            self.restart_tick(cx);
            return;
        }
        if self.want_play {
            if let Some(player) = &self.player {
                player.play();
            }
            self.restart_tick(cx);
        }
    }

    fn apply_pending_seek(&mut self) -> bool {
        let Some(at) = self.pending_seek else {
            return true;
        };
        let landed = self.player.as_ref().is_some_and(|player| player.seek(at));
        if !landed {
            return false;
        }
        self.pending_seek = None;
        self.set_playhead(at);
        if self.want_play
            && let Some(player) = &self.player
            && !player.is_playing()
        {
            player.play();
        }
        true
    }

    fn on_load_failed(&mut self, cx: &mut Context<Self>) {
        self.state = LoadState::Failed;
        cx.notify();
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, LoadState::Ready)
    }

    fn restart_tick(&mut self, cx: &mut Context<Self>) {
        self.tick_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let interval = this
                    .update(cx, |_view, cx| {
                        if cx.active_window().is_none() {
                            AUDIO_TICK_IDLE
                        } else {
                            AUDIO_TICK_INTERVAL
                        }
                    })
                    .unwrap_or(AUDIO_TICK_INTERVAL);
                cx.background_executor().timer(interval).await;
                let keep_going = this.update(cx, |view, cx| view.tick(cx)).unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let inactive = cx.active_window().is_none();
        if self.pending_seek.is_some() {
            let landed = self.apply_pending_seek();
            if self.pending_seek.is_some() {
                return self.want_play
                    || self
                        .player
                        .as_ref()
                        .is_some_and(|player| player.is_playing());
            }
            if landed && !inactive {
                cx.notify();
            }
        }
        let (finished, position, playing) = {
            let Some(player) = &self.player else {
                return self.pending_seek.is_some();
            };
            if inactive {
                return player.is_playing() || self.pending_seek.is_some();
            }
            let finished = player.finished();
            if finished {
                player.pause();
            }
            (
                finished,
                player.position_secs(),
                player.is_playing() && !finished,
            )
        };
        let position = if finished {
            self.effective_duration()
        } else {
            position
        };
        let before = self.playhead;
        let before_label = self.last_label_seconds;
        self.set_playhead(position);
        let label_changed = self.last_label_seconds != before_label;
        let thumb_moved = seek_thumb_moved(before, self.playhead, self.effective_duration());
        if finished || label_changed || thumb_moved {
            cx.notify();
        }
        playing || self.pending_seek.is_some()
    }

    fn set_playhead(&mut self, position: f64) {
        let duration = self.effective_duration();
        let position = if duration > 0.0 {
            position.clamp(0.0, duration)
        } else {
            position.max(0.0)
        };
        self.playhead = position;
        let seconds = (whole_seconds(position), whole_seconds(duration));
        if seconds != self.last_label_seconds {
            self.last_label_seconds = seconds;
            self.time_label = SharedString::from(time_label(position, duration));
        }
    }

    fn seek_to(&mut self, secs: f64, cx: &mut Context<Self>) {
        let duration = self.effective_duration();
        if duration <= 0.0 {
            return;
        }
        let secs = secs.clamp(0.0, duration);
        if !self.is_ready() {
            self.pending_seek = Some(secs);
            self.set_playhead(secs);
            cx.notify();
            return;
        }
        let landed = self.player.as_ref().is_some_and(|player| player.seek(secs));
        if landed {
            self.pending_seek = None;
        } else {
            self.pending_seek = Some(secs);
        }
        self.set_playhead(secs);
        cx.notify();
    }

    fn seek_to_x(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let duration = self.effective_duration();
        if duration <= 0.0 {
            return;
        }
        let fraction = fraction_from_position(self.track_bounds, x);
        self.seek_to(fraction as f64 * duration, cx);
    }

    fn effective_duration(&self) -> f64 {
        match &self.player {
            Some(player) if player.duration_secs() > 0.0 => player.duration_secs(),
            _ => self.server_duration,
        }
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if !self.ensure_player(cx) {
            cx.notify();
            return;
        }
        if matches!(self.state, LoadState::Failed) {
            self.state = LoadState::Loading;
            self.want_play = true;
            self.time_label = SharedString::from(time_label(0.0, self.server_duration));
            self.last_label_seconds = (0, whole_seconds(self.server_duration));
            self.start_loading(self.url.clone(), cx);
            cx.notify();
            return;
        }
        let ready = self.is_ready();
        let action = self.player.as_ref().map(|player| {
            if !ready {
                return PlayAction::Defer;
            }
            if player.is_playing() {
                player.pause();
                PlayAction::Pause
            } else {
                let restart = player.finished();
                player.play();
                PlayAction::Play { restart }
            }
        });
        match action {
            Some(PlayAction::Pause) => self.tick_task = None,
            Some(PlayAction::Play { restart }) => {
                if restart {
                    self.set_playhead(0.0);
                }
                self.restart_tick(cx);
            }
            _ => self.want_play = !self.want_play,
        }
        cx.notify();
    }
}

enum PlayAction {
    Pause,
    Play { restart: bool },
    Defer,
}

impl Render for AudioPlayerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let playing = self
            .player
            .as_ref()
            .map(|player| player.is_playing())
            .unwrap_or(false);
        let play_icon = match self.state {
            LoadState::Failed => IconName::TriangleAlert,
            _ if playing => IconName::AudioPause,
            _ => IconName::AudioPlay,
        };
        let download_url = self.download_url.clone();
        let download_name = self.download_name.clone();
        let duration = self.effective_duration();
        let fraction = if duration > 0.0 {
            (self.playhead / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let seek = self.seek_track(fraction, cx);
        audio_pill(
            "audio-play",
            "audio-download",
            play_icon,
            self.time_label.clone(),
            seek,
            cx.listener(|view, _, _window, cx| view.toggle_play(cx)),
            move |_, _, cx| {
                crate::util::download::save_with_progress_toast(
                    download_url.clone(),
                    download_name.clone(),
                    cx,
                )
            },
        )
    }
}

impl AudioPlayerView {
    fn seek_track(&self, fraction: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entity_id = cx.entity_id();
        let view = cx.entity();
        div()
            .id("audio-seek")
            .relative()
            .flex()
            .items_center()
            .w(px(SEEK_TRACK_WIDTH))
            .h(px(14.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    view.seek_to_x(event.position.x, cx);
                }),
            )
            .on_drag(SeekDrag(entity_id), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(
                cx.listener(move |view, event: &DragMoveEvent<SeekDrag>, _window, cx| {
                    let SeekDrag(id) = event.drag(cx);
                    if *id != entity_id {
                        return;
                    }
                    cx.stop_propagation();
                    view.seek_to_x(event.event.position.x, cx);
                }),
            )
            .child(seek_track_face(fraction))
            .child(
                canvas(
                    move |bounds, _, cx| view.update(cx, |this, _| this.track_bounds = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}

fn seek_track_face(fraction: f32) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(px(4.))
        .rounded_full()
        .bg(TRACK_BG)
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(relative(fraction))
                .rounded_full()
                .bg(gpui::white()),
        )
        .child(
            div()
                .absolute()
                .top(px(-3.))
                .left(relative(fraction))
                .ml(px(-5.))
                .size(px(10.))
                .rounded_full()
                .bg(gpui::white()),
        )
}

pub(crate) fn preview_seek_track(
    id: impl Into<ElementId>,
    owner: (i64, usize),
    on_fraction: impl Fn(f32, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let bounds = std::rc::Rc::new(std::cell::RefCell::new(Bounds::default()));
    let paint_bounds = bounds.clone();
    let down_bounds = bounds.clone();
    let drag_bounds = bounds.clone();
    let on_fraction = std::rc::Rc::new(on_fraction);
    let on_down = on_fraction.clone();
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .w(px(SEEK_TRACK_WIDTH))
        .h(px(14.))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let fraction = fraction_from_position(*down_bounds.borrow(), event.position.x);
                on_down(fraction, window, cx);
            },
        )
        .on_drag(
            PreviewSeekDrag {
                message_id: owner.0,
                index: owner.1,
            },
            |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            },
        )
        .on_drag_move(move |event: &DragMoveEvent<PreviewSeekDrag>, window, cx| {
            let drag = event.drag(cx);
            if drag.message_id != owner.0 || drag.index != owner.1 {
                return;
            }
            cx.stop_propagation();
            let fraction = fraction_from_position(*drag_bounds.borrow(), event.event.position.x);
            on_fraction(fraction, window, cx);
        })
        .child(seek_track_face(0.0))
        .child(
            canvas(
                move |bounds, _, _| *paint_bounds.borrow_mut() = bounds,
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}

pub(crate) fn audio_pill(
    play_id: impl Into<ElementId>,
    download_id: impl Into<ElementId>,
    play_icon: IconName,
    time_label: SharedString,
    seek: gpui::AnyElement,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_download: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .flex()
        .w_full()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .min_w(px(PILL_MIN_WIDTH))
                .rounded_full()
                .py(px(6.))
                .pl(px(6.))
                .pr(px(14.))
                .bg(PILL_BG)
                .text_color(gpui::white())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .id(play_id)
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(24.))
                                .rounded_full()
                                .bg(gpui::white())
                                .cursor_pointer()
                                .hover(|s| s.bg(PLAY_HOVER_BG))
                                .on_click(on_toggle)
                                .child(Icon::new(play_icon).size(px(16.)).text_color(PILL_BG)),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_features(TABULAR_FIGURES.clone())
                                .whitespace_nowrap()
                                .child(time_label),
                        ),
                )
                .child(seek)
                .child(
                    div()
                        .id(download_id)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(on_download)
                        .child(
                            Icon::new(IconName::Download)
                                .size(px(20.))
                                .text_color(gpui::white()),
                        ),
                ),
        )
        .into_any_element()
}

/// The pill a voice message shows while its bytes are still on their way — for
/// the sender until the upload finishes, for everyone else until `presign_finish`
/// names the key. Spelling out "Uploading…" beats a bare spinner: the row is
/// otherwise indistinguishable from an audio player that simply will not start.
pub(crate) fn audio_sending_pill(duration: f64, locale: &str) -> gpui::AnyElement {
    div()
        .flex()
        .w_full()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .min_w(px(PILL_MIN_WIDTH))
                .rounded_full()
                .py(px(6.))
                .pl(px(6.))
                .pr(px(14.))
                .bg(PILL_BG)
                .text_color(gpui::white())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded_full()
                        .bg(gpui::white())
                        .child(Spinner::new().with_size(Size::Small).color(PILL_BG.into())),
                )
                .child(
                    div()
                        .ml_2()
                        .text_size(px(14.))
                        .whitespace_nowrap()
                        .child(mezon_i18n::t(locale, "message.attachment.uploading")),
                )
                .when(duration > 0.0, |d| {
                    d.child(
                        div()
                            .text_size(px(14.))
                            .whitespace_nowrap()
                            .child(audio_time_label(0.0, duration)),
                    )
                }),
        )
        .into_any_element()
}

pub(crate) fn audio_failed_pill(duration: f64) -> gpui::AnyElement {
    div()
        .flex()
        .w_full()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .min_w(px(PILL_MIN_WIDTH))
                .rounded_full()
                .py(px(6.))
                .pl(px(6.))
                .pr(px(14.))
                .bg(PILL_BG)
                .text_color(gpui::white())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded_full()
                        .bg(gpui::white())
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(16.))
                                .text_color(PILL_BG),
                        ),
                )
                .child(
                    div()
                        .ml_2()
                        .text_size(px(14.))
                        .whitespace_nowrap()
                        .child(audio_time_label(0.0, duration)),
                ),
        )
        .into_any_element()
}

async fn fetch_audio(
    client: Arc<dyn HttpClient>,
    url: SharedString,
    byte_tx: flume::Sender<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let mut response = client.get(url.as_ref(), ().into(), true).await?;
    if !response.status().is_success() {
        anyhow::bail!("audio fetch status {}", response.status());
    }
    if let Some(length) = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && length > AUDIO_FETCH_MAX_BYTES as u64
    {
        anyhow::bail!(
            "response body of {length} bytes exceeds the {AUDIO_FETCH_MAX_BYTES} byte transfer limit"
        );
    }

    let mut body = Vec::new();
    let mut buffer = vec![0u8; AUDIO_FETCH_CHUNK];
    loop {
        let read = response.body_mut().read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if body.len() + read > AUDIO_FETCH_MAX_BYTES {
            anyhow::bail!("response body exceeds the {AUDIO_FETCH_MAX_BYTES} byte transfer limit");
        }
        body.extend_from_slice(&buffer[..read]);
        let _ = byte_tx.send_async(buffer[..read].to_vec()).await;
    }
    Ok(body)
}

pub(crate) fn report_audio_output_unavailable(cx: &mut App) {
    Shell::global(cx).update(cx, |shell, cx| {
        shell.error_once(AUDIO_OUTPUT_TOAST_KEY, "Audio output unavailable", cx)
    });
}

fn whole_seconds(total: f64) -> u64 {
    if total.is_finite() && total > 0.0 {
        total as u64
    } else {
        0
    }
}

fn minute_width(total_secs: u64) -> usize {
    let minutes = total_secs / 60;
    if minutes < 10 {
        1
    } else {
        minutes.to_string().len()
    }
}

fn format_clock(total_secs: u64, minute_width: usize) -> String {
    format!(
        "{:0width$}:{:02}",
        total_secs / 60,
        total_secs % 60,
        width = minute_width
    )
}

fn seek_thumb_moved(before: f64, after: f64, duration: f64) -> bool {
    let delta = (after - before).abs();
    if duration <= 0.0 {
        return delta >= 0.05;
    }
    delta / duration * f64::from(SEEK_TRACK_WIDTH) >= 1.0
}

fn fraction_from_position(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    let width = bounds.size.width;
    if width <= px(0.0) {
        return 0.0;
    }
    ((x - bounds.left()) / width).clamp(0.0, 1.0)
}

fn time_label(current: f64, duration: f64) -> String {
    let duration_secs = whole_seconds(duration);
    if duration_secs == 0 {
        return format_clock(whole_seconds(current), 1);
    }
    let width = minute_width(duration_secs);
    let current_secs = whole_seconds(current).min(duration_secs);
    format!(
        "{} / {}",
        format_clock(current_secs, width),
        format_clock(duration_secs, width)
    )
}

pub(crate) fn audio_time_label(current: f64, duration: f64) -> SharedString {
    SharedString::from(time_label(current, duration))
}

#[cfg(test)]
mod tests {
    use super::{fraction_from_position, time_label};
    use gpui::{Bounds, point, px, size};

    #[test]
    fn the_total_is_hidden_until_it_is_known() {
        assert_eq!(time_label(0.0, 0.0), "0:00");
        assert_eq!(time_label(5.0, 0.0), "0:05");
        assert_eq!(time_label(5.0, 222.0), "0:05 / 3:42");
    }

    #[test]
    fn the_clock_keeps_one_width_for_the_whole_clip() {
        let short = time_label(1.0, 90.0);
        assert_eq!(short, "0:01 / 1:30");
        assert_eq!(time_label(8.0, 90.0).len(), short.len());
        assert_eq!(time_label(11.0, 90.0).len(), short.len());
        let long = time_label(5.0, 700.0);
        assert_eq!(long, "00:05 / 11:40");
        assert_eq!(time_label(65.0, 700.0).len(), long.len());
        assert_eq!(time_label(600.0, 700.0).len(), long.len());
    }

    #[test]
    fn a_click_on_the_track_maps_to_a_fraction_of_its_width() {
        let bounds = Bounds {
            origin: point(px(10.), px(0.)),
            size: size(px(100.), px(14.)),
        };
        assert!((fraction_from_position(bounds, px(10.)) - 0.0).abs() < f32::EPSILON);
        assert!((fraction_from_position(bounds, px(60.)) - 0.5).abs() < f32::EPSILON);
        assert!((fraction_from_position(bounds, px(200.)) - 1.0).abs() < f32::EPSILON);
    }
}
