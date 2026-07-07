mod filter;
mod groups;
mod items;

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, uniform_list, App, Context, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, ScrollStrategy, SharedString, Subscription, Task,
    UniformListScrollHandle, WeakEntity, Window,
};
use mezon_store::{
    AuthState, ChannelId, ChannelList, ClanId, ClanList, ClanMembersStore, DirectKind,
    DirectMessageStore, LoginStore, Settings, UserId, UsersByUserStore,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::app::shell::Shell;
use crate::components::primitives::{Input, InputEvent, InputState};
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;

use filter::filter_and_sort_indices;
use groups::{
    PaletteDisplayRow, PaletteSectionLabels, build_display_rows, render_section_header,
};
use items::{
    build_palette_items, ensure_palette_sources_loaded, render_palette_row, PaletteItem,
    PaletteItemKind, PaletteRowActions, ROW_PX,
};

const FILTER_DEBOUNCE_MS: u64 = 200;
const KEY_CONTEXT: &str = "CommandPalette";

actions!(mezon_command_palette, [PaletteMoveUp, PaletteMoveDown]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", PaletteMoveUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", PaletteMoveDown, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", ::menu::Cancel, Some(KEY_CONTEXT)),
    ]);
}

pub struct CommandPaletteModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    search_input: Entity<InputState>,
    items: Rc<Vec<PaletteItem>>,
    debounced_query: String,
    filtered: Rc<Vec<usize>>,
    display_rows: Rc<Vec<PaletteDisplayRow>>,
    selected_visible: usize,
    keyboard_nav: bool,
    items_dirty: bool,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _search_sub: Subscription,
    _debounce_task: Task<()>,
    _channel_observe: Subscription,
    _clan_observe: Subscription,
    _direct_observe: Subscription,
    _users_observe: Subscription,
    _members_observe: Subscription,
}

impl Focusable for CommandPaletteModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPaletteModal {
    pub fn toggle(locale: SharedString, window: &mut Window, cx: &mut App) {
        let shell = Shell::global(cx);
        if shell.read(cx).command_palette_open() {
            Self::close(cx);
        } else if !shell.read(cx).has_modal() {
            Self::open(locale, window, cx);
        }
    }

    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        ensure_palette_sources_loaded(cx);

        let placeholder: SharedString = mezon_i18n::t(&locale, "common.searchModal.placeholder")
            .to_string()
            .into();

        let view = cx.new(|cx| {
            let search_input =
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));
            let search_sub = cx.subscribe(
                &search_input,
                |this: &mut Self, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => this.schedule_debounced_filter(cx),
                    InputEvent::PressEnter => this.select_current(cx),
                },
            );
            let items = Rc::new(build_palette_items(cx));
            let filtered = Rc::new(filter_and_sort_indices(items.as_ref(), ""));
            let display_rows = Rc::new(build_display_rows(
                items.as_ref(),
                filtered.as_ref(),
                "",
                &previous_channel_ids(cx),
                &section_labels(&locale),
            ));
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                search_input,
                items,
                debounced_query: String::new(),
                filtered,
                display_rows: display_rows.clone(),
                selected_visible: first_selectable_row(display_rows.as_ref()),
                keyboard_nav: true,
                items_dirty: false,
                scroll: UniformListScrollHandle::new(),
                image_cache: cx.new(|cx| {
                    LruImageCache::avatar_thumbnail(
                        "command-palette-avatars",
                        AVATAR_IMAGE_CACHE_CAPACITY,
                        AVATAR_IMAGE_CACHE_BYTES,
                        AVATAR_ENTRY_MAX_BYTES,
                        cx,
                    )
                }),
                _search_sub: search_sub,
                _debounce_task: Task::ready(()),
                _channel_observe: Subscription::new(|| ()),
                _clan_observe: Subscription::new(|| ()),
                _direct_observe: Subscription::new(|| ()),
                _users_observe: Subscription::new(|| ()),
                _members_observe: Subscription::new(|| ()),
            }
        });

        let view_for_observe = view.clone();
        cx.defer(move |cx| {
            view_for_observe.update(cx, |this, cx| {
                this._channel_observe = cx.observe(&ChannelList::global(cx), |this, _, cx| {
                    this.mark_items_dirty(cx);
                });
                this._clan_observe = cx.observe(&ClanList::global(cx), |this, _, cx| {
                    this.mark_items_dirty(cx);
                });
                this._direct_observe = cx.observe(&DirectMessageStore::global(cx), |this, _, cx| {
                    this.mark_items_dirty(cx);
                });
                if let Some(store) = UsersByUserStore::try_global(cx) {
                    this._users_observe = cx.observe(&store, |this, _, cx| {
                        this.mark_items_dirty(cx);
                    });
                }
                if let Some(store) = ClanMembersStore::try_global(cx) {
                    this._members_observe = cx.observe(&store, |this, _, cx| {
                        this.mark_items_dirty(cx);
                    });
                }
            });
        });

        let focus_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_command_palette(view.into(), cx));
    }

    pub fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    pub fn try_toggle_authenticated(cx: &mut App) {
        use gpui::AppContext;

        let Some(settings) = Settings::try_global(cx) else {
            return;
        };
        let Some(login) = LoginStore::try_global(cx) else {
            return;
        };
        if !matches!(
            login.read(cx).auth_state().read(cx),
            AuthState::Authenticated(_)
        ) {
            return;
        }
        let Some(window_handle) =
            crate::app::main_window::handle(cx).or_else(|| cx.active_window())
        else {
            return;
        };
        let locale = settings.read(cx).language.clone().into();
        cx.defer(move |cx| {
            let _ = cx.update_window(window_handle, |_, window, cx| {
                Self::toggle(locale, window, cx);
            });
        });
    }

    fn mark_items_dirty(&mut self, cx: &mut Context<Self>) {
        if !self.items_dirty {
            self.items_dirty = true;
            cx.notify();
        }
    }

    fn schedule_debounced_filter(&mut self, cx: &mut Context<Self>) {
        self._debounce_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(FILTER_DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.debounced_query = this.search_input.read(cx).value().to_string();
                this.recompute_filtered(cx);
                cx.notify();
            });
        });
    }

    fn recompute_filtered(&mut self, cx: &App) {
        self.filtered = Rc::new(filter_and_sort_indices(
            self.items.as_ref(),
            &self.debounced_query,
        ));
        let previous = if self.debounced_query.is_empty() {
            previous_channel_ids(cx)
        } else {
            Vec::new()
        };
        self.display_rows = Rc::new(build_display_rows(
            self.items.as_ref(),
            self.filtered.as_ref(),
            &self.debounced_query,
            &previous,
            &section_labels(&self.locale),
        ));
        self.selected_visible = first_selectable_row(self.display_rows.as_ref());
        self.scroll
            .scroll_to_item(self.selected_visible, ScrollStrategy::Top);
    }

    fn refresh_items_if_needed(&mut self, cx: &App) {
        if self.items_dirty {
            self.items = Rc::new(build_palette_items(cx));
            self.items_dirty = false;
            self.recompute_filtered(cx);
        }
    }

    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let len = self.display_rows.len();
        if len == 0 {
            return;
        }
        let mut next = self.selected_visible as i32;
        for _ in 0..len {
            next += delta;
            if next < 0 {
                next = len as i32 - 1;
            } else if next >= len as i32 {
                next = 0;
            }
            let ix = next as usize;
            if matches!(self.display_rows[ix], PaletteDisplayRow::Item { .. }) {
                self.selected_visible = ix;
                self.keyboard_nav = true;
                self.scroll
                    .scroll_to_item(ix, ScrollStrategy::Nearest);
                cx.notify();
                return;
            }
        }
    }

    fn select_current(&mut self, cx: &mut Context<Self>) {
        self.select_visible(self.selected_visible, cx);
    }

    fn select_visible(&mut self, visible_ix: usize, cx: &mut Context<Self>) {
        let Some(PaletteDisplayRow::Item { item_index }) = self.display_rows.get(visible_ix) else {
            return;
        };
        let Some(item) = self.items.get(*item_index).cloned() else {
            return;
        };
        let locale = self.locale.clone();
        Self::close(cx);
        match item.kind {
            PaletteItemKind::Channel => {
                let Some(clan_id) = item.clan_id else {
                    return;
                };
                let Some(channel_id) = item.channel_id else {
                    return;
                };
                ChannelList::global(cx).update(cx, |store, cx| {
                    store.reset_user_channel_unread(channel_id, cx);
                    store.apply_read(clan_id, channel_id, cx);
                });
                navigate(
                    cx,
                    Route::Channel {
                        clan_id,
                        channel_id,
                    },
                );
            }
            PaletteItemKind::Direct => {
                let Some(direct_id) = item.channel_id else {
                    return;
                };
                let channel_type = item
                    .dm_channel_type
                    .map(|ty| ty.to_string())
                    .unwrap_or_else(|| DirectKind::Dm.channel_type().to_string());
                DirectMessageStore::global(cx).update(cx, |store, cx| {
                    store.note_read(direct_id, cx);
                    store.set_current(direct_id, item.dm_channel_type.unwrap_or(3));
                });
                navigate(
                    cx,
                    Route::DirectMessage {
                        direct_id,
                        message_type: channel_type,
                    },
                );
            }
            PaletteItemKind::Member => {
                let Some(user_id) = item.user_id else {
                    return;
                };
                if let Some((direct_id, channel_type)) = find_dm_for_user(user_id, cx) {
                    DirectMessageStore::global(cx).update(cx, |store, cx| {
                        store.note_read(direct_id, cx);
                        store.set_current(direct_id, channel_type);
                    });
                    navigate(
                        cx,
                        Route::DirectMessage {
                            direct_id,
                            message_type: channel_type.to_string(),
                        },
                    );
                } else {
                    let error_message: SharedString = mezon_i18n::t(
                        &locale,
                        "common.searchModal.noResults",
                    )
                    .to_string()
                    .into();
                    let task = DirectMessageStore::global(cx).update(cx, |store, cx| {
                        store.create_dm_with_user(user_id, cx)
                    });
                    cx.spawn(async move |_, cx| match task.await {
                        Ok((direct_id, channel_type)) => {
                            cx.update(|cx| {
                                navigate(
                                    cx,
                                    Route::DirectMessage {
                                        direct_id,
                                        message_type: channel_type.to_string(),
                                    },
                                );
                            });
                        }
                        Err(err) => {
                            tracing::warn!("create DM failed: {err}");
                            cx.update(|cx| {
                                Shell::global(cx).update(cx, |shell, cx| {
                                    shell.error(error_message, cx)
                                });
                            });
                        }
                    })
                    .detach();
                }
            }
        }
    }

    fn hover_visible(&mut self, visible_ix: usize, cx: &mut Context<Self>) {
        if self.keyboard_nav {
            return;
        }
        if matches!(
            self.display_rows.get(visible_ix),
            Some(PaletteDisplayRow::Item { .. })
        ) {
            self.selected_visible = visible_ix;
            cx.notify();
        }
    }
}

impl Render for CommandPaletteModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_items_if_needed(cx);

        let theme = cx.theme().clone();
        let locale = self.locale.clone();
        let protip = mezon_i18n::t(&locale, "common.searchModal.protip");
        let protip_description = mezon_i18n::t(&locale, "common.searchModal.protipDescription");

        let search = div()
            .flex_shrink_0()
            .mt_2()
            .mb(px(15.))
            .rounded_lg()
            .bg(theme.tokens.bg_input_secondary)
            .border_1()
            .border_color(theme.tokens.border_theme_primary)
            .px_3()
            .py(px(18.))
            .child(
                Input::new(&self.search_input)
                    .w_full()
                    .text_size(px(16.))
                    .text_color(theme.tokens.text_theme_message),
            );

        let count = self.display_rows.len();
        let search_query = self.debounced_query.clone();
        let list = if count == 0 {
            div()
                .id("command-palette-list")
                .flex_shrink_0()
                .max_h(px(250.))
                .min_h(px(120.))
                .flex()
                .items_center()
                .justify_center()
                .px_4()
                .text_size(px(14.))
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(&locale, "common.searchModal.noResults"))
        } else {
            let items = self.items.clone();
            let display_rows = self.display_rows.clone();
            let search_query = search_query.clone();
            let selected_visible = self.selected_visible;
            let entity = cx.weak_entity();
            let list_h = (count as f32 * ROW_PX).min(250.);
            div()
                .id("command-palette-list")
                .flex_shrink_0()
                .w_full()
                .max_h(px(250.))
                .pr(px(5.))
                .overflow_hidden()
                .on_mouse_move(cx.listener(|this, _, _, _cx| {
                    if this.keyboard_nav {
                        this.keyboard_nav = false;
                    }
                }))
                .child(
                    uniform_list("command-palette-list-inner", count, move |range, _window, cx| {
                        let theme = cx.theme();
                        let items = items.clone();
                        let display_rows = display_rows.clone();
                        let search_query = search_query.clone();
                        let entity = entity.clone();
                        range
                            .map(|visible_ix| {
                                render_display_row(
                                    theme,
                                    visible_ix,
                                    &display_rows,
                                    items.as_ref(),
                                    &search_query,
                                    selected_visible,
                                    entity.clone(),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .track_scroll(&self.scroll)
                    .h(px(list_h))
                    .w_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&self.scroll),
                    window,
                    cx,
                )
        };

        let footer = div()
            .flex_shrink_0()
            .pt_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.status_online)
                            .child(format!("{protip} ")),
                    )
                    .child(div().child(protip_description)),
            );

        div()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .occlude()
            .image_cache(self.image_cache.clone())
            .on_action(cx.listener(|this, _: &PaletteMoveUp, _, cx| {
                this.move_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &PaletteMoveDown, _, cx| {
                this.move_selection(1, cx);
            }))
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Self::close(cx);
            }))
            .w(px(640.))
            .max_w_full()
            .flex()
            .flex_col()
            .px_6()
            .py_4()
            .rounded(px(6.))
            .bg(theme.tokens.bg_modal_theme_search)
            .shadow_lg()
            .child(search)
            .child(list)
            .child(footer)
    }
}

fn render_display_row(
    theme: &crate::theme::Theme,
    visible_ix: usize,
    display_rows: &[PaletteDisplayRow],
    items: &[PaletteItem],
    search_query: &str,
    selected_visible: usize,
    entity: WeakEntity<CommandPaletteModal>,
) -> gpui::AnyElement {
    let Some(row) = display_rows.get(visible_ix) else {
        return div().h(px(ROW_PX)).into_any_element();
    };
    match row {
        PaletteDisplayRow::SectionHeader(label) => render_section_header(theme, label),
        PaletteDisplayRow::Item { item_index } => {
            let selected = visible_ix == selected_visible;
            let actions = PaletteRowActions {
                on_hover: Rc::new({
                    let entity = entity.clone();
                    move |cx| {
                        let _ = entity.update(cx, |this, cx| this.hover_visible(visible_ix, cx));
                    }
                }),
                on_click: Rc::new({
                    let entity = entity.clone();
                    move |cx| {
                        let _ = entity.update(cx, |this, cx| {
                            this.select_visible(visible_ix, cx)
                        });
                    }
                }),
            };
            items
                .get(*item_index)
                .map(|item| {
                    render_palette_row(theme, item, search_query, selected, Some(actions))
                })
                .unwrap_or_else(|| div().h(px(ROW_PX)).into_any_element())
        }
    }
}

fn section_labels(locale: &SharedString) -> PaletteSectionLabels {
    PaletteSectionLabels {
        previous: mezon_i18n::t(locale, "common.searchModal.previousChannels")
            .to_string()
            .into(),
        mentions: mezon_i18n::t(locale, "common.searchModal.mentions")
            .to_string()
            .into(),
        unread: mezon_i18n::t(locale, "common.searchModal.unreadChannels")
            .to_string()
            .into(),
    }
}

fn previous_channel_ids(cx: &App) -> Vec<ChannelId> {
    let active_clan_id = ClanList::global(cx)
        .read(cx)
        .active_clan_id
        .unwrap_or(ClanId(0));
    ChannelList::global(cx)
        .read(cx)
        .previous_channel_ids_for_palette(active_clan_id)
}

fn first_selectable_row(rows: &[PaletteDisplayRow]) -> usize {
    rows.iter()
        .position(|row| matches!(row, PaletteDisplayRow::Item { .. }))
        .unwrap_or(0)
}

fn find_dm_for_user(user_id: UserId, cx: &App) -> Option<(ChannelId, i32)> {
    DirectMessageStore::global(cx)
        .read(cx)
        .channels()
        .iter()
        .find(|dm| dm.kind == DirectKind::Dm && dm.peer_user_id == Some(user_id))
        .map(|dm| (dm.id, dm.kind.channel_type()))
}
