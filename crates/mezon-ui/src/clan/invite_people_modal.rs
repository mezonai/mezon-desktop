use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::imgproxy;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FontWeight, Image as ClipboardImage, ImageFormat, ObjectFit, RenderImage, SharedString,
    Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, uniform_list,
};
use mezon_store::{
    AppConfig, ChannelId, ClanId, ClanInviteLink, ClanList, ClanMembersEvent, ClanMembersStore,
    DirectEvent, DirectKind, DirectMessageStore, Friend, FriendEvent, FriendState, FriendStore,
    UserId,
};
use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct InviteFriendRow {
    key: SharedString,
    user_id: Option<UserId>,
    channel: Option<(ChannelId, i32)>,
    label: SharedString,
    username: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
}

#[derive(Clone)]
struct QrInviteImage {
    render: Arc<RenderImage>,
    clipboard: ClipboardImage,
}

pub struct InvitePeopleModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    clan_name: String,
    clan_avatar_src: SharedString,
    clan_avatar_raw: SharedString,
    locale: String,
    invite_link: String,
    invite_link_loading: bool,
    invite_link_error: Option<String>,
    qr_image: Option<QrInviteImage>,
    show_qr: bool,
    search_input: Entity<InputState>,
    rows: Vec<InviteFriendRow>,
    sent: HashSet<SharedString>,
    sending: HashSet<SharedString>,
    copied: bool,
    scroll: UniformListScrollHandle,
    _input_sub: Subscription,
    _friend_sub: Subscription,
    _direct_sub: Subscription,
    _member_sub: Subscription,
}

impl Focusable for InvitePeopleModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl InvitePeopleModal {
    pub fn new(
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        clan_name: String,
        clan_avatar_url: String,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        DirectMessageStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));

        let search_placeholder = mezon_i18n::t(&locale, "invitation.searchPlaceholder").to_string();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(search_placeholder)
                .height(px(40.))
                .radius(px(8.))
        });
        let input_sub = cx.subscribe(
            &search_input,
            |this: &mut Self, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            },
        );
        let friend_sub = cx.subscribe(
            &FriendStore::global(cx),
            |this: &mut Self, _store, _event: &FriendEvent, cx| {
                this.rebuild_rows(cx);
                cx.notify();
            },
        );
        let direct_sub = cx.subscribe(
            &DirectMessageStore::global(cx),
            |this: &mut Self, _store, _event: &DirectEvent, cx| {
                this.rebuild_rows(cx);
                cx.notify();
            },
        );
        let member_sub = cx.subscribe(
            &ClanMembersStore::global(cx),
            move |this: &mut Self, _store, event: &ClanMembersEvent, cx| {
                if matches!(event, ClanMembersEvent::Changed { clan_id: changed } if *changed == clan_id) {
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            },
        );
        search_input.update(cx, |input, cx| input.focus(window, cx));

        let clan_avatar_src = if clan_avatar_url.is_empty() {
            String::new()
        } else {
            imgproxy::avatar_url(cx, &clan_avatar_url)
        };
        let mut modal = Self {
            focus_handle: cx.focus_handle(),
            clan_id,
            clan_name,
            clan_avatar_src: SharedString::from(clan_avatar_src),
            clan_avatar_raw: SharedString::from(clan_avatar_url),
            locale,
            invite_link: String::new(),
            invite_link_loading: true,
            invite_link_error: None,
            qr_image: None,
            show_qr: false,
            search_input,
            rows: Vec::new(),
            sent: HashSet::new(),
            sending: HashSet::new(),
            copied: false,
            scroll: UniformListScrollHandle::new(),
            _input_sub: input_sub,
            _friend_sub: friend_sub,
            _direct_sub: direct_sub,
            _member_sub: member_sub,
        };
        modal.rebuild_rows(cx);
        modal.load_invite_link(clan_id, channel_id, cx);
        modal
    }

    fn invite_link(&self) -> String {
        self.invite_link.clone()
    }

    fn invite_link_ready(&self) -> bool {
        !self.invite_link_loading
            && self.invite_link_error.is_none()
            && !self.invite_link.is_empty()
    }

    fn load_invite_link(
        &mut self,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) {
        let Some(clan_list) = ClanList::try_global(cx) else {
            self.invite_link_loading = false;
            self.invite_link_error =
                Some(tr(&self.locale, "inviteToChannel.share.errorCreateLink"));
            return;
        };
        let config = AppConfig::try_global(cx).cloned();
        let locale = self.locale.clone();
        let task = clan_list.update(cx, |store, cx| {
            store.create_invite_link(clan_id, channel_id, cx)
        });

        cx.spawn(async move |this, cx| match task.await {
            Ok(link) => {
                let invite_link = invite_link_from_api(&link, config.as_ref())
                    .ok_or_else(|| tr(&locale, "inviteToChannel.share.errorCreateLink"));
                let _ = this.update(cx, |this, cx| match invite_link {
                    Ok(invite_link) => {
                        this.invite_link = invite_link;
                        this.qr_image = build_qr_invite_image(&this.invite_link);
                        this.invite_link_loading = false;
                        this.invite_link_error = None;
                        cx.notify();
                    }
                    Err(err) => {
                        this.invite_link.clear();
                        this.qr_image = None;
                        this.invite_link_loading = false;
                        this.invite_link_error = Some(err);
                        cx.notify();
                    }
                });
            }
            Err(err) => {
                tracing::warn!("create clan invite link failed: {err}");
                let _ = this.update(cx, |this, cx| {
                    this.invite_link.clear();
                    this.qr_image = None;
                    this.invite_link_loading = false;
                    this.invite_link_error =
                        Some(tr(&this.locale, "inviteToChannel.share.errorCreateLink"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let clan_members: HashSet<UserId> = ClanMembersStore::global(cx)
            .read(cx)
            .members(self.clan_id)
            .into_iter()
            .map(|member| member.id())
            .collect();
        let friends = FriendStore::global(cx);
        let friends = friends.read(cx);
        let mut listed_users = HashSet::new();
        let mut rows = Vec::new();

        for direct in DirectMessageStore::global(cx).read(cx).channels() {
            if direct.kind == DirectKind::Dm {
                let Some(user_id) = direct.peer_user_id else {
                    continue;
                };
                if clan_members.contains(&user_id)
                    || friends
                        .friends()
                        .iter()
                        .any(|friend| friend.id == user_id && friend.state == FriendState::Blocked)
                    || !listed_users.insert(user_id)
                {
                    continue;
                }
            }
            if !query.is_empty()
                && !direct.label.to_lowercase().contains(&query)
                && !direct.peer_username.to_lowercase().contains(&query)
            {
                continue;
            }
            let avatar_src = if direct.avatar.is_empty() {
                String::new()
            } else {
                imgproxy::avatar_url(cx, &direct.avatar)
            };
            rows.push(InviteFriendRow {
                key: format!("channel-{}", direct.id).into(),
                user_id: direct.peer_user_id,
                channel: Some((direct.id, direct.kind.stream_mode())),
                label: direct.label.clone().into(),
                username: direct.peer_username.clone().into(),
                avatar_src: avatar_src.into(),
                avatar_raw: direct.avatar.clone().into(),
            });
        }

        for friend in friends.friends() {
            if friend.state == FriendState::Blocked
                || clan_members.contains(&friend.id)
                || listed_users.contains(&friend.id)
                || !friend_matches_query(friend, &query)
            {
                continue;
            }
            listed_users.insert(friend.id);
            let label = friend.label().to_string();
            let avatar_src = if friend.avatar_url.is_empty() {
                String::new()
            } else {
                imgproxy::avatar_url(cx, &friend.avatar_url)
            };
            rows.push(InviteFriendRow {
                key: format!("user-{}", friend.id).into(),
                user_id: Some(friend.id),
                channel: None,
                label: SharedString::from(label),
                username: SharedString::from(friend.username.clone()),
                avatar_src: SharedString::from(avatar_src),
                avatar_raw: SharedString::from(friend.avatar_url.clone()),
            });
        }
        rows.sort_by_key(|row| row.label.to_lowercase());
        self.rows = rows;
    }

    fn invite_friend(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if self.sent.contains(&key) || self.sending.contains(&key) {
            return;
        }
        if !self.invite_link_ready() {
            return;
        }

        let Some(store) = DirectMessageStore::try_global(cx) else {
            Shell::global(cx).update(cx, |shell, cx| {
                shell.error(tr(&self.locale, "shareContact.card.messageError"), cx);
            });
            return;
        };

        self.sending.insert(key.clone());
        cx.notify();

        let Some(row) = self.rows.iter().find(|row| row.key == key) else {
            self.sending.remove(&key);
            cx.notify();
            return;
        };
        let label = row.label.to_string();
        let avatar = row.avatar_raw.to_string();
        let username = row.username.to_string();
        let user_id = row.user_id;
        let channel = row.channel;
        let content = self.invite_message_content(cx);
        let task = store.update(cx, |store, cx| {
            store.send_direct_text_to_target(user_id, channel, label, avatar, username, content, cx)
        });
        let locale = self.locale.clone();
        cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                let _ = this.update(cx, |this, cx| {
                    this.sending.remove(&key);
                    this.sent.insert(key.clone());
                    cx.notify();
                });
            }
            Err(err) => {
                tracing::warn!("send clan invite failed: {err}");
                let message = format!("{}: {err}", tr(&locale, "inviteToChannel.share.error"));
                let _ = this.update(cx, |this, cx| {
                    this.sending.remove(&key);
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                });
            }
        })
        .detach();
    }

    fn invite_message_content(&self, cx: &App) -> String {
        let Some(clan) = ClanList::global(cx)
            .read(cx)
            .clan_by_id(self.clan_id)
            .cloned()
        else {
            return self.invite_link();
        };
        let url = self.invite_link();
        let end = url.chars().count();
        serde_json::json!({
            "t": url,
            "mk": [
                { "s": 0, "e": end, "type": "lk" },
                {
                    "s": end, "e": end + 1, "type": "lk_ogp", "url": url,
                    "title": clan.name, "image": clan.avatar_url.unwrap_or_default(),
                    "banner": clan.banner_url.unwrap_or_default(), "clanId": clan.id.to_string(),
                    "member_count": ClanMembersStore::global(cx).read(cx).count(self.clan_id),
                    "is_community": clan.is_community
                }
            ]
        })
        .to_string()
    }

    fn copy_invite_link(&mut self, cx: &mut Context<Self>) {
        if !self.invite_link_ready() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.invite_link()));
        self.copied = true;
        cx.notify();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn open_qr(&mut self, cx: &mut Context<Self>) {
        if !self.invite_link_ready() {
            return;
        }
        self.show_qr = true;
        cx.notify();
    }

    fn close_qr(&mut self, cx: &mut Context<Self>) {
        self.show_qr = false;
        cx.notify();
    }

    fn copy_qr(&mut self, cx: &mut Context<Self>) {
        let Some(qr) = &self.qr_image else {
            let message =
                mezon_i18n::t(&self.locale, "inviteToChannel.qrModal.errorGenerating").to_string();
            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_image(&qr.clipboard));
        let message =
            mezon_i18n::t(&self.locale, "invitation.messages.qrCopiedSuccess").to_string();
        Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for InvitePeopleModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.show_qr {
            return render_qr_modal(cx.theme(), cx.entity(), self);
        }

        let theme = cx.theme().clone();
        let entity = cx.entity();
        let title = SharedString::from(invite_modal_title(&self.locale, &self.clan_name));
        let invite_link = self.invite_link();
        let copied = self.copied;
        let invite_ready = self.invite_link_ready();
        let invite_loading = self.invite_link_loading;
        let invite_error = self.invite_link_error.clone();
        let invite_label = SharedString::from(tr(&self.locale, "inviteToChannel.btnInvite"));
        let sent_label = SharedString::from(tr(&self.locale, "inviteToChannel.btnSent"));
        let sending_label = SharedString::from(tr(&self.locale, "forwardMessage.modal.sending"));

        const ROW_HEIGHT: f32 = 56.;
        const MAX_VISIBLE_ROWS: usize = 4;

        let list_entity = entity.clone();
        let row_count = self.rows.len();
        let visible_rows = row_count.min(MAX_VISIBLE_ROWS);
        let list_height = px(ROW_HEIGHT * visible_rows as f32);

        let friend_list = uniform_list(
            "invite-friends-list",
            row_count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                let modal = list_entity.read(cx);
                range
                    .map(|ix| match modal.rows.get(ix) {
                        Some(row) => render_friend_row(
                            &theme,
                            row.clone(),
                            modal.sent.contains(&row.key),
                            modal.sending.contains(&row.key),
                            invite_ready,
                            invite_label.clone(),
                            sent_label.clone(),
                            sending_label.clone(),
                            list_entity.clone(),
                        ),
                        None => div().h(px(ROW_HEIGHT)).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .w_full()
        .h(list_height)
        .min_h_0();

        let qr_entity = entity.clone();
        let list_body = if row_count == 0 {
            div()
                .py(px(10.))
                .text_size(px(13.))
                .text_color(theme.text_secondary)
                .child(mezon_i18n::t(&self.locale, "invitation.noResults"))
                .into_any_element()
        } else {
            div()
                .h(list_height)
                .min_h_0()
                .overflow_hidden()
                .child(friend_list)
                .into_any_element()
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .occlude()
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .w(px(500.))
            .max_w(px(500.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(20.))
                    .py(px(15.))
                    .border_b_1()
                    .border_color(theme.tokens.border_primary)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::white())
                            .child(title),
                    )
                    .child(
                        div()
                            .id("invite-people-close")
                            .size(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .opacity(0.65)
                            .hover(|s| s.opacity(1.0))
                            .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(24.))
                                    .text_color(theme.text_secondary),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .px(px(24.))
                    .pt(px(20.))
                    .child(
                        Input::new(&self.search_input).text_color(theme.tokens.text_theme_primary),
                    )
                    .child(list_body),
            )
            .child(
                div().px(px(24.)).pb(px(20.)).child(
                    div()
                        .border_t_1()
                        .border_color(theme.tokens.border_primary)
                        .pt(px(18.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .mb(px(8.))
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui::white())
                                .child(invite_send_link_label(&self.locale))
                                .child(
                                    div()
                                        .id("invite-copy-qr")
                                        .text_color(theme.text_link)
                                        .when(invite_ready, |el| {
                                            el.cursor_pointer().on_click(
                                                move |_: &ClickEvent, _window, cx| {
                                                    qr_entity
                                                        .update(cx, |this, cx| this.open_qr(cx));
                                                },
                                            )
                                        })
                                        .when(!invite_ready, |el| el.opacity(0.55))
                                        .child(copy_qr_label(&self.locale)),
                                ),
                        )
                        .child(render_copy_link(
                            &theme,
                            invite_link,
                            copied,
                            invite_loading,
                            invite_error,
                            invite_ready,
                            &self.locale,
                            entity.clone(),
                        )),
                ),
            )
            .into_any_element()
    }
}
fn render_qr_modal(
    theme: &Theme,
    entity: Entity<InvitePeopleModal>,
    modal: &InvitePeopleModal,
) -> AnyElement {
    let mut clan_avatar = Avatar::new()
        .name(SharedString::from(modal.clan_name.clone()))
        .size_px(px(40.));
    if !modal.clan_avatar_src.is_empty() {
        clan_avatar = clan_avatar.src(modal.clan_avatar_src.clone());
        if !modal.clan_avatar_raw.is_empty() && modal.clan_avatar_raw != modal.clan_avatar_src {
            clan_avatar = clan_avatar.fallback_src(modal.clan_avatar_raw.clone());
        }
    } else if !modal.clan_avatar_raw.is_empty() {
        clan_avatar = clan_avatar.src(modal.clan_avatar_raw.clone());
    }

    let cancel_entity = entity.clone();
    let copy_entity = entity.clone();
    let qr_content = match &modal.qr_image {
        Some(qr) => img(qr.render.clone())
            .size(px(260.))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => div()
            .size(px(260.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.text_secondary)
            .child(mezon_i18n::t(
                &modal.locale,
                "inviteToChannel.qrModal.errorGenerating",
            ))
            .into_any_element(),
    };

    div()
        .track_focus(&modal.focus_handle)
        .key_context("menu")
        .occlude()
        .on_action({
            let entity = entity.clone();
            move |_: &::menu::Cancel, _window, cx| {
                entity.update(cx, |this, cx| this.close_qr(cx));
            }
        })
        .w(px(340.))
        .max_w(px(340.))
        .flex()
        .flex_col()
        .items_center()
        .rounded(px(12.))
        .bg(theme.tokens.theme_setting_primary)
        .shadow_lg()
        .p(px(22.))
        .child(
            div()
                .relative()
                .w(px(290.))
                .pt(px(20.))
                .child(
                    div()
                        .w_full()
                        .rounded(px(6.))
                        .bg(gpui::white())
                        .p(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(qr_content),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .size(px(46.))
                                .rounded_full()
                                .bg(gpui::white())
                                .p(px(3.))
                                .child(clan_avatar),
                        ),
                ),
        )
        .child(
            div()
                .mt(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    Button::new("invite-qr-cancel")
                        .label(mezon_i18n::t(&modal.locale, "invitation.buttons.cancel"))
                        .ghost()
                        .h(px(40.))
                        .min_w(px(88.))
                        .border_1()
                        .border_color(theme.tokens.border_primary)
                        .on_click(move |_, _window, cx| {
                            cancel_entity.update(cx, |this, cx| this.close_qr(cx));
                        }),
                )
                .child(
                    Button::new("invite-qr-copy")
                        .label(copy_qr_label(&modal.locale))
                        .primary()
                        .h(px(40.))
                        .min_w(px(96.))
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .on_click(move |_, _window, cx| {
                            copy_entity.update(cx, |this, cx| this.copy_qr(cx));
                        }),
                ),
        )
        .into_any_element()
}
fn friend_matches_query(friend: &Friend, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    friend.label().to_lowercase().contains(query) || friend.username.to_lowercase().contains(query)
}
fn render_friend_row(
    theme: &Theme,
    row: InviteFriendRow,
    sent: bool,
    sending: bool,
    invite_ready: bool,
    invite_label: SharedString,
    sent_label: SharedString,
    sending_label: SharedString,
    entity: Entity<InvitePeopleModal>,
) -> AnyElement {
    let mut avatar = Avatar::new().name(row.label.clone()).size_px(px(36.));
    if !row.avatar_src.is_empty() {
        avatar = avatar.src(row.avatar_src.clone());
        if !row.avatar_raw.is_empty() && row.avatar_raw != row.avatar_src {
            avatar = avatar.fallback_src(row.avatar_raw.clone());
        }
    } else if !row.avatar_raw.is_empty() {
        avatar = avatar.src(row.avatar_raw.clone());
    }

    let action = render_invite_action(
        theme,
        row.key.clone(),
        sent,
        sending,
        invite_ready,
        invite_label,
        sent_label,
        sending_label,
        entity,
    );
    let group_name = SharedString::from(format!("invite-row-{}", row.key));

    div()
        .id(SharedString::from(format!("invite-friend-row-{}", row.key)))
        .group(group_name)
        .w_full()
        .h(px(56.))
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px(px(12.))
        .rounded_lg()
        .hover(|s| s.bg(theme.tokens.bg_item_hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .flex_1()
                .child(avatar)
                .child(
                    div()
                        .truncate()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(gpui::white())
                        .child(row.label),
                ),
        )
        .child(action)
        .into_any_element()
}

fn render_invite_action(
    theme: &Theme,
    key: SharedString,
    sent: bool,
    sending: bool,
    invite_ready: bool,
    invite_label: SharedString,
    sent_label: SharedString,
    sending_label: SharedString,
    entity: Entity<InvitePeopleModal>,
) -> AnyElement {
    if sent {
        return div()
            .min_w(px(72.))
            .h(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme.text_secondary)
            .child(sent_label)
            .into_any_element();
    }

    let label = if sending { sending_label } else { invite_label };
    let group_name = SharedString::from(format!("invite-row-{key}"));

    div()
        .id(SharedString::from(format!("invite-friend-{key}")))
        .min_w(px(72.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .when(invite_ready && !sending, |el| {
            el.cursor_pointer()
                .group_hover(group_name, |s| {
                    s.bg(theme.status_online).text_color(gpui::white())
                })
                .on_click(move |_: &ClickEvent, _window, cx| {
                    entity.update(cx, |this, cx| this.invite_friend(key.clone(), cx));
                })
        })
        .when(!invite_ready || sending, |el| el.opacity(0.65))
        .child(label)
        .into_any_element()
}
fn render_copy_link(
    theme: &Theme,
    invite_link: String,
    copied: bool,
    loading: bool,
    error: Option<String>,
    invite_ready: bool,
    locale: &str,
    entity: Entity<InvitePeopleModal>,
) -> AnyElement {
    let button_label = if copied {
        mezon_i18n::t(locale, "invitation.buttons.copied")
    } else {
        mezon_i18n::t(locale, "inviteToChannel.iconTitle.copyLink")
    };
    let button_bg = if copied {
        theme.status_online
    } else {
        theme.tokens.button_theme_primary
    };
    let display_text = if let Some(error) = error {
        error
    } else if loading {
        tr(locale, "inviteToChannel.loadingInviteLink")
    } else {
        invite_link
    };

    div()
        .h(px(44.))
        .flex()
        .items_center()
        .overflow_hidden()
        .rounded_lg()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(theme.tokens.bg_input_secondary)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .px(px(16.))
                .truncate()
                .text_size(px(14.))
                .text_color(gpui::white())
                .child(display_text),
        )
        .child(
            div()
                .id("invite-link-copy")
                .h_full()
                .w(px(110.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_r_lg()
                .bg(button_bg)
                .text_size(px(14.))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::white())
                .when(invite_ready, |el| {
                    el.cursor_pointer().hover(|s| s.opacity(0.9)).on_click(
                        move |_: &ClickEvent, _window, cx| {
                            entity.update(cx, |this, cx| this.copy_invite_link(cx));
                        },
                    )
                })
                .when(!invite_ready, |el| el.opacity(0.65))
                .child(button_label),
        )
        .into_any_element()
}

fn tr(locale: &str, key: &'static str) -> String {
    mezon_i18n::t(locale, key).to_string()
}

fn invite_modal_title(locale: &str, clan_name: &str) -> String {
    mezon_i18n::t(locale, "invitation.modal.title").replace("{{target}}", clan_name)
}

fn invite_send_link_label(locale: &str) -> String {
    mezon_i18n::t(locale, "invitation.modal.sendLinkText").replace(
        "{{type}}",
        mezon_i18n::t(locale, "invitation.modal.clanInvite"),
    )
}

fn copy_qr_label(locale: &str) -> String {
    mezon_i18n::t(locale, "invitation.buttons.copyQR").to_string()
}

fn invite_link_for_code(invite_code: &str, config: Option<&AppConfig>) -> String {
    let origin = invite_origin_from_config(config);
    if origin.is_empty() {
        format!("/invite/{invite_code}")
    } else {
        format!("{origin}/invite/{invite_code}")
    }
}

fn invite_link_from_api(link: &ClanInviteLink, config: Option<&AppConfig>) -> Option<String> {
    let invite_code = invite_code_from_link(&link.invite_link).or_else(|| {
        if link.id == 0 {
            None
        } else {
            Some(link.id.to_string())
        }
    })?;
    Some(invite_link_for_code(&invite_code, config))
}

fn invite_code_from_link(link: &str) -> Option<String> {
    let trimmed = link.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    let raw_code = if let Some(index) = lower.find("/invite/") {
        &trimmed[index + "/invite/".len()..]
    } else {
        trimmed
    };
    let code = raw_code.split(['?', '#', '/']).next().unwrap_or("").trim();
    if code.is_empty() {
        None
    } else {
        Some(code.to_string())
    }
}

fn invite_origin_from_config(config: Option<&AppConfig>) -> String {
    let Some(config) = config else {
        return String::new();
    };

    invite_origin_from_url_like(&config.redirect_uri)
        .or_else(|| invite_origin_from_url_like(&config.domain_url))
        .unwrap_or_default()
}

fn invite_origin_from_host(host: &str, secure: bool) -> Option<String> {
    let trimmed = host.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let (scheme, host_part) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => (if secure { "https" } else { "http" }.to_string(), trimmed),
    };
    let host_part = host_part
        .split('/')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('/');
    if scheme.is_empty() || host_part.is_empty() {
        None
    } else {
        Some(format!("{scheme}://{host_part}"))
    }
}

fn invite_origin_from_url_like(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    invite_origin_from_host(trimmed, true)
}

fn build_qr_invite_image(data: &str) -> Option<QrInviteImage> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    if width == 0 {
        return None;
    }

    let colors = code.to_colors();
    let scale = (320 / width).max(2);
    let dim = (width * scale) as u32;
    let mut buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        dim,
        dim,
        image::Rgba([255, 255, 255, 255]),
    );

    for (i, color) in colors.iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let ox = ((i % width) * scale) as u32;
        let oy = ((i / width) * scale) as u32;
        for dy in 0..scale as u32 {
            for dx in 0..scale as u32 {
                buffer.put_pixel(ox + dx, oy + dy, image::Rgba([0, 0, 0, 255]));
            }
        }
    }

    let render = Arc::new(RenderImage::new(vec![image::Frame::new(buffer.clone())]));
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut png, image::ImageFormat::Png)
        .ok()?;
    let clipboard = ClipboardImage::from_bytes(ImageFormat::Png, png.into_inner());

    Some(QrInviteImage { render, clipboard })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_origin_uses_redirect_uri_web_origin() {
        let mut cfg = AppConfig::dev_defaults();
        cfg.api_host = "api.example.test".into();
        cfg.redirect_uri = "https://chat.example.test/login/callback".into();
        assert_eq!(
            invite_link_for_code("42", Some(&cfg)),
            "https://chat.example.test/invite/42"
        );
    }

    #[test]
    fn invite_link_uses_api_invite_id_instead_of_clan_id() {
        let cfg = AppConfig::dev_defaults();
        let link = ClanInviteLink {
            id: 2076277531916374016,
            invite_link: String::new(),
        };

        assert_eq!(
            invite_link_from_api(&link, Some(&cfg)),
            Some("https://mezon.ai/invite/2076277531916374016".to_string())
        );
    }

    #[test]
    fn qr_invite_image_generates_renderable_and_clipboard_png() {
        let qr = build_qr_invite_image("https://mezon.ai/invite/42").expect("qr image");
        assert_eq!(qr.clipboard.format, ImageFormat::Png);
        assert!(!qr.clipboard.bytes.is_empty());
    }
}
