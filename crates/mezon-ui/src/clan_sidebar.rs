use gpui::{App, ClickEvent, Context, Entity, SharedString, Window, div, prelude::*, px};
use mezon_store::ClanList;

use gpui_component::Sizable;

use crate::components::primitives::{Avatar, Badge, Icon, IconName, Size};
use crate::theme::Theme;

pub struct ClanSidebar {
    list: Entity<ClanList>,
}

impl ClanSidebar {
    pub fn new(list: Entity<ClanList>, cx: &mut Context<Self>) -> Self {
        let _ = cx.observe(&list, |_, _, cx| cx.notify());
        Self { list }
    }
}

impl Render for ClanSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let list_handle = self.list.clone();
        let active_id = self.list.read(cx).active_clan_id.clone();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(theme.bg_primary)
            .gap_1()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Avatar::new().name("U").with_size(Size::Small)),
            )
            .child(div().w_full().h_1().bg(theme.border).my_1())
            .child(div().flex().flex_col().gap_3().flex_1().children(
                self.list.read(cx).clans.iter().map(|clan| {
                    let is_active = Some(&clan.id) == active_id.as_ref();
                    let unread = clan.unread_count;
                    let clan_id = clan.id.clone();
                    let list = list_handle.clone();

                    let mut clan_div = div()
                        .id(SharedString::from(format!("clan-{}", clan.id)))
                        .relative()
                        .w_full()
                        .cursor_pointer()
                        .when(is_active, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(4.))
                                    .bg(theme.brand),
                            )
                        });

                    clan_div.interactivity().on_click(
                        move |_event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                            list.update(cx, |m, cx| {
                                m.select_clan(&clan_id);
                                cx.notify();
                            });
                        },
                    );

                    let clan_div = clan_div.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .relative()
                            .child(
                                Avatar::new()
                                    .name(clan.initials.clone())
                                    .with_size(Size::Small),
                            )
                            .when(unread > 0, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .right_0()
                                        .child(Badge::new().count(unread as usize)),
                                )
                            }),
                    );

                    clan_div
                }),
            ))
            .child(Icon::new(IconName::Plus))
    }
}
