use gpui::{
    AnyElement, App, Context, Entity, FontWeight, ListHorizontalSizingBehavior, Render,
    ScrollHandle, SharedString, Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
    relative, size, uniform_list,
};
use mezon_store::{
    BadgeService, ClanId, ClanMembersEvent, ClanMembersStore, ClanSound, PermissionStore, Settings,
    StickerEvent, StickerStore, UserId, VoiceStore,
};

use super::sound_picker::{SoundEditTarget, SoundPicker, SoundPickerEvent};
use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Sizable, Size, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::download::save_with_progress_toast;

const CARD_WIDTH: f32 = 236.0;
const SOUND_CARD_HEIGHT: f32 = 136.0;
const SOUND_CONTENT_MAX_WIDTH: f32 = 740.0;
const SOUND_GRID_GAP_X: f32 = 16.0;
const SOUND_GRID_GAP_Y: f32 = 16.0;
const SOUND_GRID_MIN_COLUMNS: u16 = 1;
const SOUND_GRID_MAX_COLUMNS: u16 = 3;
const SOUND_ROW_HEIGHT: f32 = SOUND_CARD_HEIGHT + SOUND_GRID_GAP_Y;

#[derive(Clone)]
struct SoundCardData {
    id: SharedString,
    shortname: SharedString,
    src: SharedString,
    download_name: SharedString,
    creator_name: Option<SharedString>,
    creator_avatar: Option<SharedString>,
    can_manage: bool,
}

fn sound_grid_columns() -> usize {
    let mut columns = SOUND_GRID_MIN_COLUMNS;
    for column_count in SOUND_GRID_MIN_COLUMNS..=SOUND_GRID_MAX_COLUMNS {
        let row_width =
            column_count as f32 * CARD_WIDTH + (column_count - 1) as f32 * SOUND_GRID_GAP_X;
        if row_width <= SOUND_CONTENT_MAX_WIDTH {
            columns = column_count;
        } else {
            break;
        }
    }
    usize::from(columns)
}

fn sound_grid_gap_x() -> f32 {
    let columns = sound_grid_columns() as f32;
    let gaps = (columns - 1.0).max(1.0);
    ((SOUND_CONTENT_MAX_WIDTH - columns * CARD_WIDTH) / gaps).max(SOUND_GRID_GAP_X)
}

fn format_mmss(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
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

fn sound_download_name(shortname: &str, url: &str) -> SharedString {
    url.split('/')
        .next_back()
        .filter(|name| !name.is_empty())
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from(format!("{shortname}.mp3")))
}

fn section_heading_xs(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    let text = text.into().to_string().to_uppercase();
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text_secondary)
        .child(text)
}

fn body_text(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .text_sm()
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.tokens.text_theme_primary)
        .child(text.into())
}

pub struct SoundSettingPage {
    clan_id: ClanId,
    clan_id_str: String,
    settings: Entity<Settings>,
    sticker_store: Entity<StickerStore>,
    scroll: ScrollHandle,
    grid_scroll: UniformListScrollHandle,
    grid_cells: Vec<SoundCardData>,
    grid_columns: usize,
    _sticker_sub: Subscription,
    _sound_observe: Subscription,
    _voice_observe: Subscription,
    _members_observe: Subscription,
    _perm_observe: Subscription,
    _modal_sub: Option<Subscription>,
}

impl SoundSettingPage {
    pub fn new(clan_id: ClanId, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let sticker_store = StickerStore::global(cx);
        sticker_store.update(cx, |store, cx| store.ensure_loaded(cx));
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        PermissionStore::global(cx).update(cx, |store, cx| {
            store.load_clan_permissions(clan_id, cx);
        });

        let clan_id_str = clan_id.get().to_string();
        let sticker_sub = cx.subscribe(&sticker_store, |this, _, _: &StickerEvent, cx| {
            this.rebuild_sounds(cx);
            cx.notify();
        });
        let sound_observe = cx.observe(&sticker_store, |this, _, cx| {
            let count = StickerStore::global(cx)
                .read(cx)
                .sounds_for_clan(&this.clan_id_str)
                .len();
            if count != this.sound_count() {
                this.rebuild_sounds(cx);
                cx.notify();
            }
        });
        let voice_observe = cx.observe(&VoiceStore::global(cx), |_, _, cx| cx.notify());
        let members_observe = cx.subscribe(&ClanMembersStore::global(cx), |this, _, event, cx| {
            if matches!(event, ClanMembersEvent::Changed { clan_id } if *clan_id == this.clan_id) {
                this.rebuild_sounds(cx);
                cx.notify();
            }
        });
        let perm_observe = cx.observe(&PermissionStore::global(cx), |this, _, cx| {
            this.rebuild_sounds(cx);
            cx.notify();
        });

        let mut this = Self {
            clan_id,
            clan_id_str,
            settings,
            sticker_store,
            scroll: ScrollHandle::new(),
            grid_scroll: UniformListScrollHandle::new(),
            grid_cells: Vec::new(),
            grid_columns: sound_grid_columns(),
            _sticker_sub: sticker_sub,
            _sound_observe: sound_observe,
            _voice_observe: voice_observe,
            _members_observe: members_observe,
            _perm_observe: perm_observe,
            _modal_sub: None,
        };
        this.rebuild_sounds(cx);
        this
    }

    pub fn release(&mut self, cx: &mut Context<Self>) {
        self._modal_sub.take();
        VoiceStore::global(cx).update(cx, |store, cx| store.stop_sound_preview(cx));
    }

    fn sound_count(&self) -> usize {
        self.grid_cells.len()
    }

    fn grid_row_count(&self) -> usize {
        self.grid_cells.len().div_ceil(self.grid_columns)
    }

    fn rebuild_sounds(&mut self, cx: &App) {
        self.grid_columns = sound_grid_columns();
        let sounds = self
            .sticker_store
            .read(cx)
            .sounds_for_clan(&self.clan_id_str);
        self.grid_cells = sounds
            .iter()
            .map(|sound| self.card_data(sound, cx))
            .collect();
    }

    fn can_manage_sound(&self, creator_id: &str, cx: &App) -> bool {
        let perms = PermissionStore::global(cx)
            .read(cx)
            .clan_settings_permissions(self.clan_id, cx);
        if perms.has_manage_clan {
            return true;
        }
        let current = BadgeService::global(cx)
            .read(cx)
            .current_user_id(cx)
            .map(|id| id.get().to_string());
        current.is_some_and(|uid| uid == creator_id)
    }

    fn creator_display(
        &self,
        creator_id: &str,
        cx: &App,
    ) -> (Option<SharedString>, Option<SharedString>) {
        if creator_id.is_empty() {
            return (None, None);
        }
        let Some(user_id) = creator_id.parse::<UserId>().ok() else {
            return (None, None);
        };
        let Some(member) = ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, user_id)
        else {
            return (None, None);
        };
        let name = SharedString::from(member.name().to_string());
        let avatar = if member.avatar().is_empty() {
            None
        } else {
            Some(SharedString::from(crate::util::imgproxy::avatar_url(
                cx,
                member.avatar(),
            )))
        };
        (Some(name), avatar)
    }

    fn card_data(&self, sound: &ClanSound, cx: &App) -> SoundCardData {
        let (creator_name, creator_avatar) = self.creator_display(&sound.creator_id, cx);
        SoundCardData {
            id: SharedString::from(sound.id.clone()),
            shortname: SharedString::from(sound.shortname.clone()),
            src: SharedString::from(sound.src.clone()),
            download_name: sound_download_name(&sound.shortname, &sound.src),
            creator_name,
            creator_avatar,
            can_manage: self.can_manage_sound(&sound.creator_id, cx),
        }
    }

    fn open_picker(
        &mut self,
        editing: Option<SoundEditTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clan_id = self.clan_id;
        let settings = self.settings.clone();
        let modal = cx.new(|cx| SoundPicker::new(clan_id, editing, settings, window, cx));
        self._modal_sub = Some(cx.subscribe(&modal, |this, _, _: &SoundPickerEvent, cx| {
            this._modal_sub = None;
            cx.notify();
        }));
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
    }

    fn open_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(None, window, cx);
    }

    fn toggle_preview(&mut self, url: String, cx: &mut Context<Self>) {
        VoiceStore::global(cx).update(cx, |store, cx| {
            store.toggle_sound_preview(url, cx);
        });
    }

    fn confirm_delete_sound(
        &mut self,
        sound_id: SharedString,
        shortname: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.settings.read(cx).language.clone();
        let clan_id = self.clan_id;
        Shell::global(cx).update(cx, |shell, cx| {
            shell.confirm_delete_sound(clan_id, sound_id, shortname, &locale, window, cx);
        });
    }

    fn render_upload_card(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .p(px(16.0))
            .gap(px(16.0))
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.theme_setting_nav)
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(locale, "clanSoundSetting.main.uploadHere")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "clanSoundSetting.main.personalizeDescription",
                            )),
                    ),
            )
            .child(
                Button::new("sound-upload-open")
                    .label(mezon_i18n::t(locale, "clanSoundSetting.main.uploadSound"))
                    .primary()
                    .with_size(Size::Large)
                    .on_click(cx.listener(|this, _, window, cx| this.open_upload(window, cx))),
            )
    }

    fn render_empty_state(&self, locale: &str, theme: &Theme) -> impl IntoElement {
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_center()
            .py(px(40.0))
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(theme.border)
            .bg(theme.tokens.theme_setting_nav)
            .child(
                Icon::new(IconName::Speaker)
                    .size(px(40.0))
                    .text_color(theme.tokens.text_theme_primary)
                    .mb(px(8.0)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(
                        locale,
                        "clanSoundSetting.main.noSoundEffects",
                    )),
            )
    }

    fn render_grid(
        &self,
        locale: &str,
        theme: &Theme,
        entity: Entity<SoundSettingPage>,
    ) -> impl IntoElement {
        let row_count = self.grid_row_count();
        let list_entity = entity.clone();
        let grid_height = px(row_count as f32 * SOUND_ROW_HEIGHT);

        let grid_list = uniform_list(
            "clan-sound-settings-grid",
            row_count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                let page = list_entity.read(cx);
                range
                    .map(|row_ix| {
                        render_sound_grid_row(
                            row_ix,
                            &page.grid_cells,
                            page.grid_columns,
                            &theme,
                            list_entity.clone(),
                            cx,
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_item_size(size(px(SOUND_CONTENT_MAX_WIDTH), px(SOUND_ROW_HEIGHT)))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
        .track_scroll(&self.grid_scroll)
        .size_full();

        v_flex()
            .w_full()
            .max_w(px(SOUND_CONTENT_MAX_WIDTH))
            .gap(px(16.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Icon::new(IconName::Speaker)
                            .size(px(20.0))
                            .text_color(theme.tokens.text_theme_primary),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(
                                locale,
                                "clanSoundSetting.main.soundEffectList",
                            )),
                    ),
            )
            .child(
                div()
                    .id("clan-sound-settings-grid-container")
                    .w_full()
                    .h(grid_height)
                    .child(grid_list),
            )
    }
}

fn render_playbar(
    sound_id: &str,
    url: SharedString,
    download_name: SharedString,
    previewing: bool,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let voice = VoiceStore::global(cx).read(cx);
    let (position, duration) = if previewing {
        voice
            .sound_preview_timeline(url.as_ref())
            .unwrap_or_else(|| {
                (
                    0.0,
                    voice.cached_sound_duration(url.as_ref()).unwrap_or(0.0),
                )
            })
    } else {
        (
            0.0,
            voice.cached_sound_duration(url.as_ref()).unwrap_or(0.0),
        )
    };
    let progress = if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let time_label = audio_time_label(if previewing { position } else { 0.0 }, duration);
    let show_time = previewing || duration > 0.0;

    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(36.0))
        .px(px(10.0))
        .rounded_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg_secondary)
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
        .child(if show_time {
            div()
                .flex_shrink_0()
                .text_xs()
                .whitespace_nowrap()
                .text_color(theme.tokens.text_theme_primary)
                .child(time_label)
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!("sound-download-{sound_id}")))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .child(
                    Icon::new(IconName::Download)
                        .size(px(16.0))
                        .text_color(theme.tokens.text_theme_primary),
                )
                .on_click(move |_, _, cx| {
                    save_with_progress_toast(url.clone(), download_name.clone(), cx);
                })
                .into_any_element()
        })
}

fn render_sound_card(
    sound: &SoundCardData,
    theme: &Theme,
    entity: Entity<SoundSettingPage>,
    cx: &App,
) -> impl IntoElement {
    let url = sound.src.to_string();
    let previewing = VoiceStore::global(cx)
        .read(cx)
        .previewing_sound()
        .is_some_and(|active| active == url.as_str());
    let sound_id = sound.id.clone();
    let shortname = sound.shortname.clone();
    let download_url = sound.src.clone();
    let download_name = sound.download_name.clone();
    let play_url = url.clone();

    v_flex()
        .id(SharedString::from(format!("sound-card-{sound_id}")))
        .w(px(CARD_WIDTH))
        .h(px(SOUND_CARD_HEIGHT))
        .flex_shrink_0()
        .relative()
        .p(px(16.0))
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.tokens.theme_setting_nav)
        .child(
            div()
                .relative()
                .w_full()
                .mb(px(12.0))
                .child(
                    div()
                        .w_full()
                        .px(px(28.0))
                        .text_center()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(shortname.clone()),
                )
                .when(sound.can_manage, |el| {
                    let delete_entity = entity.clone();
                    let delete_id = sound_id.clone();
                    let delete_name = shortname.clone();
                    el.child(
                        div()
                            .absolute()
                            .top(px(-8.0))
                            .right(px(-8.0))
                            .id(SharedString::from(format!("sound-delete-{sound_id}")))
                            .size(px(24.0))
                            .rounded_full()
                            .bg(theme.tokens.theme_setting_primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(12.0))
                                    .text_color(theme.status_dnd),
                            )
                            .on_click(move |_, window, cx| {
                                delete_entity.update(cx, |this, cx| {
                                    this.confirm_delete_sound(
                                        delete_id.clone(),
                                        delete_name.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                }),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(px(8.0))
                .mb(px(8.0))
                .child(
                    div()
                        .id(SharedString::from(format!("sound-play-{sound_id}")))
                        .size(px(36.0))
                        .rounded_full()
                        .bg(theme.brand)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            Icon::new(if previewing {
                                IconName::AudioPause
                            } else {
                                IconName::AudioPlay
                            })
                            .size(px(16.0))
                            .text_color(theme.text_primary),
                        )
                        .on_click({
                            let play_entity = entity.clone();
                            let play_url = play_url.clone();
                            move |_, _, cx| {
                                play_entity.update(cx, |this, cx| {
                                    this.toggle_preview(play_url.clone(), cx);
                                });
                            }
                        }),
                )
                .child(render_playbar(
                    &sound_id,
                    download_url,
                    download_name,
                    previewing,
                    theme,
                    cx,
                )),
        )
        .when_some(sound.creator_name.clone(), |el, name| {
            let avatar = sound.creator_avatar.clone().unwrap_or_default();
            el.child(
                h_flex()
                    .w_full()
                    .max_w_full()
                    .justify_center()
                    .items_center()
                    .gap(px(4.0))
                    .mt(px(4.0))
                    .child({
                        let mut avatar_el = Avatar::new().name(name.clone()).size_px(px(16.0));
                        if !avatar.is_empty() {
                            avatar_el = avatar_el.src(avatar);
                        }
                        div().flex_shrink_0().child(avatar_el)
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .max_w(px(80.0))
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary)
                            .truncate()
                            .whitespace_nowrap()
                            .child(name),
                    ),
            )
        })
}

fn render_sound_grid_row(
    row_ix: usize,
    grid_cells: &[SoundCardData],
    grid_columns: usize,
    theme: &Theme,
    entity: Entity<SoundSettingPage>,
    cx: &App,
) -> AnyElement {
    let gap_x = sound_grid_gap_x();
    let start = row_ix * grid_columns;
    let end = (start + grid_columns).min(grid_cells.len());
    let mut row = h_flex()
        .w_full()
        .h(px(SOUND_ROW_HEIGHT))
        .gap_x(px(gap_x))
        .items_start();

    for sound in &grid_cells[start..end] {
        row = row.child(render_sound_card(sound, theme, entity.clone(), cx).into_any_element());
    }

    row.into_any_element()
}

impl Render for SoundSettingPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store_count = StickerStore::global(cx)
            .read(cx)
            .sounds_for_clan(&self.clan_id_str)
            .len();
        if store_count != self.sound_count() {
            self.rebuild_sounds(cx);
        } else if self.sound_count() == 0 {
            StickerStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        }

        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let entity = cx.entity();
        let sound_count = self.sound_count();

        let requirements_section = v_flex()
            .flex_shrink_0()
            .w_full()
            .pb(px(24.0))
            .border_b_1()
            .border_color(theme.border)
            .gap_2()
            .child(section_heading_xs(
                mezon_i18n::t(&locale, "clanSoundSetting.main.uploadInstructions"),
                &theme,
            ))
            .child(body_text(
                mezon_i18n::t(&locale, "clanSoundSetting.main.fileRequirements"),
                &theme,
            ));

        div().relative().size_full().min_h_0().child(
            div()
                .id("clan-sound-settings-scroll")
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(requirements_section)
                .child(
                    div()
                        .w_full()
                        .mt(px(16.0))
                        .child(self.render_upload_card(&locale, &theme, cx)),
                )
                .child(
                    div()
                        .w_full()
                        .mt(px(16.0))
                        .pb(px(60.0))
                        .child(if sound_count == 0 {
                            v_flex()
                                .w_full()
                                .gap(px(16.0))
                                .child(
                                    h_flex()
                                        .w_full()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            Icon::new(IconName::Speaker)
                                                .size(px(20.0))
                                                .text_color(theme.tokens.text_theme_primary),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(theme.text_secondary)
                                                .child(mezon_i18n::t(
                                                    &locale,
                                                    "clanSoundSetting.main.soundEffectList",
                                                )),
                                        ),
                                )
                                .child(self.render_empty_state(&locale, &theme))
                                .into_any_element()
                        } else {
                            self.render_grid(&locale, &theme, entity).into_any_element()
                        }),
                ),
        )
    }
}
