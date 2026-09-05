use crate::app::shell::Shell;
use crate::components::compositions::{
    FRIEND_PICK_ROW_HEIGHT, FriendPickRow, render_friend_pick_row,
};
use crate::components::primitives::{Input, InputEvent, InputState};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;
use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use mezon_store::{
    DirectMessageStore, FriendEvent, FriendState, FriendStore, MAX_GROUP_MEMBERS, UserId,
};

pub struct CreateMessageGroupModal {
    focus_handle: FocusHandle,
    locale: String,
    search_input: Entity<InputState>,
    all_rows: Vec<FriendPickRow>,
    visible: Vec<usize>,
    selected: Vec<UserId>,
    creating: bool,
    scroll: UniformListScrollHandle,
    _input_sub: Subscription,
    _friend_sub: Subscription,
}

impl Focusable for CreateMessageGroupModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateMessageGroupModal {
    pub fn new(locale: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));

        let placeholder = mezon_i18n::t(
            &locale,
            "directMessage.createMessageGroup.searchPlaceholder",
        )
        .to_string();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(34.))
                .radius(px(8.))
        });
        let input_sub = cx.subscribe(
            &search_input,
            |this: &mut Self, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refilter(cx);
                    cx.notify();
                }
            },
        );
        let friend_sub = cx.subscribe(
            &FriendStore::global(cx),
            |this: &mut Self, _store, event: &FriendEvent, cx| {
                if matches!(event, FriendEvent::Changed) {
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            },
        );
        search_input.update(cx, |input, cx| input.focus(window, cx));

        let mut modal = Self {
            focus_handle: cx.focus_handle(),
            locale,
            search_input,
            all_rows: Vec::new(),
            visible: Vec::new(),
            selected: Vec::new(),
            creating: false,
            scroll: UniformListScrollHandle::new(),
            _input_sub: input_sub,
            _friend_sub: friend_sub,
        };
        modal.rebuild_rows(cx);
        modal
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let friends = FriendStore::global(cx);
        let friends = friends.read(cx);
        self.all_rows = friends
            .friends()
            .iter()
            .filter(|friend| friend.state != FriendState::Blocked)
            .map(|friend| FriendPickRow::from_friend(friend, cx))
            .collect();
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        self.visible = self
            .all_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.matches_lowercase_query(&query))
            .map(|(ix, _)| ix)
            .collect();
    }

    fn number_can_add(&self) -> usize {
        (MAX_GROUP_MEMBERS - 1).min(self.all_rows.len())
    }

    fn remaining_can_add(&self) -> usize {
        self.number_can_add().saturating_sub(self.selected.len())
    }

    fn is_selected(&self, user_id: UserId) -> bool {
        self.selected.contains(&user_id)
    }

    fn toggle(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        if let Some(pos) = self.selected.iter().position(|id| *id == user_id) {
            self.selected.remove(pos);
        } else {
            if self.selected.len() >= MAX_GROUP_MEMBERS - 1 {
                return;
            }
            self.selected.push(user_id);
        }
        cx.notify();
    }

    fn create_label(&self) -> SharedString {
        let key = if self.creating {
            "directMessage.createMessageGroup.creating"
        } else if self.selected.is_empty() {
            "directMessage.createMessageGroup.createDMOrGroupChat"
        } else if self.selected.len() == 1 {
            "directMessage.createMessageGroup.createDM"
        } else {
            "directMessage.createMessageGroup.createGroupChat"
        };
        SharedString::from(mezon_i18n::t(&self.locale, key))
    }

    fn handle_create(&mut self, cx: &mut Context<Self>) {
        if self.creating || self.selected.is_empty() {
            return;
        }
        let Some(store) = DirectMessageStore::try_global(cx) else {
            return;
        };

        let friend_store = FriendStore::global(cx);
        let friends = friend_store.read(cx);
        let mut members: Vec<(UserId, String, String, String)> = Vec::new();
        for id in &self.selected {
            if let Some(friend) = friends.friend(*id) {
                members.push((
                    *id,
                    friend.label().to_string(),
                    friend.avatar_url.clone(),
                    friend.username.clone(),
                ));
            }
        }
        if members.is_empty() {
            return;
        }

        let modal_id = cx.entity_id();
        self.creating = true;
        cx.notify();

        let task = if members.len() == 1 {
            let (id, label, avatar, username) = members.remove(0);
            store.update(cx, |store, cx| {
                store.create_dm_with_user(id, label, avatar, username, cx)
            })
        } else {
            let group_label = members
                .iter()
                .map(|member| member.1.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let ids: Vec<UserId> = members.into_iter().map(|member| member.0).collect();
            store.update(cx, |store, cx| {
                store.create_group_with_users(ids, group_label, cx)
            })
        };

        cx.spawn(async move |this, cx| match task.await {
            Ok((channel_id, channel_type)) => {
                cx.update(|cx| {
                    navigate(
                        cx,
                        Route::DirectMessage {
                            direct_id: channel_id,
                            message_type: channel_type.to_string(),
                        },
                    );
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal_view(modal_id, cx));
                });
            }
            Err(err) => {
                tracing::warn!("create dm/group failed: {err}");
                let _ = this.update(cx, |this, cx| {
                    this.creating = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for CreateMessageGroupModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let entity = cx.entity();

        let title = mezon_i18n::t(
            &self.locale,
            "directMessage.createMessageGroup.selectFriends",
        )
        .to_string();
        let subtitle = mezon_i18n::t(
            &self.locale,
            "directMessage.createMessageGroup.canAddMoreFriends",
        )
        .replace("{{count}}", &self.remaining_can_add().to_string());
        let create_label = self.create_label();
        let enabled = !self.creating && !self.selected.is_empty();

        const LIST_HEIGHT: f32 = 190.;

        let row_count = self.visible.len();
        let list_body = if row_count == 0 {
            div()
                .h(px(LIST_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .px(px(24.))
                .text_center()
                .text_size(px(14.))
                .text_color(theme.text_secondary)
                .child(mezon_i18n::t(
                    &self.locale,
                    "directMessage.createMessageGroup.noFriendsFound",
                ))
                .into_any_element()
        } else {
            let list_entity = entity.clone();
            uniform_list(
                "create-group-friends",
                row_count,
                move |range, _window, cx| {
                    let theme = cx.theme().clone();
                    let modal = list_entity.read(cx);
                    range
                        .map(|ix| {
                            match modal.visible.get(ix).and_then(|i| modal.all_rows.get(*i)) {
                                Some(row) => {
                                    let toggle_entity = list_entity.clone();
                                    render_friend_pick_row(
                                        &theme,
                                        row,
                                        modal.is_selected(row.user_id),
                                        move |user_id, cx| {
                                            toggle_entity
                                                .update(cx, |this, cx| this.toggle(user_id, cx));
                                        },
                                    )
                                }
                                None => div().h(px(FRIEND_PICK_ROW_HEIGHT)).into_any_element(),
                            }
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll)
            .w_full()
            .h(px(LIST_HEIGHT))
            .into_any_element()
        };

        let button_entity = entity.clone();
        let create_button = div()
            .id("create-dm-group-submit")
            .h(px(38.))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(gpui::white())
            .bg(theme.tokens.button_theme_primary)
            .when(enabled, |el| {
                el.cursor_pointer().hover(|s| s.opacity(0.9)).on_click(
                    move |_: &ClickEvent, _window, cx| {
                        button_entity.update(cx, |this, cx| this.handle_create(cx));
                    },
                )
            })
            .when(!enabled, |el| el.opacity(0.6))
            .child(create_label);

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .occlude()
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .w(px(440.))
            .max_w(px(440.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(8.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .text_size(px(14.))
                            .text_color(theme.text_secondary)
                            .child(subtitle),
                    )
                    .child(div().mt(px(20.)).child(
                        Input::new(&self.search_input).text_color(theme.tokens.text_theme_primary),
                    )),
            )
            .child(list_body)
            .child(div().p(px(20.)).child(create_button))
            .into_any_element()
    }
}
