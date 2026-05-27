use gpui::{App, ClickEvent, Context, Entity, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelList, ClanList, Settings};

use crate::components::compositions::channel_row::ChannelRow;
use crate::components::primitives::{Icon, IconName};
use crate::theme::resolve_theme;

fn on_channel_click(
    channel_list: Entity<ChannelList>,
    channel_id: String,
    on_navigate: Option<crate::components::NavigateFn>,
    clan_id: Option<String>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        channel_list.update(cx, |m, cx| {
            m.select_channel(&channel_id);
            m.mark_read(&channel_id);
            cx.notify();
        });
        if let Some(ref cb) = on_navigate
            && let Some(ref cid) = clan_id
        {
            let path = format!("/chat/clans/{}/channels/{}", cid, channel_id);
            cb(crate::components::NavOp::Push(path), cx);
        }
    }
}

pub struct ChannelSidebar {
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    on_navigate: Option<crate::components::NavigateFn>,
    settings: Entity<Settings>,
    collapsed: std::collections::HashSet<String>,
}

impl ChannelSidebar {
    pub fn new(
        clan_list: Entity<ClanList>,
        channel_list: Entity<ChannelList>,
        on_navigate: Option<crate::components::NavigateFn>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx.observe(&clan_list, |_, _, cx| cx.notify());
        let _ = cx.observe(&channel_list, |_, _, cx| cx.notify());
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        Self {
            clan_list,
            channel_list,
            on_navigate,
            settings,
            collapsed: std::collections::HashSet::new(),
        }
    }
}

impl Render for ChannelSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);
        let clans = self.clan_list.read(cx);
        let channels = self.channel_list.read(cx);

        let active_clan_name = clans
            .active_clan()
            .map(|c| c.name.as_str())
            .unwrap_or("Select a clan")
            .to_string();

        let categories: Vec<_> = clans
            .active_clan_id
            .as_ref()
            .map(|id| channels.categories_for_clan(id))
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                (
                    c.name.clone(),
                    self.collapsed.contains(&c.name),
                    c.channels.clone(),
                )
            })
            .collect();

        let active_channel_id = channels.active_channel_id.clone();
        let channel_list_handle = self.channel_list.clone();
        let theme_clone = theme.clone();
        let on_navigate = self.on_navigate.clone();
        let active_clan_id_for_nav = clans.active_clan_id.clone();
        let sidebar_entity = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(theme.bg_secondary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px_3()
                    .py_3()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(active_clan_name),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .children(categories.into_iter().map(
                        move |(cat_name, is_collapsed, cat_channels)| {
                            let cat_name2 = cat_name.clone();
                            let nav = on_navigate.clone();
                            let clan_id_for_nav = active_clan_id_for_nav.clone();

                            let mut header = div()
                                .id(SharedString::from(format!("cat-{}", cat_name)))
                                .flex()
                                .flex_row()
                                .items_center()
                                .w_full()
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .text_color(theme_clone.text_muted)
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(
                                    Icon::new(if is_collapsed {
                                        IconName::ArrowRight
                                    } else {
                                        IconName::ArrowDown
                                    })
                                    .size(px(12.0))
                                    .text_color(theme_clone.text_muted),
                                )
                                .child(div().ml_1().child(cat_name.clone()));

                            let sidebar_for_cat = sidebar_entity.clone();
                            header.interactivity().on_click(
                                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                    sidebar_for_cat.update(cx, |this, cx| {
                                        if this.collapsed.contains(&cat_name2) {
                                            this.collapsed.remove(&cat_name2);
                                        } else {
                                            this.collapsed.insert(cat_name2.clone());
                                        }
                                        cx.notify();
                                    });
                                },
                            );

                            div()
                                .flex()
                                .flex_col()
                                .child(header)
                                .children(if is_collapsed {
                                    vec![]
                                } else {
                                    cat_channels
                                        .iter()
                                        .map(|ch| {
                                            let ch_id = ch.id.clone();
                                            let row_handle = channel_list_handle.clone();
                                            let nav_inner = nav.clone();
                                            let clan_id_inner = clan_id_for_nav.clone();

                                            let mut row = div()
                                                .id(SharedString::from(format!("ch-{}", ch.id)))
                                                .child(
                                                    ChannelRow::new(
                                                        ch.name.clone(),
                                                        ch.channel_type,
                                                    )
                                                    .selected(
                                                        Some(&ch.id) == active_channel_id.as_ref(),
                                                    )
                                                    .unread(ch.unread)
                                                    .private(ch.private)
                                                    .render(&theme_clone),
                                                );

                                            row.interactivity().on_click(on_channel_click(
                                                row_handle,
                                                ch_id,
                                                nav_inner,
                                                clan_id_inner,
                                            ));

                                            row
                                        })
                                        .collect::<Vec<_>>()
                                })
                        },
                    )),
            )
    }
}
