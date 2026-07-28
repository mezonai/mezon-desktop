use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, ClickEvent, ClipboardItem, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, ListAlignment, ListState, MouseButton, MouseDownEvent, SharedString,
    Window, div, list, prelude::*, px,
};
use mezon_store::{
    AppConfig, BadgeService, CanvasStore, CanvasSummary, ChannelId, ChannelList, Settings, UserId,
    canvas_web_link,
};
use ui::{PopoverMenuHandle, ScrollAxes, Scrollbars, WithScrollbar};

use crate::navigation::confirm_delete_canvas;
use crate::navigation::{CanvasRoute, active_canvas_id, navigate_to_canvas};
use crate::view::canvas_can_delete;
use mezon_theme::{ActiveTheme, Theme};
use mezon_widgets::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Sizable, Size, Spinner,
    h_flex, v_flex,
};

const POPOVER_WIDTH: f32 = 480.;
const HEADER_HEIGHT: f32 = 48.;
const BODY_HEIGHT: f32 = 352.;
const LIST_OVERDRAW: f32 = 200.;

pub struct CanvasPopoverPanel {
    settings: Entity<Settings>,
    popover_handle: PopoverMenuHandle<CanvasPopoverPanel>,
    search_input: Entity<InputState>,
    search_query: SharedString,
    focus_handle: FocusHandle,
    list_state: ListState,
    copied_row: Option<usize>,
    _copy_reset: Option<gpui::Task<()>>,
    _subs: Vec<gpui::Subscription>,
}

impl CanvasPopoverPanel {
    pub fn new(
        settings: Entity<Settings>,
        search_input: Entity<InputState>,
        popover_handle: PopoverMenuHandle<CanvasPopoverPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let search_for_sub = search_input.clone();
        let search_sub = cx.subscribe_in(&search_input, window, move |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_query = search_for_sub.read(cx).value().to_string().into();
                cx.notify();
            }
        });

        let subs = vec![
            cx.observe(&CanvasStore::global(cx), |_, _, cx| cx.notify()),
            cx.observe(&settings, |_, _, cx| cx.notify()),
            search_sub,
        ];

        let list_state = ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW)).measure_all();

        Self {
            settings,
            popover_handle,
            search_input,
            search_query: SharedString::default(),
            focus_handle,
            list_state,
            copied_row: None,
            _copy_reset: None,
            _subs: subs,
        }
    }

    fn mark_row_copied(&mut self, index: usize, cx: &mut Context<Self>) {
        self.copied_row = Some(index);
        cx.notify();
        self._copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                this.copied_row = None;
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Focusable for CanvasPopoverPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for CanvasPopoverPanel {}

impl Render for CanvasPopoverPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let store = CanvasStore::global(cx).read(cx);
        let loading = store.is_loading();
        let query = self.search_query.to_lowercase();
        let canvases: Vec<CanvasSummary> = store
            .canvases()
            .iter()
            .filter(|c| {
                if query.is_empty() {
                    return true;
                }
                c.title.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let canvases = Rc::new(canvases);
        let count = canvases.len();
        let current = self.list_state.item_count();
        if count > current {
            self.list_state.splice(0..0, count - current);
        } else if count < current {
            self.list_state.reset(count);
        }
        let list_state = self.list_state.clone();
        let clan_id = store.clan_id().map(|id| id.to_string());
        let channel_id = store.channel_id().map(|s| s.to_string());
        let handle = self.popover_handle.clone();
        let search_input = self.search_input.clone();
        let tokens = &theme.tokens;

        let channel_creator = ChannelList::global(cx)
            .read(cx)
            .active_channel()
            .map(|c| c.creator_id);
        let current_user = BadgeService::global(cx).read(cx).current_user_id(cx);
        let active_canvas_id = active_canvas_id(cx);

        v_flex()
            .occlude()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(POPOVER_WIDTH))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .text_color(tokens.text_theme_message)
            .child(render_header(
                theme.as_ref(),
                &locale,
                search_input,
                handle.clone(),
                cx,
            ))
            .child(render_body(
                canvases,
                loading,
                theme.as_ref(),
                locale,
                handle,
                clan_id,
                channel_id,
                channel_creator,
                current_user,
                active_canvas_id,
                list_state,
                self.copied_row,
                cx.weak_entity(),
                window,
                cx,
            ))
    }
}

fn render_header(
    theme: &Theme,
    locale: &str,
    search_input: Entity<InputState>,
    handle: PopoverMenuHandle<CanvasPopoverPanel>,
    cx: &mut Context<CanvasPopoverPanel>,
) -> impl IntoElement {
    let tokens = &theme.tokens;
    let create_handle = handle.clone();
    let close_handle = handle;

    let title_block = h_flex()
        .items_center()
        .gap_4()
        .pr_4()
        .flex_shrink_0()
        .border_r_1()
        .border_color(tokens.border_primary)
        .child(
            Icon::new(IconName::CanvasIcon)
                .size_4()
                .text_color(tokens.bg_icon_theme),
        )
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(locale, "channelTopbar.modals.canvas.title")),
        );

    let search_block = div()
        .relative()
        .w(px(224.))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            h_flex()
                .w_full()
                .h(px(24.))
                .pl_4()
                .pr_2()
                .rounded_md()
                .bg(tokens.theme_input)
                .items_center()
                .child(
                    Input::new(&search_input)
                        .flex_1()
                        .text_sm()
                        .text_color(tokens.text_theme_primary),
                ),
        )
        .child(
            h_flex()
                .absolute()
                .right(px(4.))
                .top_0()
                .bottom_0()
                .items_center()
                .pl_1()
                .child(
                    Icon::new(IconName::Search)
                        .size_4()
                        .text_color(tokens.text_theme_primary),
                ),
        );

    let create_btn = Button::new("canvas-create-btn")
        .label(mezon_i18n::t(locale, "channelTopbar.modals.canvas.create"))
        .primary()
        .with_size(Size::Small)
        .on_click(cx.listener(move |_, _: &ClickEvent, _window, cx| {
            create_canvas(create_handle.clone(), cx);
        }));

    let close_btn = Button::new("canvas-close-btn")
        .icon(
            Icon::new(IconName::Close)
                .size_4()
                .text_color(tokens.text_theme_primary_hover),
        )
        .ghost()
        .with_size(Size::Small)
        .on_click(move |_: &ClickEvent, _window, cx| {
            close_handle.hide(cx);
        });

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .px(px(16.))
        .h(px(HEADER_HEIGHT))
        .border_b_1()
        .border_color(tokens.border_primary)
        .bg(tokens.theme_setting_nav)
        .child(title_block)
        .child(search_block)
        .child(
            h_flex()
                .items_center()
                .gap_4()
                .flex_shrink_0()
                .child(create_btn)
                .child(close_btn),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_body(
    canvases: Rc<Vec<CanvasSummary>>,
    loading: bool,
    theme: &Theme,
    locale: String,
    handle: PopoverMenuHandle<CanvasPopoverPanel>,
    clan_id: Option<String>,
    channel_id: Option<String>,
    channel_creator: Option<UserId>,
    current_user: Option<UserId>,
    active_canvas_id: Option<String>,
    list_state: ListState,
    copied_row: Option<usize>,
    panel: gpui::WeakEntity<CanvasPopoverPanel>,
    window: &mut Window,
    cx: &mut Context<CanvasPopoverPanel>,
) -> impl IntoElement {
    let tokens = &theme.tokens;

    let body: gpui::AnyElement = if canvases.is_empty() {
        if loading {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .child(Spinner::new().with_size(Size::Small))
                .into_any_element()
        } else {
            render_empty(theme, &locale, handle.clone(), cx).into_any_element()
        }
    } else {
        let canvases_for_list = canvases.clone();
        let locale_for_list = locale.clone();
        let handle_for_list = handle.clone();
        let clan_for_list = clan_id.clone();
        let channel_for_list = channel_id.clone();
        let active_for_list = active_canvas_id.clone();
        let panel_for_list = panel.clone();
        let copied_for_list = copied_row;
        div()
            .size_full()
            .overflow_hidden()
            .p_2()
            .child(
                list(list_state.clone(), move |ix, _window, cx| {
                    let theme = cx.theme();
                    let Some(canvas) = canvases_for_list.get(ix) else {
                        return div().into_any_element();
                    };
                    div()
                        .w_full()
                        .pb_2()
                        .child(canvas_row(
                            ix,
                            canvas,
                            theme.as_ref(),
                            &locale_for_list,
                            handle_for_list.clone(),
                            clan_for_list.clone(),
                            channel_for_list.clone(),
                            channel_creator,
                            current_user,
                            active_for_list.as_deref(),
                            copied_for_list == Some(ix),
                            panel_for_list.clone(),
                        ))
                        .into_any_element()
                })
                .size_full(),
            )
            .custom_scrollbars(
                Scrollbars::always_visible(ScrollAxes::Vertical).tracked_scroll_handle(&list_state),
                window,
                cx,
            )
            .into_any_element()
    };

    div()
        .w_full()
        .h(px(BODY_HEIGHT))
        .overflow_hidden()
        .bg(tokens.theme_setting_primary)
        .child(body)
}

fn render_empty(
    theme: &Theme,
    locale: &str,
    handle: PopoverMenuHandle<CanvasPopoverPanel>,
    cx: &mut Context<CanvasPopoverPanel>,
) -> impl IntoElement {
    let tokens = &theme.tokens;
    v_flex()
        .items_center()
        .justify_center()
        .size_full()
        .px_12()
        .gap_2()
        .child(
            Icon::new(IconName::CanvasIcon)
                .size(px(56.))
                .text_color(tokens.bg_icon_theme),
        )
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(locale, "channelTopbar.canvas.emptyTitle")),
        )
        .child(
            div()
                .text_base()
                .text_center()
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(
                    locale,
                    "channelTopbar.canvas.emptyDescription",
                )),
        )
        .child(
            Button::new("canvas-empty-create")
                .label(mezon_i18n::t(locale, "channelTopbar.canvas.createCanvas"))
                .primary()
                .with_size(Size::Small)
                .mt_4()
                .on_click(cx.listener(move |_, _: &ClickEvent, _window, cx| {
                    create_canvas(handle.clone(), cx);
                })),
        )
}

#[allow(clippy::too_many_arguments)]
fn canvas_row(
    index: usize,
    canvas: &CanvasSummary,
    theme: &Theme,
    locale: &str,
    handle: PopoverMenuHandle<CanvasPopoverPanel>,
    clan_id: Option<String>,
    channel_id: Option<String>,
    channel_creator: Option<UserId>,
    current_user: Option<UserId>,
    active_canvas_id: Option<&str>,
    link_copied: bool,
    panel: gpui::WeakEntity<CanvasPopoverPanel>,
) -> gpui::AnyElement {
    let tokens = &theme.tokens;
    let title = if canvas.title.is_empty() {
        mezon_i18n::t(locale, "common.canvas.untitled").to_string()
    } else {
        canvas.title.clone()
    };
    let canvas_id = canvas.id.clone();
    let is_active = active_canvas_id == Some(canvas_id.as_str());
    let can_delete = canvas_can_delete(canvas.creator_id, channel_creator, current_user);

    let open_id = canvas_id.clone();
    let open_clan = clan_id.clone();
    let open_channel = channel_id.clone();
    let open_handle = handle.clone();

    let copy_clan = clan_id.clone();
    let copy_channel = channel_id.clone();
    let copy_id = canvas_id.clone();

    let delete_id = canvas_id.clone();
    let delete_title = title.clone();
    let delete_locale = locale.to_string();
    let delete_clan = clan_id.clone();
    let delete_channel = channel_id.clone();
    let delete_handle = handle.clone();

    let row_bg = if is_active {
        tokens.bg_tertiary
    } else {
        tokens.theme_setting_primary
    };

    let mut actions = h_flex()
        .absolute()
        .top(px(8.))
        .right(px(8.))
        .items_center()
        .gap_1();

    actions = actions.child(
        Button::new(("canvas-copy", index))
            .icon(
                Icon::new(if link_copied {
                    IconName::Check
                } else {
                    IconName::CopyIcon
                })
                .size_4()
                .text_color(if link_copied {
                    theme.status_online
                } else {
                    tokens.text_theme_primary
                }),
            )
            .ghost()
            .with_size(Size::XSmall)
            .on_click(move |_: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                let Some(clan_id) = copy_clan.as_deref() else {
                    return;
                };
                let Some(channel_id) = copy_channel.as_deref() else {
                    return;
                };
                let Some(cfg) = AppConfig::try_global(cx) else {
                    return;
                };
                let link = canvas_web_link(&cfg.redirect_uri, clan_id, channel_id, &copy_id);
                cx.write_to_clipboard(ClipboardItem::new_string(link));
                let _ = panel.update(cx, |this, cx| this.mark_row_copied(index, cx));
            }),
    );

    if can_delete {
        actions = actions.child(
            Button::new(("canvas-del", index))
                .label("✕")
                .ghost()
                .with_size(Size::XSmall)
                .on_click(move |_: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    let (Some(clan), Some(channel)) =
                        (delete_clan.as_ref(), delete_channel.as_ref())
                    else {
                        return;
                    };
                    let Ok(clan_id) = clan.parse() else {
                        return;
                    };
                    let Ok(channel_id) = channel.parse() else {
                        return;
                    };
                    delete_handle.hide(cx);
                    confirm_delete_canvas(
                        delete_id.clone(),
                        delete_title.clone(),
                        clan_id,
                        channel_id,
                        &delete_locale,
                        window,
                        cx,
                    );
                }),
        );
    }

    div()
        .id(("canvas-item", index))
        .relative()
        .w_full()
        .py_2()
        .pl_4()
        .pr(if can_delete { px(64.) } else { px(40.) })
        .rounded_lg()
        .cursor_pointer()
        .bg(row_bg)
        .hover(|s| s.bg(tokens.bg_hover))
        .on_click(move |_: &ClickEvent, _window, cx| {
            let (Some(clan), Some(channel)) = (open_clan.as_ref(), open_channel.as_ref()) else {
                return;
            };
            let Ok(clan_id) = clan.parse() else {
                return;
            };
            let Ok(channel_id) = channel.parse() else {
                return;
            };
            let Ok(canvas_id) = open_id.parse() else {
                return;
            };
            navigate_to_canvas(
                CanvasRoute {
                    clan_id,
                    channel_id,
                    canvas_id,
                },
                cx,
            );
            open_handle.hide(cx);
        })
        .child(
            div()
                .h(px(24.))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(tokens.text_theme_message)
                .child(title),
        )
        .child(actions)
        .into_any_element()
}

fn create_canvas(
    handle: PopoverMenuHandle<CanvasPopoverPanel>,
    cx: &mut Context<CanvasPopoverPanel>,
) {
    let store = CanvasStore::global(cx).read(cx);
    let Some(clan_id) = store.clan_id() else {
        return;
    };
    let Some(channel_id) = store.channel_id().and_then(|s| s.parse().ok()) else {
        return;
    };
    cx.defer(move |cx| {
        navigate_to_canvas(
            CanvasRoute {
                clan_id,
                channel_id,
                canvas_id: ChannelId::default(),
            },
            cx,
        );
        handle.hide(cx);
    });
}

pub fn canvas_popover_on_open() -> Rc<dyn Fn(&mut Window, &mut App)> {
    Rc::new(|_window, cx| {
        CanvasStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(cx);
        });
    })
}
