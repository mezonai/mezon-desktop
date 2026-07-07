mod filter;
mod items;

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, uniform_list, App, Context, Entity, FocusHandle, Focusable, FontWeight,
    SharedString, Subscription, Task, UniformListScrollHandle, Window,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};
use mezon_store::{
    AuthState, ChannelList, ClanList, ClanMembersStore, DirectMessageStore, LoginStore, Settings,
    UsersByUserStore,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Input, InputEvent, InputState};
use crate::theme::ActiveTheme;

use filter::filter_and_sort_indices;
use items::{
    build_palette_items, ensure_palette_sources_loaded, render_palette_row, PaletteItem, ROW_PX,
};

const FILTER_DEBOUNCE_MS: u64 = 200;

pub struct CommandPaletteModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    search_input: Entity<InputState>,
    items: Rc<Vec<PaletteItem>>,
    debounced_query: String,
    filtered: Rc<Vec<usize>>,
    items_dirty: bool,
    scroll: UniformListScrollHandle,
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
                |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.schedule_debounced_filter(cx);
                    }
                },
            );
            let items = Rc::new(build_palette_items(cx));
            let filtered = Rc::new(filter_and_sort_indices(items.as_ref(), ""));
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                search_input,
                items,
                debounced_query: String::new(),
                filtered,
                items_dirty: false,
                scroll: UniformListScrollHandle::new(),
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
                this.recompute_filtered();
                cx.notify();
            });
        });
    }

    fn recompute_filtered(&mut self) {
        self.filtered = Rc::new(filter_and_sort_indices(
            self.items.as_ref(),
            &self.debounced_query,
        ));
    }

    fn refresh_items_if_needed(&mut self, cx: &App) {
        if self.items_dirty {
            self.items = Rc::new(build_palette_items(cx));
            self.items_dirty = false;
            self.recompute_filtered();
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

        let count = self.filtered.len();
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
            let filtered = self.filtered.clone();
            let search_query = search_query.clone();
            let list_h = (count as f32 * ROW_PX).min(250.);
            div()
                .id("command-palette-list")
                .flex_shrink_0()
                .w_full()
                .max_h(px(250.))
                .pr(px(5.))
                .overflow_hidden()
                .child(
                    uniform_list("command-palette-list-inner", count, move |range, _window, cx| {
                        let theme = cx.theme();
                        let items = items.clone();
                        let filtered = filtered.clone();
                        let search_query = search_query.clone();
                        range
                            .map(|visible_ix| match filtered.get(visible_ix).copied() {
                                Some(item_ix) => items
                                    .get(item_ix)
                                    .map(|item| render_palette_row(theme, item, &search_query))
                                    .unwrap_or_else(|| div().h(px(ROW_PX)).into_any_element()),
                                None => div().h(px(ROW_PX)).into_any_element(),
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
            .key_context("menu")
            .occlude()
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
