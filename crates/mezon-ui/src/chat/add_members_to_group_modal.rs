use std::collections::HashSet;

use crate::app::shell::Shell;
use crate::components::compositions::{
    FRIEND_PICK_ROW_HEIGHT, FriendPickRow, render_friend_pick_row,
};
use crate::components::primitives::{Input, InputEvent, InputState, Sizable, Size, Spinner};
use crate::theme::ActiveTheme;
use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use mezon_store::{
    AddGroupMembersError, ChannelId, FriendEvent, FriendState, FriendStore, GroupMembersEvent,
    GroupMembersStore, MAX_GROUP_MEMBERS, UserId,
};

pub struct AddMembersToGroupModal {
    focus_handle: FocusHandle,
    channel_id: ChannelId,
    locale: String,
    title: SharedString,
    add_label: SharedString,
    search_input: Entity<InputState>,
    all_rows: Vec<FriendPickRow>,
    visible: Vec<usize>,
    selected: Vec<UserId>,
    roster_size: Option<usize>,
    adding: bool,
    scroll: UniformListScrollHandle,
    _input_sub: Subscription,
    _friend_sub: Subscription,
    _group_sub: Subscription,
}

impl Focusable for AddMembersToGroupModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddMembersToGroupModal {
    pub fn open(channel_id: ChannelId, locale: String, window: &mut Window, cx: &mut App) {
        let modal = cx.new(|cx| Self::new(channel_id, locale, window, cx));
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.clone().into(), cx));
        window.defer(cx, move |window, cx| {
            modal.update(cx, |this, cx| {
                this.search_input
                    .update(cx, |input, cx| input.focus(window, cx));
            });
        });
    }

    fn new(
        channel_id: ChannelId,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        GroupMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(channel_id, cx));

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
        let group_sub = cx.subscribe(
            &GroupMembersStore::global(cx),
            |this: &mut Self, _store, event: &GroupMembersEvent, cx| {
                let GroupMembersEvent::Changed { channel_id } = event;
                if *channel_id == this.channel_id {
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            },
        );
        let title = mezon_i18n::t(&locale, "common.addMembers")
            .to_string()
            .into();
        let add_label = mezon_i18n::t(&locale, "directMessage.createMessageGroup.addToGroupChat")
            .to_string()
            .into();

        let mut modal = Self {
            focus_handle: cx.focus_handle(),
            channel_id,
            locale,
            title,
            add_label,
            search_input,
            all_rows: Vec::new(),
            visible: Vec::new(),
            selected: Vec::new(),
            roster_size: None,
            adding: false,
            scroll: UniformListScrollHandle::new(),
            _input_sub: input_sub,
            _friend_sub: friend_sub,
            _group_sub: group_sub,
        };
        modal.rebuild_rows(cx);
        modal
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let (current, roster_size) = self.current_members(cx);
        self.roster_size = roster_size;
        let friends = FriendStore::global(cx);
        let friends = friends.read(cx);
        self.all_rows = friends
            .friends()
            .iter()
            .filter(|friend| friend.state != FriendState::Blocked)
            .filter(|friend| !current.contains(&friend.id))
            .map(|friend| FriendPickRow::from_friend(friend, cx))
            .collect();
        let eligible: HashSet<UserId> = self.all_rows.iter().map(|row| row.user_id).collect();
        self.selected.retain(|id| eligible.contains(id));
        let capacity = self.capacity();
        self.selected.truncate(capacity);
        self.refilter(cx);
    }

    fn current_members(&self, cx: &App) -> (HashSet<UserId>, Option<usize>) {
        let Some(store) = GroupMembersStore::try_global(cx) else {
            return (HashSet::new(), None);
        };
        let store = store.read(cx);
        if !store.is_loaded(self.channel_id) {
            return (HashSet::new(), None);
        }
        let members: HashSet<UserId> = store
            .members(self.channel_id)
            .iter()
            .map(|member| member.id())
            .collect();
        let size = members.len();
        (members, Some(size))
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

    fn capacity(&self) -> usize {
        match self.roster_size {
            Some(size) => addable_slots(size, self.all_rows.len()),
            None => 0,
        }
    }

    fn remaining_can_add(&self) -> usize {
        self.capacity().saturating_sub(self.selected.len())
    }

    fn is_selected(&self, user_id: UserId) -> bool {
        self.selected.contains(&user_id)
    }

    fn toggle(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        if self.adding {
            return;
        }
        if let Some(pos) = self.selected.iter().position(|id| *id == user_id) {
            self.selected.remove(pos);
        } else {
            if self.selected.len() >= self.capacity() {
                return;
            }
            self.selected.push(user_id);
        }
        cx.notify();
    }

    fn handle_add(&mut self, cx: &mut Context<Self>) {
        if self.adding || self.selected.is_empty() {
            return;
        }
        let Some(store) = GroupMembersStore::try_global(cx) else {
            return;
        };
        let channel_id = self.channel_id;
        let modal_id = cx.entity_id();
        let locale = self.locale.clone();
        let user_ids = self.selected.clone();
        let failed: SharedString = mezon_i18n::t(&self.locale, "common.somethingWentWrong").into();
        let group_full: SharedString =
            mezon_i18n::t(&self.locale, "directMessage.createMessageGroup.groupFull")
                .replace("{{count}}", &MAX_GROUP_MEMBERS.to_string())
                .into();

        self.adding = true;
        cx.notify();

        let task = store.update(cx, |store, cx| store.add_members(channel_id, user_ids, cx));
        cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                let _ = this.update(cx, |this, cx| {
                    this.adding = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal_view(modal_id, cx));
                });
            }
            Err(err) => {
                let message = match err {
                    AddGroupMembersError::GroupFull => group_full,
                    AddGroupMembersError::Api(code) => {
                        SharedString::from(mezon_i18n::api_error(&locale, code))
                    }
                    AddGroupMembersError::Other(_) => failed,
                };
                let _ = this.update(cx, |this, cx| {
                    this.adding = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message.clone(), cx));
                });
            }
        })
        .detach();
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

fn addable_slots(member_count: usize, candidate_count: usize) -> usize {
    MAX_GROUP_MEMBERS
        .saturating_sub(member_count)
        .min(candidate_count)
}

impl Render for AddMembersToGroupModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let entity = cx.entity();

        let subtitle = mezon_i18n::t(
            &self.locale,
            "directMessage.createMessageGroup.canAddMoreFriends",
        )
        .replace("{{count}}", &self.remaining_can_add().to_string());
        let enabled = !self.adding && !self.selected.is_empty();

        const LIST_HEIGHT: f32 = 190.;

        let row_count = self.visible.len();
        let list_body = if self.roster_size.is_none() || row_count == 0 {
            let key = if self.roster_size.is_none() {
                "root.loading"
            } else {
                "directMessage.createMessageGroup.noFriendsFound"
            };
            div()
                .h(px(LIST_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .px(px(24.))
                .text_center()
                .text_size(px(14.))
                .text_color(theme.text_secondary)
                .child(mezon_i18n::t(&self.locale, key))
                .into_any_element()
        } else {
            let list_entity = entity.clone();
            uniform_list(
                "add-group-members-friends",
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
        let add_button = div()
            .id("add-group-members-submit")
            .h(px(38.))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded(px(6.))
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(gpui::white())
            .bg(theme.tokens.button_theme_primary)
            .when(enabled, |el| {
                el.cursor_pointer().hover(|s| s.opacity(0.9)).on_click(
                    move |_: &ClickEvent, _window, cx| {
                        button_entity.update(cx, |this, cx| this.handle_add(cx));
                    },
                )
            })
            .when(!enabled, |el| el.opacity(0.6))
            .when(self.adding, |el| {
                el.child(Spinner::new().with_size(Size::XSmall).color(gpui::white()))
            })
            .child(self.add_label.clone());

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
                            .child(self.title.clone()),
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
            .child(div().p(px(20.)).child(add_button))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_bounded_by_the_server_cap() {
        assert_eq!(addable_slots(3, 50), MAX_GROUP_MEMBERS - 3);
    }

    #[test]
    fn slots_are_bounded_by_the_candidates_on_offer() {
        assert_eq!(addable_slots(3, 2), 2);
    }

    #[test]
    fn a_full_group_offers_no_slots() {
        assert_eq!(addable_slots(MAX_GROUP_MEMBERS, 10), 0);
        assert_eq!(addable_slots(MAX_GROUP_MEMBERS + 5, 10), 0);
    }
}
