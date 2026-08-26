use std::rc::Rc;

use gpui::{
    Anchor, App, ClickEvent, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Window,
    anchored, deferred, div, img, prelude::*, px, relative, svg,
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
        disabled: bool,
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
    Submenu {
        label: SharedString,
        sub_text: Option<SharedString>,
        options: Vec<SubmenuOption>,
        open: bool,
        danger: bool,
        on_open: MenuHandler,
        on_select: SubmenuHandler,
        on_parent_click: Option<MenuHandler>,
    },
    Checkbox {
        label: SharedString,
        checked: bool,
        on_click: MenuHandler,
    },
    Separator,
}

#[derive(Clone)]
pub struct SubmenuOption {
    pub value: i32,
    pub label: SharedString,
    pub selected: bool,
    pub disabled: bool,
}

type SubmenuHandler = Rc<dyn Fn(i32, &mut Window, &mut App)>;

#[derive(IntoElement, Default)]
pub struct ContextMenu {
    items: Vec<Item>,
    quick_reactions: Vec<QuickReaction>,
    on_quick_reaction: Option<QuickReactionHandler>,
    on_reaction_close: Option<MenuHandler>,
    on_submenu_close: Option<MenuHandler>,
    on_dismiss: Option<DismissHandler>,
    anchor: Point<Pixels>,
}

const SUBMENU_WIDTH: f32 = 240.;
const MENU_WIDTH_ESTIMATE: f32 = 240.;
const QUICK_REACTION_EMOJI_PX: f32 = 24.;
const QUICK_REACTION_EMOJI_SOURCE_PX: u32 = 48;

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
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: Some(icon),
            trailing_icon: None,
            danger: false,
            disabled: false,
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
            disabled: false,
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
        self.items.push(Item::Entry {
            label: label.into(),
            leading_icon: Some(icon),
            trailing_icon: None,
            danger: true,
            disabled: false,
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
            disabled: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        if let Some(Item::Entry { disabled: flag, .. }) = self.items.last_mut() {
            *flag = disabled;
        }
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(Item::Separator);
        self
    }

    pub fn on_submenu_close(
        mut self,
        on_submenu_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_submenu_close = Some(Rc::new(on_submenu_close));
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

    pub fn submenu(
        mut self,
        label: impl Into<SharedString>,
        sub_text: Option<SharedString>,
        options: Vec<SubmenuOption>,
        open: bool,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
        on_select: impl Fn(i32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Submenu {
            label: label.into(),
            sub_text,
            options,
            open,
            danger: false,
            on_open: Rc::new(on_open),
            on_select: Rc::new(on_select),
            on_parent_click: None,
        });
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn danger_submenu(
        mut self,
        label: impl Into<SharedString>,
        sub_text: Option<SharedString>,
        options: Vec<SubmenuOption>,
        open: bool,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
        on_select: impl Fn(i32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Submenu {
            label: label.into(),
            sub_text,
            options,
            open,
            danger: true,
            on_open: Rc::new(on_open),
            on_select: Rc::new(on_select),
            on_parent_click: None,
        });
        self
    }

    /// A submenu whose parent row is itself clickable (mirrors React's "Mute
    /// Channel/Category" row: click = act now, hover = pick a duration).
    #[allow(clippy::too_many_arguments)]
    pub fn submenu_clickable(
        mut self,
        label: impl Into<SharedString>,
        sub_text: Option<SharedString>,
        options: Vec<SubmenuOption>,
        open: bool,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
        on_select: impl Fn(i32, &mut Window, &mut App) + 'static,
        on_parent_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Submenu {
            label: label.into(),
            sub_text,
            options,
            open,
            danger: false,
            on_open: Rc::new(on_open),
            on_select: Rc::new(on_select),
            on_parent_click: Some(Rc::new(on_parent_click)),
        });
        self
    }

    pub fn checkbox_item(
        mut self,
        label: impl Into<SharedString>,
        checked: bool,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Checkbox {
            label: label.into(),
            checked,
            on_click: Rc::new(on_click),
        });
        self
    }
}

pub struct ContextMenuProbeItem {
    pub kind: &'static str,
    pub label: String,
    pub disabled: bool,
    pub options: Vec<(i32, String)>,
}

impl ContextMenu {
    pub fn probe_items(&self) -> Vec<ContextMenuProbeItem> {
        self.items
            .iter()
            .map(|item| match item {
                Item::Separator => ContextMenuProbeItem {
                    kind: "separator",
                    label: String::new(),
                    disabled: false,
                    options: Vec::new(),
                },
                Item::Entry {
                    label,
                    danger,
                    disabled,
                    ..
                } => ContextMenuProbeItem {
                    kind: if *danger { "danger" } else { "item" },
                    label: label.to_string(),
                    disabled: *disabled,
                    options: Vec::new(),
                },
                Item::Submenu {
                    label,
                    options,
                    danger,
                    ..
                } => ContextMenuProbeItem {
                    kind: if *danger { "danger_submenu" } else { "submenu" },
                    label: label.to_string(),
                    disabled: false,
                    options: options
                        .iter()
                        .map(|option| (option.value, option.label.to_string()))
                        .collect(),
                },
                Item::Checkbox { label, checked, .. } => ContextMenuProbeItem {
                    kind: if *checked {
                        "checkbox_on"
                    } else {
                        "checkbox_off"
                    },
                    label: label.to_string(),
                    disabled: false,
                    options: Vec::new(),
                },
                Item::ReactionSubmenu { label, .. } => ContextMenuProbeItem {
                    kind: "reaction_submenu",
                    label: label.to_string(),
                    disabled: false,
                    options: Vec::new(),
                },
            })
            .collect()
    }

    pub fn probe_activate(
        &self,
        index: usize,
        value: Option<i32>,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<()> {
        self.probe_invoke(index, value, window, cx)?;
        if let Some(dismiss) = &self.on_dismiss {
            dismiss(window, cx);
        }
        Ok(())
    }

    fn probe_invoke(
        &self,
        index: usize,
        value: Option<i32>,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<()> {
        let item = self
            .items
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("no menu item at index {index}"))?;
        match item {
            Item::Entry {
                disabled, on_click, ..
            } => {
                if *disabled {
                    anyhow::bail!("menu item at index {index} is disabled");
                }
                on_click(window, cx);
                Ok(())
            }
            Item::Checkbox { on_click, .. } => {
                on_click(window, cx);
                Ok(())
            }
            Item::Submenu {
                options,
                on_open,
                on_select,
                on_parent_click,
                ..
            } => {
                let Some(value) = value else {
                    match on_parent_click {
                        Some(on_parent_click) => on_parent_click(window, cx),
                        None => on_open(window, cx),
                    }
                    return Ok(());
                };
                if !options.iter().any(|option| option.value == value) {
                    anyhow::bail!(
                        "submenu at index {index} has no option {value}; expected one of {:?}",
                        options.iter().map(|o| o.value).collect::<Vec<_>>()
                    );
                }
                on_select(value, window, cx);
                Ok(())
            }
            Item::Separator => anyhow::bail!("menu item at index {index} is a separator"),
            Item::ReactionSubmenu { .. } => {
                anyhow::bail!("menu item at index {index} is the reaction submenu")
            }
        }
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
        let danger_text = theme.danger_text;
        let danger_hover_bg = theme.danger_hover_bg;
        let brand = theme.brand;
        let dismiss = self.on_dismiss.clone();
        let on_reaction_close = self.on_reaction_close;
        let on_submenu_close = self.on_submenu_close;
        let flyout_open = self.items.iter().any(|item| {
            matches!(
                item,
                Item::Submenu { open: true, .. } | Item::ReactionSubmenu { open: true, .. }
            )
        });
        let close_open_flyouts: Option<MenuHandler> = (flyout_open
            && (on_reaction_close.is_some() || on_submenu_close.is_some()))
        .then(|| {
            let on_reaction_close = on_reaction_close.clone();
            let on_submenu_close = on_submenu_close.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                if let Some(close) = &on_reaction_close {
                    close(window, cx);
                }
                if let Some(close) = &on_submenu_close {
                    close(window, cx);
                }
            }) as MenuHandler
        });
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
            .image_cache(crate::image_cache::shared_emoji_cache(cx))
            .occlude();

        if !self.quick_reactions.is_empty() {
            let mut reaction_row = h_flex().gap_1().px(px(6.)).pt(px(4.)).pb(px(6.));
            for (index, reaction) in self.quick_reactions.into_iter().enumerate() {
                let emoji_id = reaction.emoji_id.clone();
                let shortname = reaction.shortname.clone();
                let src = crate::util::imgproxy::emoji_url_sized(
                    cx,
                    &reaction.emoji_id,
                    QUICK_REACTION_EMOJI_SOURCE_PX,
                );
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
                    cell = cell.child(
                        img(SharedString::from(src))
                            .id("quick-reaction-emoji-frames")
                            .size(px(QUICK_REACTION_EMOJI_PX))
                            .with_fallback(move || {
                                div()
                                    .size(px(QUICK_REACTION_EMOJI_PX))
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
                            }),
                    );
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
                    disabled,
                    on_click,
                } => {
                    let dismiss = dismiss.clone();
                    let label_color = if disabled {
                        muted
                    } else if is_danger {
                        danger_text
                    } else {
                        text
                    };
                    let icon_color = if disabled {
                        muted
                    } else if is_danger {
                        danger_text
                    } else {
                        muted
                    };
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
                            .when(disabled, |row| row.opacity(0.5).cursor_not_allowed())
                            .when(!disabled, |row| {
                                row.cursor_pointer().hover(|s| {
                                    if is_danger {
                                        s.bg(danger_hover_bg)
                                    } else {
                                        s.bg(hover)
                                    }
                                })
                            })
                            .when_some(close_open_flyouts.clone(), |row, close| {
                                row.on_hover(move |hovered, window, cx| {
                                    if *hovered {
                                        close(window, cx);
                                    }
                                })
                            })
                            .when(leading_icon.is_some(), |row| row.gap_2())
                            .when_some(leading_icon, |row, icon| {
                                let icon_el = if icon == IconName::Flower {
                                    svg()
                                        .path(icon.path())
                                        .size(px(24.))
                                        .flex_none()
                                        .text_color(icon_color)
                                } else {
                                    Icon::new(icon).size_4().text_color(icon_color)
                                };
                                row.child(icon_el)
                            })
                            .child(div().flex_1().child(label))
                            .when_some(trailing_icon, |row, icon| {
                                row.child(Icon::new(icon).size_4().text_color(icon_color))
                            })
                            .when(!disabled, |row| {
                                row.on_click(move |_: &ClickEvent, window, cx| {
                                    on_click(window, cx);
                                    if let Some(dismiss) = &dismiss {
                                        dismiss(window, cx);
                                    }
                                })
                            }),
                    );
                }
                Item::Submenu {
                    label,
                    sub_text,
                    options,
                    open,
                    danger: is_danger,
                    on_open,
                    on_select,
                    on_parent_click,
                } => {
                    let dismiss = dismiss.clone();
                    let parent_label_color = if is_danger { danger_text } else { text };
                    let submenu = if open {
                        let mut sub = v_flex()
                            .w(px(SUBMENU_WIDTH))
                            .p(px(6.))
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(bg)
                            .shadow_lg()
                            .occlude();
                        for (oi, option) in options.iter().enumerate() {
                            let value = option.value;
                            let disabled = option.disabled;
                            let on_select = on_select.clone();
                            let dismiss_o = dismiss.clone();
                            sub = sub.child(
                                h_flex()
                                    .id(("submenu-option", oi))
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .px(px(8.))
                                    .py(px(6.))
                                    .rounded(px(4.))
                                    .when(disabled, |row| row.opacity(0.5).cursor_default())
                                    .when(!disabled, |row| {
                                        row.cursor_pointer().hover(|s| s.bg(hover))
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .text_color(text)
                                            .truncate()
                                            .child(option.label.clone()),
                                    )
                                    .when(option.selected, |el| {
                                        el.child(
                                            Icon::new(IconName::Check).size_4().text_color(text),
                                        )
                                    })
                                    .when(!disabled, |el| {
                                        el.on_click(move |_: &ClickEvent, window, cx| {
                                            on_select(value, window, cx);
                                            if let Some(dismiss) = &dismiss_o {
                                                dismiss(window, cx);
                                            }
                                        })
                                    }),
                            );
                        }
                        Some(
                            div()
                                .absolute()
                                .top(px(-6.))
                                .w_0()
                                .h_0()
                                .when(submenu_open_left, |el| el.right(relative(1.)).mr(px(4.)))
                                .when(!submenu_open_left, |el| el.left(relative(1.)).ml(px(4.)))
                                .child(
                                    anchored()
                                        .anchor(if submenu_open_left {
                                            Anchor::TopRight
                                        } else {
                                            Anchor::TopLeft
                                        })
                                        .snap_to_window_with_margin(px(8.))
                                        .child(sub),
                                ),
                        )
                    } else {
                        None
                    };
                    let mut label_col = v_flex().flex_1().child(div().child(label));
                    if let Some(sub_text) = sub_text {
                        label_col = label_col.child(
                            div()
                                .ml(px(8.))
                                .mb(px(4.))
                                .text_xs()
                                .text_color(text)
                                .child(sub_text),
                        );
                    }
                    let parent_dismiss = dismiss.clone();
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
                            .text_color(parent_label_color)
                            .cursor_pointer()
                            .hover(|s| {
                                if is_danger {
                                    s.bg(danger_hover_bg)
                                } else {
                                    s.bg(hover)
                                }
                            })
                            .on_hover({
                                let close = close_open_flyouts.clone();
                                move |hovered, window, cx| {
                                    if *hovered {
                                        if !open && let Some(close) = &close {
                                            close(window, cx);
                                        }
                                        on_open(window, cx);
                                    }
                                }
                            })
                            .when_some(on_parent_click, |row, on_parent_click| {
                                row.on_click(move |_: &ClickEvent, window, cx| {
                                    on_parent_click(window, cx);
                                    if let Some(dismiss) = &parent_dismiss {
                                        dismiss(window, cx);
                                    }
                                })
                            })
                            .child(label_col)
                            .child(Icon::new(IconName::ChevronRight).size_4().text_color(muted))
                            .children(submenu),
                    );
                }
                Item::Checkbox {
                    label,
                    checked,
                    on_click,
                } => {
                    let dismiss = dismiss.clone();
                    panel = panel.child(
                        h_flex()
                            .id(("context-menu-item", index))
                            .w_full()
                            .items_center()
                            .gap_2()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(4.))
                            .text_sm()
                            .text_color(text)
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .when_some(close_open_flyouts.clone(), |row, close| {
                                row.on_hover(move |hovered, window, cx| {
                                    if *hovered {
                                        close(window, cx);
                                    }
                                })
                            })
                            .child(
                                div()
                                    .w(px(16.))
                                    .h(px(16.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(if checked { brand } else { border })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(checked, |d| d.bg(brand))
                                    .when(checked, |d| {
                                        d.child(
                                            Icon::new(IconName::Check)
                                                .size(px(11.))
                                                .text_color(gpui::white()),
                                        )
                                    }),
                            )
                            .child(div().flex_1().child(label))
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
                            let src = crate::util::imgproxy::emoji_url_sized(
                                cx,
                                &reaction.emoji_id,
                                QUICK_REACTION_EMOJI_SOURCE_PX,
                            );
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
                                        .id("reaction-sub-emoji-frames")
                                        .size(px(QUICK_REACTION_EMOJI_PX))
                                        .flex_none()
                                        .with_fallback(move || {
                                            div()
                                                .size(px(QUICK_REACTION_EMOJI_PX))
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
                            .on_hover({
                                let close = close_open_flyouts.clone();
                                move |hovered, window, cx| {
                                    if *hovered {
                                        if !open && let Some(close) = &close {
                                            close(window, cx);
                                        }
                                        on_open(window, cx);
                                    }
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
    // A window-wide backdrop catches a left-click anywhere outside the menu (and its
    // absolute submenu flyouts) and dismisses — independent of geometry or focus, so
    // menu items can use plain `on_click`. It must cover the WHOLE window, not just the
    // view that opened the menu, so it is anchored at the window origin with an oversized
    // surface (a plain `.inset_0()` here would size to the parent view, e.g. only the
    // channel list). It is deliberately NOT `.occlude()`: an occluding backdrop would
    // also swallow a RIGHT-click meant to open another item's context menu (right-click
    // passes through to the item, which just replaces `open_menu`). The menu panel and
    // its flyouts have their OWN `.occlude()`, so a click landing on them blocks this
    // backdrop (it never fires for menu items); a click anywhere else still hits this
    // topmost non-blocking hitbox → dismiss, while the underlying element also receives
    // the click (pass-through), matching the previous `on_mouse_down_out` behaviour.
    let dismiss = menu.on_dismiss.clone();
    deferred(
        div()
            .when_some(dismiss, |el, dismiss| {
                el.child(anchored().position(Point::default()).child(
                    div().w(px(100000.)).h(px(100000.)).on_mouse_down(
                        MouseButton::Left,
                        move |_: &MouseDownEvent, window, cx| {
                            dismiss(window, cx);
                        },
                    ),
                ))
            })
            .child(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(menu.anchor(position)),
            ),
    )
}
