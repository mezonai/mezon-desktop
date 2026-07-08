use std::collections::HashSet;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, uniform_list,
};
use mezon_store::{
    ChannelEvent, ChannelId, ChannelList, ChannelType, ClanId, ClanList, DirectEvent,
    DirectMessageStore, ForwardTarget, MessageId, MessagesStore,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const ROW_PX: f32 = 44.;

struct ForwardOption {
    channel_id: ChannelId,
    label: SharedString,
    avatar: SharedString,
    is_dm: bool,
    filter_key: String,
    target: ForwardTarget,
}

fn build_options(cx: &App) -> Vec<ForwardOption> {
    let mut options = Vec::new();

    if let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id {
        let channel_list = ChannelList::global(cx);
        for category in channel_list.read(cx).categories_for_clan(clan_id) {
            for channel in &category.channels {
                let (channel_type, mode) = match channel.channel_type {
                    ChannelType::Text => (1, 2),
                    ChannelType::Thread => (7, 6),
                    _ => continue,
                };
                options.push(ForwardOption {
                    channel_id: channel.id,
                    label: SharedString::from(channel.name.clone()),
                    avatar: SharedString::default(),
                    is_dm: false,
                    filter_key: channel.name.to_lowercase(),
                    target: ForwardTarget {
                        clan_id: channel.clan_id,
                        channel_id: channel.id,
                        channel_type,
                        mode,
                        is_public: !channel.private,
                    },
                });
            }
        }
    }

    for dm in DirectMessageStore::global(cx).read(cx).channels() {
        let avatar = if dm.avatar.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &dm.avatar))
        };
        options.push(ForwardOption {
            channel_id: dm.id,
            label: SharedString::from(dm.label.clone()),
            avatar,
            is_dm: true,
            filter_key: dm.label.to_lowercase(),
            target: ForwardTarget {
                clan_id: ClanId(0),
                channel_id: dm.id,
                channel_type: dm.kind.channel_type(),
                mode: dm.kind.stream_mode(),
                is_public: false,
            },
        });
    }

    options
}

pub struct ForwardMessageModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    message_ids: Vec<MessageId>,
    options: Vec<ForwardOption>,
    filtered: Vec<usize>,
    selected: HashSet<ChannelId>,
    search_input: Entity<InputState>,
    note_input: Entity<InputState>,
    submitting: bool,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _search_sub: Subscription,
    _channel_sub: Subscription,
    _dm_sub: Subscription,
}

impl Focusable for ForwardMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ForwardMessageModal {
    pub fn open(
        message_ids: Vec<MessageId>,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        if message_ids.is_empty() {
            return;
        }
        DirectMessageStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));

        let search_ph =
            mezon_i18n::t(&locale, "forwardMessage.modal.searchPlaceholder").to_string();
        let note_ph =
            mezon_i18n::t(&locale, "forwardMessage.modal.additionalMessagePlaceholder").to_string();

        let view = cx.new(|cx| {
            let search_input =
                cx.new(|cx| InputState::new(window, cx).placeholder(search_ph.clone()));
            let note_input = cx.new(|cx| InputState::new(window, cx).placeholder(note_ph.clone()));
            let search_sub = cx.subscribe(
                &search_input,
                |this: &mut Self, _input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.recompute_filtered(cx);
                        cx.notify();
                    }
                },
            );
            let channel_sub = cx.subscribe(
                &ChannelList::global(cx),
                |this: &mut Self, _, _e: &ChannelEvent, cx| {
                    this.rebuild_options(cx);
                    cx.notify();
                },
            );
            let dm_sub = cx.subscribe(
                &DirectMessageStore::global(cx),
                |this: &mut Self, _, _e: &DirectEvent, cx| {
                    this.rebuild_options(cx);
                    cx.notify();
                },
            );
            let image_cache = crate::image_cache::shared_avatar_cache(cx);
            let options = build_options(cx);
            let filtered = (0..options.len()).collect();
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                message_ids,
                options,
                filtered,
                selected: HashSet::new(),
                search_input,
                note_input,
                submitting: false,
                scroll: UniformListScrollHandle::new(),
                image_cache,
                _search_sub: search_sub,
                _channel_sub: channel_sub,
                _dm_sub: dm_sub,
            }
        });
        let focus_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn rebuild_options(&mut self, cx: &App) {
        self.options = build_options(cx);
        self.selected
            .retain(|id| self.options.iter().any(|o| o.channel_id == *id));
        self.recompute_filtered(cx);
    }

    fn recompute_filtered(&mut self, cx: &App) {
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        self.filtered = self
            .options
            .iter()
            .enumerate()
            .filter(|(_, o)| query.is_empty() || o.filter_key.contains(&query))
            .map(|(ix, _)| ix)
            .collect();
    }

    fn toggle(&mut self, channel_id: ChannelId) {
        if !self.selected.remove(&channel_id) {
            self.selected.insert(channel_id);
        }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.submitting || self.selected.is_empty() {
            return;
        }
        let targets: Vec<ForwardTarget> = self
            .options
            .iter()
            .filter(|o| self.selected.contains(&o.channel_id))
            .map(|o| o.target.clone())
            .collect();
        if targets.is_empty() {
            return;
        }
        let note = {
            let value = self.note_input.read(cx).value().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        self.submitting = true;
        let ids = self.message_ids.clone();
        let last = ids.len().saturating_sub(1);
        MessagesStore::global(cx).update(cx, |store, cx| {
            for (index, id) in ids.iter().enumerate() {
                let note_for = if index == last { note.clone() } else { None };
                store.forward(*id, targets.clone(), note_for, cx);
            }
        });
        Self::close(cx);
    }
}

impl Render for ForwardMessageModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let entity = cx.entity();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_4()
            .border_b_1()
            .border_color(theme.tokens.border_theme_primary)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(mezon_i18n::t(&locale, "forwardMessage.modal.title")),
            )
            .child(
                div()
                    .id("forward-close")
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(18.))
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            );

        let search = div().px_4().pt_3().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .rounded_lg()
                .bg(theme.tokens.bg_surface)
                .border_1()
                .border_color(theme.tokens.border_theme_primary)
                .px_3()
                .py_2()
                .child(
                    Icon::new(IconName::Search)
                        .size_4()
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(
                    Input::new(&self.search_input)
                        .w_full()
                        .text_sm()
                        .text_color(theme.tokens.text_theme_message),
                ),
        );

        let count = self.filtered.len();
        let list_entity = entity.clone();
        let list = uniform_list("forward-target-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let this = list_entity.read(cx);
            range
                .map(|ix| match this.filtered.get(ix) {
                    Some(&option_ix) => match this.options.get(option_ix) {
                        Some(option) => {
                            let selected = this.selected.contains(&option.channel_id);
                            render_option_row(&theme, option, selected, &list_entity)
                        }
                        None => div().h(px(ROW_PX)).into_any_element(),
                    },
                    None => div().h(px(ROW_PX)).into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();

        let body = if count == 0 {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_sm()
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(&locale, "forwardMessage.modal.noResults"))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .px_2()
                .child(list)
                .into_any_element()
        };

        let note = div().px_4().pt_2().child(
            div()
                .flex()
                .items_center()
                .rounded_lg()
                .bg(theme.tokens.bg_surface)
                .border_1()
                .border_color(theme.tokens.border_theme_primary)
                .px_3()
                .py_2()
                .child(
                    Input::new(&self.note_input)
                        .w_full()
                        .text_sm()
                        .text_color(theme.tokens.text_theme_message),
                ),
        );

        let send_entity = entity.clone();
        let send_disabled = self.submitting || self.selected.is_empty();
        let send_label = if self.submitting {
            mezon_i18n::t(&locale, "forwardMessage.modal.sending")
        } else {
            mezon_i18n::t(&locale, "forwardMessage.modal.send")
        };

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_3()
            .p_4()
            .child(
                div()
                    .id("forward-cancel")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .child(mezon_i18n::t(&locale, "forwardMessage.modal.cancel"))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
            )
            .child(
                Button::new("forward-send")
                    .primary()
                    .label(send_label)
                    .loading(self.submitting)
                    .disabled(send_disabled)
                    .on_click(move |_: &ClickEvent, _window, cx| {
                        send_entity.update(cx, |this, cx| this.send(cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            .occlude()
            .image_cache(self.image_cache.clone())
            .w(px(460.))
            .max_h(gpui::relative(0.8))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(search)
            .child(body)
            .child(note)
            .child(footer)
    }
}

fn render_option_row(
    theme: &Theme,
    option: &ForwardOption,
    selected: bool,
    entity: &Entity<ForwardMessageModal>,
) -> AnyElement {
    let ent = entity.clone();
    let channel_id = option.channel_id;

    let leading: AnyElement = if option.is_dm {
        if option.avatar.is_empty() {
            div()
                .size(px(28.))
                .rounded_full()
                .flex_shrink_0()
                .bg(theme.tokens.bg_active_member_channel)
                .into_any_element()
        } else {
            img(option.avatar.clone())
                .size(px(28.))
                .rounded_full()
                .flex_shrink_0()
                .into_any_element()
        }
    } else {
        div()
            .size(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(IconName::Hashtag)
                    .size_4()
                    .text_color(theme.tokens.text_theme_primary),
            )
            .into_any_element()
    };

    let mut row = div()
        .id(("forward-option", channel_id.get() as usize))
        .h(px(ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme.tokens.bg_item_theme_hover));
    if selected {
        row = row.bg(theme.tokens.bg_active_member_channel);
    }

    row.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .min_w_0()
            .child(leading)
            .child(
                div()
                    .truncate()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(option.label.clone()),
            ),
    )
    .when(selected, |el| {
        el.child(
            Icon::new(IconName::Check)
                .size_4()
                .flex_shrink_0()
                .text_color(theme.tokens.text_secondary),
        )
    })
    .on_click(move |_: &ClickEvent, _window, cx| {
        ent.update(cx, |this, cx| {
            this.toggle(channel_id);
            cx.notify();
        });
    })
    .into_any_element()
}
