use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, DismissEvent, Div, ElementId,
    EventEmitter, FocusHandle, Focusable, FontWeight, MouseDownEvent, ParentElement, Render,
    SharedString, Stateful, StyleRefinement, Styled, Window, div, prelude::*, px,
};
use mezon_store::{ProfileContext, RolesStore, Settings, UserId, resolve_user_profile};
use ui::{Clickable, PopoverMenu, Toggleable};

use crate::components::primitives::Avatar;
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

pub struct UserProfilePopover {
    focus_handle: FocusHandle,
    user_id: UserId,
    context: ProfileContext,
    settings: gpui::Entity<Settings>,
    avatar_image_cache: gpui::Entity<LruImageCache>,
    _roles_sub: Option<gpui::Subscription>,
}

impl UserProfilePopover {
    pub fn new(
        user_id: UserId,
        context: ProfileContext,
        settings: gpui::Entity<Settings>,
        avatar_image_cache: gpui::Entity<LruImageCache>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent))
            .detach();
        let roles_sub = RolesStore::try_global(cx)
            .map(|roles_store| cx.observe(&roles_store, |_, _, cx| cx.notify()));
        Self {
            focus_handle,
            user_id,
            context,
            settings,
            avatar_image_cache,
            _roles_sub: roles_sub,
        }
    }
}

impl Focusable for UserProfilePopover {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for UserProfilePopover {}

impl Render for UserProfilePopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = resolve_user_profile(self.user_id, self.context, cx);
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();

        let banner_bg = gpui::rgb(0x0081_8cf8_u32);

        let (display_name, username, avatar_url, about_me, create_time, online) = match &profile {
            Some(p) => (
                SharedString::from(p.display_name.as_str()),
                SharedString::from(p.username.as_str()),
                SharedString::from(p.avatar_url.as_str()),
                SharedString::from(p.about_me.as_str()),
                p.create_time_seconds,
                p.online,
            ),
            None => (
                SharedString::default(),
                SharedString::default(),
                SharedString::default(),
                SharedString::default(),
                0u32,
                false,
            ),
        };

        let member_since = format_member_since(create_time);
        let dot_color = if online {
            theme.status_online
        } else {
            theme.status_offline
        };

        let mut avatar = Avatar::new()
            .name(display_name.clone())
            .size_px(px(90.))
            .image_cache(self.avatar_image_cache.clone());
        if !avatar_url.is_empty() {
            avatar = avatar.src(avatar_url);
        }

        let is_clan = matches!(self.context, ProfileContext::Clan(_));
        let clan_id_opt = match self.context {
            ProfileContext::Clan(id) => Some(id),
            _ => None,
        };
        let role_ids = profile
            .as_ref()
            .map(|p| p.role_ids.as_slice())
            .unwrap_or_default();
        let roles: Vec<(SharedString, SharedString)> = clan_id_opt
            .zip(RolesStore::try_global(cx))
            .map(|(clan_id, rs)| {
                rs.read(cx)
                    .roles_for(clan_id, role_ids)
                    .into_iter()
                    .map(|r| {
                        (
                            SharedString::from(r.name.as_str()),
                            SharedString::from(r.color.as_str()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        div()
            .occlude()
            .w(px(300.))
            .overflow_hidden()
            .rounded_lg()
            .bg(theme.bg_floating)
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .child(
                div()
                    .h(px(105.))
                    .rounded_tl_lg()
                    .rounded_tr_lg()
                    .bg(banner_bg)
                    .flex()
                    .justify_end()
                    .p_2(),
            )
            .child(
                div().flex().flex_row().px(px(16.)).mt(px(-50.)).child(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .child(
                            div()
                                .border(px(6.))
                                .border_color(theme.bg_floating)
                                .rounded_full()
                                .child(avatar),
                        )
                        .child(
                            div()
                                .absolute()
                                .bottom(px(4.))
                                .right(px(4.))
                                .size(px(16.))
                                .rounded_full()
                                .border_2()
                                .border_color(theme.bg_floating)
                                .bg(dot_color),
                        ),
                ),
            )
            .child(
                div().px(px(16.)).child(
                    div()
                        .rounded(px(10.))
                        .p_2()
                        .my(px(16.))
                        .bg(theme.bg_secondary)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_lg()
                                .text_color(theme.text_primary)
                                .overflow_hidden()
                                .child(display_name),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_muted)
                                .overflow_hidden()
                                .child(username),
                        )
                        .when(!about_me.is_empty(), |d| {
                            d.child(section_divider(theme.border))
                                .child(section_label(
                                    mezon_i18n::t(&locale, "common.userProfile.aboutMe"),
                                    theme.text_muted,
                                ))
                                .child(
                                    div()
                                        .mt_1()
                                        .text_sm()
                                        .text_color(theme.text_secondary)
                                        .child(about_me),
                                )
                        })
                        .when(create_time > 0, |d| {
                            d.child(section_divider(theme.border))
                                .child(section_label(
                                    mezon_i18n::t(&locale, "common.userProfile.memberSince"),
                                    theme.text_muted,
                                ))
                                .child(
                                    div()
                                        .mt_1()
                                        .text_sm()
                                        .text_color(theme.text_secondary)
                                        .child(member_since),
                                )
                        })
                        .when(is_clan && !roles.is_empty(), |d| {
                            d.child(section_divider(theme.border))
                                .child(section_label(SharedString::from("ROLES"), theme.text_muted))
                                .child(div().mt_1().flex().flex_wrap().gap_2().children(
                                    roles.iter().map(|(name, color)| {
                                        role_pill(name.clone(), color.as_ref(), theme)
                                    }),
                                ))
                        }),
                ),
            )
    }
}

fn section_divider(color: gpui::Rgba) -> gpui::AnyElement {
    div()
        .w_full()
        .p_1()
        .border_b_1()
        .border_color(color)
        .into_any_element()
}

fn section_label(text: impl Into<SharedString>, color: gpui::Rgba) -> gpui::AnyElement {
    div()
        .mt_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

fn format_member_since(create_time_seconds: u32) -> String {
    if create_time_seconds == 0 {
        return String::new();
    }
    let secs = create_time_seconds as i64;
    let days_since_epoch = secs / 86400;
    let (year, month, day) = days_to_date(days_since_epoch as u32);
    let month_name = MONTH_NAMES
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("Jan");
    format!("{month_name} {day:02}, {year}")
}

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn days_to_date(days_since_epoch: u32) -> (u32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn role_pill(name: SharedString, color: &str, theme: &Theme) -> AnyElement {
    let dot_color = parse_role_color(color).unwrap_or(theme.text_muted);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .bg(theme.bg_tertiary)
        .child(div().size(px(8.)).rounded_full().bg(dot_color))
        .child(div().text_xs().text_color(theme.text_secondary).child(name))
        .into_any_element()
}

fn parse_role_color(s: &str) -> Option<gpui::Rgba> {
    let s = s.trim().strip_prefix('#')?;
    let (r, g, b) = match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            (r, g, b)
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()?;
            let g = u8::from_str_radix(&s[1..2], 16).ok()?;
            let b = u8::from_str_radix(&s[2..3], 16).ok()?;
            (r * 17, g * 17, b * 17)
        }
        _ => return None,
    };
    Some(gpui::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    })
}

pub(crate) struct ClickableContainer(Stateful<Div>);

impl ClickableContainer {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self(div().id(id))
    }
}

impl ParentElement for ClickableContainer {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.0.extend(elements);
    }
}

impl Styled for ClickableContainer {
    fn style(&mut self) -> &mut StyleRefinement {
        self.0.style()
    }
}

impl IntoElement for ClickableContainer {
    type Element = Stateful<Div>;

    fn into_element(self) -> Stateful<Div> {
        self.0
    }
}

impl Clickable for ClickableContainer {
    fn on_click(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        Self(self.0.on_click(handler))
    }

    fn cursor_style(self, cursor_style: CursorStyle) -> Self {
        Self(self.0.cursor(cursor_style))
    }
}

impl Toggleable for ClickableContainer {
    fn toggle_state(self, _selected: bool) -> Self {
        self
    }
}

pub(crate) fn profile_popover_menu(
    id: impl Into<ElementId>,
    user_id: UserId,
    context: ProfileContext,
    settings: gpui::Entity<Settings>,
    avatar_image_cache: gpui::Entity<LruImageCache>,
) -> PopoverMenu<UserProfilePopover> {
    PopoverMenu::new(id)
        .menu(move |window, cx| {
            Some(cx.new(|cx| {
                UserProfilePopover::new(
                    user_id,
                    context,
                    settings.clone(),
                    avatar_image_cache.clone(),
                    window,
                    cx,
                )
            }))
        })
        .anchor(Anchor::TopRight)
        .attach(Anchor::TopLeft)
}
