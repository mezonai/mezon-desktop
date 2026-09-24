use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use gpui::{
    Context, Entity, FontWeight, ObjectFit, SharedString, Subscription, Window, div, img,
    prelude::*, px,
};
use mezon_store::{AppConfig, DirectMessageStore, FriendEvent, FriendStore, Settings, UserId};
use serde::Deserialize;

use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Button, ButtonVariants, v_flex};
use crate::router::{Route, Router, navigate, replace};
use crate::theme::ActiveTheme;
use crate::util::qr_image::{QrImage, QrImageOptions, build_qr_image};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relationship {
    Checking,
    CanChat,
    CanAdd,
}

#[derive(Deserialize)]
struct ContactData {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    avatar: String,
}

fn decode_contact(data: &str) -> Option<ContactData> {
    let raw = B64.decode(data.as_bytes()).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let decoded = percent_encoding::percent_decode_str(&text)
        .decode_utf8()
        .ok()?;
    serde_json::from_str(&decoded).ok()
}

pub struct AddFriendPage {
    username: String,
    data: Option<String>,
    settings: Entity<Settings>,
    target: Option<UserId>,
    display_name: SharedString,
    avatar_url: SharedString,
    relationship: Relationship,
    qr_image: Option<QrImage>,
    _subscriptions: Vec<Subscription>,
}

impl AddFriendPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(
            &FriendStore::global(cx),
            |this, _, event: &FriendEvent, cx| match event {
                FriendEvent::FollowerChecked { user, is_follower }
                    if this.target == Some(*user) =>
                {
                    this.on_relationship(*is_follower, cx);
                }
                FriendEvent::AddingChanged => cx.notify(),
                _ => {}
            },
        );
        let mut subscriptions = vec![subscription];
        subscriptions.push(cx.observe(&Router::global(cx), |this, _, cx| {
            this.sync_route(cx);
        }));
        let mut page = Self {
            username: String::new(),
            data: None,
            settings,
            target: None,
            display_name: SharedString::default(),
            avatar_url: SharedString::default(),
            relationship: Relationship::CanAdd,
            qr_image: None,
            _subscriptions: subscriptions,
        };
        page.sync_route(cx);
        page
    }

    fn sync_route(&mut self, cx: &mut Context<Self>) {
        let Route::AddFriend { username, data } = Router::global(cx).read(cx).route() else {
            return;
        };
        if self.username == username && self.data == data {
            return;
        }
        self.username = username;
        self.data = data.clone();
        self.load(data, cx);
        cx.notify();
    }

    fn load(&mut self, data: Option<String>, cx: &mut Context<Self>) {
        let contact = data.as_deref().and_then(decode_contact);
        self.target = contact
            .as_ref()
            .and_then(|contact| contact.id.parse::<i64>().ok())
            .filter(|id| *id != 0)
            .map(UserId);
        self.display_name = contact
            .as_ref()
            .map(|contact| contact.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.username.clone())
            .into();
        self.avatar_url = contact
            .as_ref()
            .map(|contact| contact.avatar.clone())
            .unwrap_or_default()
            .into();

        let origin = AppConfig::try_global(cx)
            .map(|config| config.domain_url.trim_end_matches('/').to_string())
            .unwrap_or_default();
        let link = match data.as_deref() {
            Some(data) => format!("{origin}/chat/{}?data={data}", self.username),
            None => format!("{origin}/chat/{}", self.username),
        };
        self.qr_image = build_qr_image(
            &link,
            QrImageOptions {
                target_size: 200,
                min_module_scale: 2,
                error_correction: qrcode::EcLevel::L,
                clipboard_border: 0,
            },
        );

        let Some(target) = self.target else {
            self.relationship = Relationship::CanAdd;
            return;
        };
        self.relationship = Relationship::Checking;
        FriendStore::global(cx).update(cx, |store, cx| store.check_is_follower(target, cx));
    }

    fn on_relationship(&mut self, is_follower: bool, cx: &mut Context<Self>) {
        self.relationship = if is_follower {
            let locale = self.settings.read(cx).language.clone();
            let message = mezon_i18n::t(&locale, "common.invite.canChatNow");
            Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
            Relationship::CanChat
        } else {
            Relationship::CanAdd
        };
        cx.notify();
    }

    fn chat_now(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target else {
            return;
        };
        let Some(store) = DirectMessageStore::try_global(cx) else {
            return;
        };
        let task = store.update(cx, |store, cx| {
            store.create_dm_with_user(target, String::new(), String::new(), String::new(), cx)
        });
        cx.spawn(async move |_, cx| match task.await {
            Ok((channel_id, channel_type)) => {
                cx.update(|cx| {
                    replace(
                        cx,
                        Route::DirectMessage {
                            direct_id: channel_id,
                            message_type: channel_type.to_string(),
                        },
                    );
                });
            }
            Err(error) => {
                tracing::warn!("open dm from invite failed: {error}");
                cx.update(|cx| navigate(cx, Route::Friends));
            }
        })
        .detach();
    }

    fn add_friend(&mut self, cx: &mut Context<Self>) {
        if FriendStore::global(cx).read(cx).is_adding() {
            return;
        }
        let username = self.username.clone();
        FriendStore::global(cx).update(cx, |store, cx| store.add_friend_by_username(username, cx));
        navigate(cx, Route::Friends);
    }
}

impl Render for AddFriendPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let adding = FriendStore::global(cx).read(cx).is_adding();

        let avatar = if self.avatar_url.is_empty() {
            Avatar::new()
                .name(self.display_name.clone())
                .size_px(px(80.))
                .into_any_element()
        } else {
            Avatar::new()
                .name(self.display_name.clone())
                .src(self.avatar_url.clone())
                .size_px(px(80.))
                .into_any_element()
        };

        let action = match self.relationship {
            Relationship::Checking => div()
                .text_sm()
                .text_color(theme.tokens.text_secondary)
                .child(mezon_i18n::t(&locale, "common.invite.verifyWait"))
                .into_any_element(),
            Relationship::CanChat => Button::new("invite-chat-now")
                .label(mezon_i18n::t(&locale, "common.invite.chatNow"))
                .primary()
                .w_full()
                .on_click(cx.listener(|this, _, _window, cx| this.chat_now(cx)))
                .into_any_element(),
            Relationship::CanAdd => Button::new("invite-add-friend")
                .label(mezon_i18n::t(&locale, "common.invite.addFriend"))
                .primary()
                .w_full()
                .disabled(adding)
                .on_click(cx.listener(|this, _, _window, cx| this.add_friend(cx)))
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.bg_primary)
            .child(
                v_flex()
                    .w(px(420.))
                    .items_center()
                    .gap_4()
                    .p_8()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.theme_setting_primary)
                    .shadow_lg()
                    .child(avatar)
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(self.display_name.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(format!("@{}", self.username)),
                    )
                    .when_some(self.qr_image.as_ref(), |el, qr| {
                        el.child(
                            div().p_4().rounded_lg().bg(gpui::white()).child(
                                img(qr.render.clone())
                                    .size(px(200.))
                                    .object_fit(ObjectFit::Contain),
                            ),
                        )
                    })
                    .child(action),
            )
    }
}
