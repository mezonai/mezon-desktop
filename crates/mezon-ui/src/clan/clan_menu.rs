use std::rc::Rc;

use gpui::{
    App, ClickEvent, Entity, MouseDownEvent, Pixels, SharedString, WeakEntity, Window, deferred,
    div, prelude::*, px,
};

use mezon_store::{ChannelList, ClanId, PermissionStore};

use crate::app::shell::Shell;
use crate::clan::create_category_modal::CreateCategoryModal;
use crate::components::primitives::{Icon, IconName, Switch, h_flex, v_flex};
use crate::theme::ActiveTheme;

type MenuHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type DismissHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App) + 'static>;

enum ClanMenuItem {
    Action {
        label: SharedString,
        icon: Option<IconName>,
        danger: bool,
        disabled: bool,
        on_click: MenuHandler,
    },
    Toggle {
        label: SharedString,
        checked: bool,
        on_toggle: ToggleHandler,
    },
}

#[derive(IntoElement, Default)]
pub struct ClanMenuDropdown {
    items: Vec<ClanMenuItem>,
    on_dismiss: Option<DismissHandler>,
}

impl ClanMenuDropdown {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn on_dismiss(mut self, on_dismiss: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    pub fn item(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ClanMenuItem::Action {
            label: label.into(),
            icon: None,
            danger: false,
            disabled: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn item_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ClanMenuItem::Action {
            label: label.into(),
            icon: Some(icon),
            danger: false,
            disabled: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn danger_item_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ClanMenuItem::Action {
            label: label.into(),
            icon: Some(icon),
            danger: true,
            disabled: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn disabled_item(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ClanMenuItem::Action {
            label: label.into(),
            icon: None,
            danger: false,
            disabled: true,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn toggle(
        mut self,
        label: impl Into<SharedString>,
        checked: bool,
        on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ClanMenuItem::Toggle {
            label: label.into(),
            checked,
            on_toggle: Rc::new(on_toggle),
        });
        self
    }
}

impl RenderOnce for ClanMenuDropdown {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let bg = tokens.bg_theme_contexify;
        let border = tokens.border_primary;
        let text = tokens.text_theme_primary;
        let hover = tokens.bg_item_hover;
        let danger = theme.status_dnd;
        let muted = theme.text_secondary;
        let dismiss = self.on_dismiss.clone();

        let mut panel = v_flex()
            .w(px(250.))
            .p(px(8.))
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(bg)
            .shadow_lg()
            .occlude();

        if let Some(dismiss) = dismiss.clone() {
            panel = panel.on_mouse_down_out(move |_: &MouseDownEvent, window, cx| {
                dismiss(window, cx);
            });
        }

        for (index, item) in self.items.into_iter().enumerate() {
            match item {
                ClanMenuItem::Action {
                    label,
                    icon,
                    danger: is_danger,
                    disabled,
                    on_click,
                } => {
                    let dismiss = dismiss.clone();
                    let label_color = if is_danger { danger } else { text };
                    let icon_color = if is_danger { danger } else { muted };
                    let row = h_flex()
                        .id(("clan-menu-item", index))
                        .w_full()
                        .items_center()
                        .justify_between()
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(4.))
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(label_color)
                        .when(!disabled, |el| el.cursor_pointer().hover(|s| s.bg(hover)))
                        .child(label)
                        .when_some(icon, |row, icon| {
                            row.child(Icon::new(icon).size(px(18.)).text_color(icon_color))
                        });

                    panel = panel.child(if disabled {
                        row
                    } else {
                        row.on_click(move |_: &ClickEvent, window, cx| {
                            on_click(window, cx);
                            if let Some(dismiss) = &dismiss {
                                dismiss(window, cx);
                            }
                        })
                    });
                }
                ClanMenuItem::Toggle {
                    label,
                    checked,
                    on_toggle,
                } => {
                    panel = panel.child(
                        h_flex()
                            .id(("clan-menu-toggle", index))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(4.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(text)
                                    .child(label),
                            )
                            .child(
                                Switch::new(("clan-menu-switch", index))
                                    .checked(checked)
                                    .on_click(move |next, window, cx| on_toggle(*next, window, cx)),
                            ),
                    );
                }
            }
        }

        panel
    }
}

pub fn clan_menu_overlay(menu: ClanMenuDropdown, top: Pixels, left: Pixels) -> impl IntoElement {
    deferred(div().absolute().top(top).left(left).child(menu))
}

fn coming_soon_modal(title: String, locale: String) -> impl Fn(&mut Window, &mut App) + 'static {
    move |window: &mut Window, cx: &mut App| {
        let title = title.clone();
        let locale = locale.clone();
        Shell::global(cx).update(cx, |shell, cx| {
            shell.show_coming_soon(title, &locale, window, cx);
        });
    }
}

pub fn build_clan_menu(
    sidebar: WeakEntity<crate::sidebar::channel_sidebar::ChannelSidebar>,
    channel_list: Entity<ChannelList>,
    clan_id: ClanId,
    locale: &str,
    show_empty_categories: bool,
    can_create_category: bool,
) -> ClanMenuDropdown {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let locale_owned = locale.to_string();

    let mut menu = ClanMenuDropdown::new().on_dismiss(move |_window, cx| {
        if let Some(view) = sidebar.upgrade() {
            view.update(cx, |this, cx| this.dismiss_clan_menu(cx));
        }
    });

    if can_create_category {
        let channel_list_create = channel_list.clone();
        let modal_locale = locale_owned.clone();
        menu = menu.item_icon(
            t("clanMenu.modalPanel.createCategory"),
            IconName::CreateCategoryIcon,
            move |window, cx| {
                let modal = cx.new(|cx| {
                    CreateCategoryModal::new(
                        clan_id,
                        channel_list_create.clone(),
                        modal_locale.clone(),
                        window,
                        cx,
                    )
                });
                Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
            },
        );
    }

    let channel_list_mark = channel_list.clone();
    menu = menu.item(t("clanMenu.modalPanel.markAsRead"), move |_window, cx| {
        channel_list_mark.update(cx, |list, cx| list.mark_clan_as_read(clan_id, cx));
    });

    let invite_label = t("clanMenu.modalPanel.invitePeople");
    menu = menu.item_icon(
        invite_label.clone(),
        IconName::AddPerson,
        coming_soon_modal(invite_label, locale_owned.clone()),
    );

    let settings_label = t("clanMenu.modalPanel.clanSettings");
    let settings_clan_id = clan_id;
    menu = menu.item_icon(
        settings_label,
        IconName::SettingProfile,
        move |_window, cx| {
            PermissionStore::global(cx).update(cx, |store, cx| {
                store.load_clan_permissions(settings_clan_id, cx);
            });
            let page = {
                let perms = PermissionStore::global(cx)
                    .read(cx)
                    .clan_settings_permissions(settings_clan_id, cx);
                crate::clan::settings::ClanSettingsPage::default_for_permissions(perms)
            };
            crate::router::navigate(
                cx,
                crate::router::Route::ClanSettings {
                    clan_id: settings_clan_id,
                    page,
                },
            );
        },
    );

    let notification_label = t("clanMenu.modalPanel.notificationSettings");
    menu = menu.item_icon(
        notification_label.clone(),
        IconName::Bell,
        coming_soon_modal(notification_label, locale_owned.clone()),
    );

    let channel_list_toggle = channel_list.clone();
    menu = menu.toggle(
        t("clanMenu.modalPanel.showEmptyCategories"),
        show_empty_categories,
        move |show, _window, cx| {
            channel_list_toggle.update(cx, |list, cx| {
                list.set_show_empty_category(clan_id, show, cx);
            });
        },
    );

    let leave_label = t("clanMenu.modalPanel.leaveClan");
    menu = menu.danger_item_icon(
        leave_label.clone(),
        IconName::LeaveClanIcon,
        coming_soon_modal(leave_label, locale_owned),
    );

    menu
}
