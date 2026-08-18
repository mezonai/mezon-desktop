use std::sync::Arc;
use std::time::Duration;

use futures::AsyncReadExt;
use gpui::{
    App, ClickEvent, Context, ElementId, Rgba, SharedString, Task, Window, div,
    http_client::HttpClient, prelude::*, px,
};
use mezon_audio::{AudioPlayer, DecodedPcm, PcmStream};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, Sizable, Size, Spinner};

const AUDIO_OUTPUT_TOAST_KEY: &str = "audio-output-unavailable";
const AUDIO_FETCH_MAX_BYTES: usize = 64 * 1024 * 1024;
const AUDIO_TICK_INTERVAL: Duration = Duration::from_millis(250);
const AUDIO_FETCH_CHUNK: usize = 64 * 1024;
const AUDIO_FETCH_QUEUE: usize = 8;

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

pub struct AudioActivation {
    pub url: SharedString,
    pub duration: f64,
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
    server_duration: f64,
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
            server_duration: duration,
            time_label: SharedString::from(time_label(0.0, duration)),
            last_label_seconds: (0, whole_seconds(duration)),
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
            if self.want_play {
                player.play();
                self.restart_tick(cx);
            }
        }
        cx.notify();
    }

    fn on_pcm_ready(&mut self, pcm: DecodedPcm, cx: &mut Context<Self>) {
        self.state = LoadState::Ready;
        if let Some(player) = &self.player {
            player.set_data(pcm);
            if self.want_play {
                player.play();
                self.restart_tick(cx);
            }
        }
        cx.notify();
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
                cx.background_executor().timer(AUDIO_TICK_INTERVAL).await;
                let keep_going = this.update(cx, |view, cx| view.tick(cx)).unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(player) = &self.player else {
            return false;
        };
        if cx.active_window().is_none() {
            return player.is_playing();
        }
        let finished = player.finished();
        if finished {
            player.pause();
        }
        let position = player.position_secs();
        let duration = self.effective_duration();
        let seconds = (whole_seconds(position), whole_seconds(duration));
        let mut changed = false;
        if seconds != self.last_label_seconds {
            self.last_label_seconds = seconds;
            self.time_label = SharedString::from(time_label(position, duration));
            changed = true;
        }
        if changed || finished {
            cx.notify();
        }
        player.is_playing()
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
        match &self.player {
            Some(player) if self.is_ready() => {
                if player.is_playing() {
                    player.pause();
                    self.tick_task = None;
                } else {
                    player.play();
                    self.restart_tick(cx);
                }
            }
            _ => {
                self.want_play = !self.want_play;
            }
        }
        cx.notify();
    }
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
        audio_pill(
            "audio-play",
            "audio-download",
            play_icon,
            self.time_label.clone(),
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

pub(crate) fn audio_pill(
    play_id: impl Into<ElementId>,
    download_id: impl Into<ElementId>,
    play_icon: IconName,
    time_label: SharedString,
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
                .justify_between()
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
                        .ml_2()
                        .text_size(px(14.))
                        .whitespace_nowrap()
                        .child(time_label),
                )
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

pub(crate) fn audio_sending_pill(duration: f64) -> gpui::AnyElement {
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
                        .child(audio_time_label(0.0, duration)),
                ),
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

fn format_mmss(total: f64) -> String {
    let seconds = whole_seconds(total);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn time_label(current: f64, duration: f64) -> String {
    if whole_seconds(duration) == 0 {
        return format_mmss(current);
    }
    format!("{} / {}", format_mmss(current), format_mmss(duration))
}

pub(crate) fn audio_time_label(current: f64, duration: f64) -> SharedString {
    SharedString::from(time_label(current, duration))
}

#[cfg(test)]
mod tests {
    use super::time_label;

    #[test]
    fn the_total_is_hidden_until_it_is_known() {
        assert_eq!(time_label(0.0, 0.0), "0:00");
        assert_eq!(time_label(5.0, 0.0), "0:05");
        assert_eq!(time_label(5.0, 222.0), "0:05 / 3:42");
    }
}
