use std::sync::Arc;

use gpui::{App, ClickEvent, Context, Entity, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelsModel, ClansModel};

use crate::components::compositions::channel_row::ChannelRow;
use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

pub struct ChannelSidebar {
    clans_model: Entity<ClansModel>,
    channels_model: Entity<ChannelsModel>,
    on_navigate: Option<Arc<dyn Fn(&str, &mut App) + Send + Sync>>,
}

impl ChannelSidebar {
    pub fn new(
        clans_model: Entity<ClansModel>,
        channels_model: Entity<ChannelsModel>,
        on_navigate: Option<Arc<dyn Fn(&str, &mut App) + Send + Sync>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx.observe(&clans_model, |_, _, cx| cx.notify());
        let _ = cx.observe(&channels_model, |_, _, cx| cx.notify());
        Self {
            clans_model,
            channels_model,
            on_navigate,
        }
    }
}

impl Render for ChannelSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let clans = self.clans_model.read(cx);
        let channels = self.channels_model.read(cx);

        let active_clan_name = clans
            .active_clan_id
            .as_ref()
            .and_then(|id| clans.clans.iter().find(|c| &c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Select a clan".to_string());

        let categories: Vec<_> = clans
            .active_clan_id
            .as_ref()
            .map(|id| channels.categories_for_clan(id))
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.name.clone(), c.collapsed, c.channels.clone()))
            .collect();

        let active_channel_id = channels.active_channel_id.clone();
        let channels_handle = self.channels_model.clone();
        let theme_clone = theme.clone();
        let on_navigate = self.on_navigate.clone();
        let active_clan_id_for_nav = clans.active_clan_id.clone();

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
                            let handle = channels_handle.clone();
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

                            header.interactivity().on_click(
                                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                    handle.update(cx, |m, cx| {
                                        m.toggle_category(&cat_name2);
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
                                            let row_handle = channels_handle.clone();
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

                                            row.interactivity().on_click(
                                                move |_: &ClickEvent,
                                                      _: &mut Window,
                                                      cx: &mut App| {
                                                    row_handle.update(cx, |m, cx| {
                                                        m.select_channel(&ch_id);
                                                        cx.notify();
                                                    });
                                                    if let Some(ref cb) = nav_inner {
                                                        if let Some(ref cid) = clan_id_inner {
                                                            let path = format!(
                                                                "/chat/clans/{}/channels/{}",
                                                                cid, ch_id
                                                            );
                                                            cb(&path, cx);
                                                        }
                                                    }
                                                },
                                            );

                                            row
                                        })
                                        .collect::<Vec<_>>()
                                })
                        },
                    )),
            )
    }
}
