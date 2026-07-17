use std::rc::Rc;

use gpui::{
    App, ClickEvent, MouseDownEvent, Pixels, Point, SharedString, Window, anchored, deferred, div,
    img, prelude::*, px, relative,
};

use super::icon::{Icon, IconName};
use super::stack::{h_flex, v_flex};
use crate::theme::ActiveTheme;

type MenuHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type DismissHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type QuickReactionHandler = Rc<dyn Fn(String, String, &mut Window, &mut App) + 'static>;

#[derive(Clone)]
struct QuickReaction {
    emoji_id: String,
    shortname: String,
}

enum Item {
    Entry {
        label: SharedString,
        leading_icon: Option<IconName>,
        trailing_icon: Option<IconName>,
        danger: bool,
        on_click: MenuHandler,
    },
    ReactionSubmenu {
        label: SharedString,
        view_more_label: SharedString,
        reactions: Vec<QuickReaction>,
        open: bool,
        on_open: MenuHandler,
        on_react: QuickReactionHandler,
        on_view_more: MenuHandler,
    },
    Separator,
}

#[derive(IntoElement, Default)]
pub struct ContextMenu {
    items: Vec<Item>,
    quick_reactions: Vec<QuickReaction>,
    on_quick_reaction: Option<QuickReactionHandler>,
    on_reaction_close: Option<MenuHandler>,
    on_dismiss: Option<DismissHandler>,
    anchor: Point<Pixels>,
}

const SUBMENU_WIDTH: f32 = 240.;
const MENU_WIDTH_ESTIMATE: f32 = 240.;

impl ContextMenu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn item(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: None,
            trailing_icon: None,
            danger: false,
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
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: Some(icon),
            trailing_icon: None,
            danger: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn item_trailing_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: None,
            trailing_icon: Some(icon),
            danger: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn danger_item(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: None,
            trailing_icon: None,
            danger: true,
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
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: Some(icon),
            trailing_icon: None,
            danger: true,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn danger_item_trailing_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: None,
            trailing_icon: Some(icon),
            danger: true,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(Item::Separator);
        self
    }

    pub fn on_dismiss(mut self, on_dismiss: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// The screen position the menu is anchored at — used to decide whether a
    /// reaction submenu opens to the right or flips left near the window edge.
    pub fn anchor(mut self, anchor: Point<Pixels>) -> Self {
        self.anchor = anchor;
        self
    }

    /// A top row of quick-reaction emojis, shown above the menu items.
    pub fn quick_reactions(
        mut self,
        emojis: impl IntoIterator<Item = (String, String)>,
        on_react: impl Fn(String, String, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.quick_reactions = emojis
            .into_iter()
            .filter(|(id, _)| !id.is_empty())
            .take(4)
            .map(|(emoji_id, shortname)| QuickReaction {
                emoji_id,
                shortname,
            })
            .collect();
        self.on_quick_reaction = Some(Rc::new(on_react));
        self
    }

    /// Called when any non-submenu item is hovered — the message menu uses this
    /// to close an open reaction submenu.
    pub fn on_reaction_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reaction_close = Some(Rc::new(handler));
        self
    }

    /// An "Add Reaction" item that reveals a hover submenu of recent emojis plus
    /// a "view more" entry. `open` is owned by the caller (it toggles it from the
    /// `on_open`/`on_reaction_close` hovers) so the submenu survives menu rebuilds.
    #[allow(clippy::too_many_arguments)]
    pub fn reaction_submenu(
        mut self,
        label: impl Into<SharedString>,
        view_more_label: impl Into<SharedString>,
        emojis: impl IntoIterator<Item = (String, String)>,
        open: bool,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
        on_react: impl Fn(String, String, &mut Window, &mut App) + 'static,
        on_view_more: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        let reactions = emojis
            .into_iter()
            .filter(|(id, _)| !id.is_empty())
            .take(4)
            .map(|(emoji_id, shortname)| QuickReaction {
                emoji_id,
                shortname,
            })
            .collect();
        self.items.push(Item::ReactionSubmenu {
            label: label.into(),
            view_more_label: view_more_label.into(),
            reactions,
            open,
            on_open: Rc::new(on_open),
            on_react: Rc::new(on_react),
            on_view_more: Rc::new(on_view_more),
        });
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.tokens.bg_theme_contexify;
        let border = theme.border;
        let text = theme.tokens.text_theme_primary;
        let muted = theme.text_secondary;
        let hover = theme.bg_hover;
        let danger = theme.status_dnd;
        let dismiss = self.on_dismiss.clone();
        let on_reaction_close = self.on_reaction_close;
        let on_quick_reaction = self.on_quick_reaction;
        // Flip the reaction submenu to the left of the menu when it would overflow
        // the window's right edge.
        let submenu_open_left = self.anchor.x + px(MENU_WIDTH_ESTIMATE) + px(SUBMENU_WIDTH)
            > window.viewport_size().width;

        let mut panel = v_flex()
            .min_w(px(220.))
            .p(px(6.))
            .rounded_md()
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

        if !self.quick_reactions.is_empty() {
            let mut reaction_row = h_flex().gap_1().px(px(6.)).pt(px(4.)).pb(px(6.));
            for (index, reaction) in self.quick_reactions.into_iter().enumerate() {
                let emoji_id = reaction.emoji_id.clone();
                let shortname = reaction.shortname.clone();
                let src = crate::util::imgproxy::emoji_url(cx, &reaction.emoji_id);
                let dismiss_click = dismiss.clone();
                let on_react = on_quick_reaction.clone();
                let mut cell = div()
                    .id(("context-menu-reaction", index))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(hover))
                    .on_click(move |_: &ClickEvent, window, cx| {
                        if let Some(on_react) = &on_react {
                            on_react(emoji_id.clone(), shortname.clone(), window, cx);
                        }
                        if let Some(dismiss) = &dismiss_click {
                            dismiss(window, cx);
                        }
                    });
                if !src.is_empty() {
                    let fallback_color = muted;
                    cell = cell.child(img(SharedString::from(src)).size(px(24.)).with_fallback(
                        move || {
                            div()
                                .size(px(24.))
                                .rounded(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::ImageThumbnail)
                                        .size(px(16.))
                                        .text_color(fallback_color),
                                )
                                .into_any_element()
                        },
                    ));
                }
                reaction_row = reaction_row.child(cell);
            }
            panel = panel
                .child(reaction_row)
                .child(div().my(px(5.)).h(px(1.)).w_full().bg(border));
        }

        for (index, item) in self.items.into_iter().enumerate() {
            match item {
                Item::Separator => {
                    panel = panel.child(div().my(px(5.)).h(px(1.)).w_full().bg(border));
                }
                Item::Entry {
                    label,
                    leading_icon,
                    trailing_icon,
                    danger: is_danger,
                    on_click,
                } => {
                    let dismiss = dismiss.clone();
                    let label_color = if is_danger { danger } else { text };
                    let icon_color = if is_danger { danger } else { muted };
                    panel = panel.child(
                        h_flex()
                            .id(("context-menu-item", index))
                            .w_full()
                            .items_center()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(4.))
                            .text_sm()
                            .text_color(label_color)
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .when_some(on_reaction_close.clone(), |row, close| {
                                row.on_hover(move |hovered, window, cx| {
                                    if *hovered {
                                        close(window, cx);
                                    }
                                })
                            })
                            .when(leading_icon.is_some(), |row| row.gap_2())
                            .when_some(leading_icon, |row, icon| {
                                row.child(Icon::new(icon).size_4().text_color(icon_color))
                            })
                            .child(div().flex_1().child(label))
                            .when_some(trailing_icon, |row, icon| {
                                row.child(Icon::new(icon).size_4().text_color(icon_color))
                            })
                            .on_click(move |_: &ClickEvent, window, cx| {
                                on_click(window, cx);
                                if let Some(dismiss) = &dismiss {
                                    dismiss(window, cx);
                                }
                            }),
                    );
                }
                Item::ReactionSubmenu {
                    label,
                    view_more_label,
                    reactions,
                    open,
                    on_open,
                    on_react,
                    on_view_more,
                } => {
                    let dismiss = dismiss.clone();
                    let submenu = if open {
                        let mut sub = v_flex()
                            .absolute()
                            .top(px(-6.))
                            .when(submenu_open_left, |el| el.right(relative(1.)).mr(px(4.)))
                            .when(!submenu_open_left, |el| el.left(relative(1.)).ml(px(4.)))
                            .w(px(SUBMENU_WIDTH))
                            .p(px(6.))
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(bg)
                            .shadow_lg()
                            .occlude();
                        for (ri, reaction) in reactions.iter().enumerate() {
                            let emoji_id = reaction.emoji_id.clone();
                            let shortname = reaction.shortname.clone();
                            let src = crate::util::imgproxy::emoji_url(cx, &reaction.emoji_id);
                            let shortname_label = SharedString::from(reaction.shortname.clone());
                            let on_react = on_react.clone();
                            let dismiss_r = dismiss.clone();
                            let mut row = h_flex()
                                .id(("reaction-sub", ri))
                                .w_full()
                                .items_center()
                                .gap_2()
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(hover))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(text)
                                        .truncate()
                                        .child(shortname_label),
                                );
                            if !src.is_empty() {
                                row = row.child(
                                    img(SharedString::from(src))
                                        .size(px(24.))
                                        .flex_none()
                                        .with_fallback(move || {
                                            div()
                                                .size(px(24.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    Icon::new(IconName::ImageThumbnail)
                                                        .size(px(16.))
                                                        .text_color(muted),
                                                )
                                                .into_any_element()
                                        }),
                                );
                            }
                            sub = sub.child(row.on_click(move |_: &ClickEvent, window, cx| {
                                on_react(emoji_id.clone(), shortname.clone(), window, cx);
                                if let Some(dismiss) = &dismiss_r {
                                    dismiss(window, cx);
                                }
                            }));
                        }
                        sub = sub.child(div().my(px(5.)).h(px(1.)).w_full().bg(border));
                        let dismiss_vm = dismiss.clone();
                        Some(
                            sub.child(
                                h_flex()
                                    .id("reaction-view-more")
                                    .w_full()
                                    .px(px(8.))
                                    .py(px(6.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(hover))
                                    .text_sm()
                                    .text_color(text)
                                    .child(view_more_label)
                                    .on_click(move |_: &ClickEvent, window, cx| {
                                        on_view_more(window, cx);
                                        if let Some(dismiss) = &dismiss_vm {
                                            dismiss(window, cx);
                                        }
                                    }),
                            ),
                        )
                    } else {
                        None
                    };
                    panel = panel.child(
                        h_flex()
                            .id(("context-menu-item", index))
                            .relative()
                            .w_full()
                            .items_center()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(4.))
                            .text_sm()
                            .text_color(text)
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .on_hover(move |hovered, window, cx| {
                                if *hovered {
                                    on_open(window, cx);
                                }
                            })
                            .child(div().flex_1().child(label))
                            .child(Icon::new(IconName::ChevronRight).size_4().text_color(muted))
                            .children(submenu),
                    );
                }
            }
        }

        panel
    }
}

pub fn context_menu_at(position: Point<Pixels>, menu: ContextMenu) -> impl IntoElement {
    deferred(
        anchored()
            .position(position)
            .snap_to_window()
            .child(menu.anchor(position)),
    )
}
