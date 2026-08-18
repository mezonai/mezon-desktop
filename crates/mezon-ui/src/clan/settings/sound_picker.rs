use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions,
    SharedString, Subscription, Task, Window, div, prelude::*, px, relative,
};
use mezon_audio::{AudioPlayer, DecodedPcm};
use mezon_store::{
    ClanId, EMOTICON_SHORTNAME_MAX, MAX_SOUND_BYTES, Settings, StickerStore, VoiceStore,
    is_valid_emoticon_shortname, validate_sound_file,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const FORM_CONTROL_H: f32 = 34.0;
const PREVIEW_TICK: Duration = Duration::from_millis(250);

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

fn clone_pcm(pcm: &DecodedPcm) -> DecodedPcm {
    DecodedPcm {
        samples: Arc::clone(&pcm.samples),
        channels: pcm.channels,
        sample_rate: pcm.sample_rate,
    }
}

fn audio_time_label(current: f64, duration: f64) -> SharedString {
    let duration_secs = duration.max(0.0).round() as u64;
    let label = if duration_secs == 0 {
        format_mmss(current)
    } else {
        format!("{} / {}", format_mmss(current), format_mmss(duration))
    };
    label.into()
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
    local_player: Option<AudioPlayer>,
    submitting: bool,
    _name_sub: Subscription,
    _voice_observe: Subscription,
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
            local_player: None,
            submitting: false,
            _name_sub: name_sub,
            _voice_observe: voice_observe,
            _submit_task: None,
            _decode_task: None,
            _preview_tick: None,
        }
    }

    fn can_preview(&self) -> bool {
        self.picked_path.is_some()
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
                return (0.0, pcm.duration_secs());
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
        let Some(pcm) = self.local_pcm.as_ref() else {
            return;
        };
        let Ok(player) = AudioPlayer::new() else {
            tracing::warn!("audio output unavailable");
            return;
        };
        player.set_data(clone_pcm(pcm));
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
                    mezon_audio::decode_audio(bytes).map_err(|err| err.to_string())
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
                    Ok(pcm) => {
                        this.local_pcm = Some(pcm);
                    }
                    Err(err) => {
                        tracing::warn!("sound picker decode failed: {err}");
                        this.local_pcm = None;
                    }
                }
                cx.notify();
            });
            if let Some(code) = error_code {
                show_error(cx, sound_error_message(&locale, code));
            }
        }));
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
            .min_w(px(0.0))
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
                    .min_w(px(0.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme.border)
                    .overflow_hidden()
                    .when(progress > 0.0, |track| {
                        track.child(
                            div()
                                .h_full()
                                .w(relative(progress))
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
        self.picked_path.is_some()
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
        self._submit_task = Some(cx.spawn(async move |this, cx| {
            let paths = match rx.await {
                Ok(Ok(Some(p))) => p,
                _ => return,
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
                this.stop_local_preview(cx);
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

        self.submitting = true;
        cx.notify();
        let clan_id = self.clan_id;
        let editing = self.editing.clone();
        let picked_path = self.picked_path.clone();

        self._submit_task = Some(cx.spawn(async move |this, cx| {
            let task = match (editing.as_ref(), picked_path.as_ref()) {
                (Some(target), _) => cx.update(|cx| {
                    StickerStore::global(cx).update(cx, |store, cx| {
                        store.update_sound(&target.id, clan_id, &target.source, &name, cx)
                    })
                }),
                (None, Some(path)) => cx.update(|cx| {
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

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| this.close(cx)))
            .w(px(520.))
            .gap_4()
            .p(px(24.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_start()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_secondary)
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        div()
                            .id("sound-picker-close")
                            .p_1()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(theme.text_secondary),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(preview_label.to_uppercase()),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .p(px(16.0))
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg_secondary)
                            .items_center()
                            .gap(px(8.0))
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
                                            cx.listener(|this, _, _, cx| this.toggle_preview(cx)),
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
                                        .text_color(theme.text_primary),
                                    ),
                            )
                            .child(self.render_preview_playbar(previewing, &theme, cx)),
                    ),
            )
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
                                    .text_color(theme.text_secondary)
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
                                                    .text_color(theme.text_primary)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(self.file_label.clone()),
                                            ),
                                    )
                                    .child({
                                        let mut browse_btn =
                                            Button::new("sound-browse").label(browse);
                                        if is_edit {
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
                                            .text_color(theme.text_secondary)
                                            .child(name_label.to_uppercase()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(if name_len > 40 {
                                                theme.danger_text
                                            } else {
                                                theme.text_muted
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
