mod gif_panel;
mod sound_panel;
mod sticker_panel;

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Subscription, Window, div, prelude::*, px,
};

use gif_panel::{GifPanel, GifPanelEvent};
use sound_panel::{SoundPanel, SoundPanelEvent};
use sticker_panel::{StickerPanel, StickerPanelEvent};

use crate::chat::message::{ReactionPicker, ReactionPickerEvent};
use crate::components::primitives::{Icon, IconName, Input, InputEvent, InputState};
use crate::theme::ActiveTheme;

const PANEL_W: f32 = 500.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SubPanel {
    Gifs,
    Stickers,
    Emoji,
    Sounds,
}

pub enum GifStickerEmojiEvent {
    Emoji {
        emoji_id: String,
        emoji: String,
    },
    Sticker {
        url: String,
        filename: String,
    },
    Gif {
        url: String,
        width: u32,
        height: u32,
    },
    Sound {
        url: String,
        filename: String,
    },
}

pub struct GifStickerEmojiPopup {
    focus_handle: FocusHandle,
    active: SubPanel,
    locale: String,
    search: Entity<InputState>,
    emoji: Option<Entity<ReactionPicker>>,
    sticker: Option<Entity<StickerPanel>>,
    gif: Option<Entity<GifPanel>>,
    sound: Option<Entity<SoundPanel>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<GifStickerEmojiEvent> for GifStickerEmojiPopup {}
impl EventEmitter<DismissEvent> for GifStickerEmojiPopup {}

impl Focusable for GifStickerEmojiPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GifStickerEmojiPopup {
    pub fn new(
        active: SubPanel,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let input_bg: gpui::Hsla = cx.theme().tokens.theme_input.into();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder_for(active, &locale))
                .height(px(40.))
                .radius(px(6.))
                .text_size(px(14.))
                .bg(input_bg)
                .borderless()
                .padding_right(px(36.))
        });
        let mut subs = Vec::new();
        subs.push(cx.subscribe(&search, |this, _input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let query = this.search.read(cx).value().to_string();
                this.apply_query(query, cx);
            }
        }));

        let mut this = Self {
            focus_handle,
            active,
            locale,
            search,
            emoji: None,
            sticker: None,
            gif: None,
            sound: None,
            _subs: subs,
        };
        this.ensure_active_panel(window, cx);
        this
    }

    fn ensure_active_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active {
            SubPanel::Emoji => {
                if self.emoji.is_none() {
                    let emoji = cx.new(|cx| ReactionPicker::new_hosted(window, cx));
                    self._subs
                        .push(cx.subscribe(&emoji, |_this, _emoji, event, cx| {
                            let ReactionPickerEvent::Picked { emoji_id, emoji } = event;
                            cx.emit(GifStickerEmojiEvent::Emoji {
                                emoji_id: emoji_id.clone(),
                                emoji: emoji.clone(),
                            });
                        }));
                    self.emoji = Some(emoji);
                }
            }
            SubPanel::Stickers => {
                if self.sticker.is_none() {
                    let locale = self.locale.clone();
                    let sticker = cx.new(|cx| StickerPanel::new(locale, cx));
                    self._subs
                        .push(cx.subscribe(&sticker, |_this, _sticker, event, cx| {
                            let StickerPanelEvent::Picked { url, filename } = event;
                            cx.emit(GifStickerEmojiEvent::Sticker {
                                url: url.clone(),
                                filename: filename.clone(),
                            });
                        }));
                    self.sticker = Some(sticker);
                }
            }
            SubPanel::Gifs => {
                if self.gif.is_none() {
                    let locale = self.locale.clone();
                    let gif = cx.new(|cx| GifPanel::new(locale, cx));
                    self._subs.push(cx.subscribe_in(
                        &gif,
                        window,
                        |this, _gif, event, window, cx| match event {
                            GifPanelEvent::Picked { url, width, height } => {
                                cx.emit(GifStickerEmojiEvent::Gif {
                                    url: url.clone(),
                                    width: *width,
                                    height: *height,
                                });
                            }
                            GifPanelEvent::SetSearch { term } => {
                                this.search.update(cx, |input, cx| {
                                    input.set_value(term.clone(), window, cx)
                                });
                            }
                        },
                    ));
                    self.gif = Some(gif);
                }
            }
            SubPanel::Sounds => {
                if self.sound.is_none() {
                    let locale = self.locale.clone();
                    let sound = cx.new(|cx| SoundPanel::new(locale, cx));
                    self._subs
                        .push(cx.subscribe(&sound, |_this, _sound, event, cx| {
                            let SoundPanelEvent::Picked { url, filename } = event;
                            cx.emit(GifStickerEmojiEvent::Sound {
                                url: url.clone(),
                                filename: filename.clone(),
                            });
                        }));
                    self.sound = Some(sound);
                }
            }
        }
    }

    pub fn active_tab(&self) -> SubPanel {
        self.active
    }

    fn apply_query(&mut self, query: String, cx: &mut Context<Self>) {
        match self.active {
            SubPanel::Emoji => {
                if let Some(emoji) = &self.emoji {
                    emoji.update(cx, |p, cx| p.set_query(query, cx));
                }
            }
            SubPanel::Stickers => {
                if let Some(sticker) = &self.sticker {
                    sticker.update(cx, |p, cx| p.set_query(query, cx));
                }
            }
            SubPanel::Gifs => {
                if let Some(gif) = &self.gif {
                    gif.update(cx, |p, cx| p.set_query(query, cx));
                }
            }
            SubPanel::Sounds => {
                if let Some(sound) = &self.sound {
                    sound.update(cx, |p, cx| p.set_query(query, cx));
                }
            }
        }
    }

    pub fn set_tab(&mut self, tab: SubPanel, window: &mut Window, cx: &mut Context<Self>) {
        if self.active == tab {
            return;
        }
        let preserve = matches!(
            (self.active, tab),
            (
                SubPanel::Stickers | SubPanel::Emoji,
                SubPanel::Stickers | SubPanel::Emoji
            )
        );
        if self.active == SubPanel::Sounds {
            sound_panel::stop_preview(cx);
        }
        self.active = tab;
        self.ensure_active_panel(window, cx);
        if !preserve {
            self.search.update(cx, |input, cx| input.clear(cx));
        }
        let placeholder = placeholder_for(tab, &self.locale);
        self.search
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        let query = self.search.read(cx).value().to_string();
        self.apply_query(query, cx);
        cx.notify();
    }
}

impl Render for GifStickerEmojiPopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entity = cx.entity();
        let viewport_h = window.viewport_size().height;
        let panel_h = (viewport_h - px(88.)).min(px(512.)).max(px(400.));

        let mut tab_bar = div().flex().flex_row().mt_3().pt_1();
        for (tab, id, key) in [
            (SubPanel::Gifs, "gse-tab-gifs", "common.gifs"),
            (SubPanel::Stickers, "gse-tab-stickers", "common.stickers"),
            (SubPanel::Emoji, "gse-tab-emojis", "common.emojis"),
            (SubPanel::Sounds, "gse-tab-sounds", "common.sounds"),
        ] {
            let active = self.active == tab;
            let label = mezon_i18n::t(&self.locale, key);
            let ent = entity.clone();
            tab_bar = tab_bar.child(
                div()
                    .id(id)
                    .px_2()
                    .mx_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(14.))
                    .when(active, |s| {
                        s.font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_secondary)
                    })
                    .when(!active, |s| s.text_color(theme.tokens.text_theme_primary))
                    .child(label)
                    .on_click(move |_event, window, cx| {
                        ent.update(cx, |this, cx| this.set_tab(tab, window, cx));
                    }),
            );
        }

        let search_row = div().pt_4().pl_2().pr_4().child(
            div()
                .relative()
                .w_full()
                .child(Input::new(&self.search))
                .child(
                    div()
                        .absolute()
                        .right(px(10.))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .child(
                            Icon::new(IconName::Search)
                                .size(px(18.))
                                .text_color(theme.tokens.text_theme_primary),
                        ),
                ),
        );

        let content = match self.active {
            SubPanel::Emoji => self.emoji.clone().map(|p| p.into_any_element()),
            SubPanel::Stickers => self.sticker.clone().map(|p| p.into_any_element()),
            SubPanel::Gifs => self.gif.clone().map(|p| p.into_any_element()),
            SubPanel::Sounds => self.sound.clone().map(|p| p.into_any_element()),
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(PANEL_W))
            .h(panel_h)
            .flex()
            .flex_col()
            .rounded_lg()
            .overflow_hidden()
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.tokens.border_primary)
                    .pb_4()
                    .child(tab_bar)
                    .child(search_row),
            )
            .child(div().w_full().flex_1().min_h_0().children(content))
    }
}

fn placeholder_for(tab: SubPanel, locale: &str) -> &'static str {
    let key = match tab {
        SubPanel::Gifs => "message.findThePerfectGif",
        SubPanel::Stickers => "message.findThePerfectSticker",
        SubPanel::Emoji => "message.findThePerfectReaction",
        SubPanel::Sounds => "message.findThePerfectSound",
    };
    mezon_i18n::t(locale, key)
}
