use gpui::{Context, FontWeight, SharedString, Window, div, prelude::*, px};

use mezon_store::{
    PERMISSION_ADMINISTRATOR, PERMISSION_SEND_MESSAGE, PermissionDefinition, PermissionStore,
};

use super::role_setting_page::RoleSettingPage;
use crate::components::primitives::{Input, InputEvent, InputState, Switch, v_flex};
use crate::theme::{ActiveTheme, Theme};

fn permission_title_key(slug: &str) -> Option<&'static str> {
    match slug {
        "administrator" => Some("clanRoles.permissionTitles.administrator"),
        "clan-owner" => Some("clanRoles.permissionTitles.clan-owner"),
        "delete-message" => Some("clanRoles.permissionTitles.delete-message"),
        "manage-channel" => Some("clanRoles.permissionTitles.manage-channel"),
        "manage-clan" => Some("clanRoles.permissionTitles.manage-clan"),
        "manage-thread" => Some("clanRoles.permissionTitles.manage-thread"),
        "send-message" => Some("clanRoles.permissionTitles.send-message"),
        "view-channel" => Some("clanRoles.permissionTitles.view-channel"),
        _ => None,
    }
}

fn permission_description_key(slug: &str) -> Option<&'static str> {
    match slug {
        "administrator" => Some("clanRoles.permissionDescriptions.administrator"),
        "delete-message" => Some("clanRoles.permissionDescriptions.delete-message"),
        "manage-channel" => Some("clanRoles.permissionDescriptions.manage-channel"),
        "manage-clan" => Some("clanRoles.permissionDescriptions.manage-clan"),
        "manage-thread" => Some("clanRoles.permissionDescriptions.manage-thread"),
        "send-message" => Some("clanRoles.permissionDescriptions.send-message"),
        "view-channel" => Some("clanRoles.permissionDescriptions.view-channel"),
        _ => None,
    }
}

fn localized_permission_title(locale: &str, perm: &PermissionDefinition) -> String {
    if let Some(key) = permission_title_key(&perm.slug) {
        let text = mezon_i18n::t(locale, key);
        if text != key {
            return text.to_string();
        }
    }
    perm.title.clone()
}

fn localized_permission_description(locale: &str, slug: &str) -> String {
    if slug.is_empty() {
        return mezon_i18n::t(locale, "clanRoles.permissionDescriptions.notAvailable").to_string();
    }
    if let Some(key) = permission_description_key(slug) {
        let text = mezon_i18n::t(locale, key);
        if text != key {
            return text.to_string();
        }
    }
    mezon_i18n::t(locale, "clanRoles.permissionDescriptions.notAvailable").to_string()
}

impl RoleSettingPage {
    pub(super) fn render_permissions_tab(
        &mut self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        is_everyone: bool,
        is_clan_owner: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_permission_search_input(window, cx);
        PermissionStore::global(cx).update(cx, |store, cx| {
            store.ensure_catalog_loaded(cx);
        });
        let definitions: Vec<PermissionDefinition> = PermissionStore::global(cx)
            .read(cx)
            .permission_definitions()
            .to_vec();
        let query = self.permission_search_query.to_ascii_lowercase();
        let filtered: Vec<&PermissionDefinition> = definitions
            .iter()
            .filter(|perm| {
                if query.is_empty() {
                    return true;
                }
                perm.slug.to_ascii_lowercase().contains(&query)
                    || localized_permission_title(locale, perm)
                        .to_ascii_lowercase()
                        .contains(&query)
                    || localized_permission_description(locale, &perm.slug)
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .collect();
        let user_permission_level = PermissionStore::global(cx)
            .read(cx)
            .current_permission_level(self.clan_id, cx)
            .unwrap_or(-1);

        v_flex()
            .gap_3()
            .child(
                div().when_some(self.permission_search_input.clone(), |col, input| {
                    col.child(
                        div()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_tertiary)
                            .child(Input::new(&input)),
                    )
                }),
            )
            .child(v_flex().gap_2().children({
                let mut rows = Vec::new();
                for perm in filtered {
                    rows.push(
                        self.render_permission_row(
                            locale,
                            theme,
                            perm,
                            can_edit,
                            is_everyone,
                            is_clan_owner,
                            user_permission_level,
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                rows
            }))
    }

    fn render_permission_row(
        &self,
        locale: &str,
        theme: &Theme,
        perm: &PermissionDefinition,
        can_edit: bool,
        is_everyone: bool,
        is_clan_owner: bool,
        user_permission_level: i32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let checked = self.draft_permission_ids.contains(&perm.id);
        let hide_admin = !is_clan_owner && can_edit && perm.slug == PERMISSION_ADMINISTRATOR;
        let lock_everyone_send = is_everyone && perm.slug == PERMISSION_SEND_MESSAGE;
        let lock_hierarchy = !is_clan_owner && can_edit && perm.level >= user_permission_level;
        let disabled = !can_edit || hide_admin || lock_everyone_send || lock_hierarchy;
        let title = localized_permission_title(locale, perm);
        let description = localized_permission_description(locale, &perm.slug);
        let perm_id = perm.id;

        div()
            .id(("role-permission", perm_id as u64))
            .w_full()
            .flex()
            .items_start()
            .justify_between()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_hover)
            .when(!disabled, |el| el.cursor_pointer())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(description),
                    ),
            )
            .child(
                div().flex_shrink_0().child(
                    Switch::new(("role-permission-switch", perm_id as u64))
                        .checked(checked)
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, next, _, cx| {
                            this.set_permission_active(perm_id, *next, cx);
                        })),
                ),
            )
    }

    pub(super) fn ensure_permission_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.permission_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clanRoles.roleManagement.searchPermissions").into();
        let input_bg = cx.theme().tokens.bg_tertiary;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(36.0))
                .text_size(px(15.0))
                .borderless()
                .bg(input_bg)
        });
        let sub = cx.subscribe(&input, |this, input_entity, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.permission_search_query = input_entity.read(cx).value().to_string();
                cx.notify();
            }
        });
        self.permission_search_input = Some(input);
        self._permission_search_sub = Some(sub);
    }
}
