use gpui::{
    Anchor, Context, Empty, Entity, FontWeight, MouseButton, MouseDownEvent, Point, Render,
    SharedString, Window, anchored, canvas, deferred, div, linear_color_stop, linear_gradient,
    point, prelude::*, px, transparent_white,
};

use mezon_store::DEFAULT_ROLE_COLOR;

use super::role_list_side_bar::{parse_role_color, role_color_or_default};
use super::role_setting_page::RoleSettingPage;
use crate::components::primitives::{
    Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

pub const ROLE_COLOR_PRESETS: &[&str] = &[
    "#1abc9c", "#2ecc71", "#3498db", "#9b59b6", "#e91e63", "#f1c40f", "#e67e22", "#e74c3c",
    "#95a5a6", "#607d8b", "#11806a", "#1f8b4c", "#206694", "#71368a", "#ad1457", "#c27c0e",
    "#e84300", "#992d22", "#979c9f", "#546e7a",
];

const PRESET_SWATCH_SIZE: f32 = 24.0;
const PRESET_GAP: f32 = 8.0;
const PRESET_COLUMNS: usize = 10;
const LARGE_SWATCH_HEIGHT: f32 = PRESET_SWATCH_SIZE * 2.0 + PRESET_GAP;

const SV_WIDTH: f32 = 220.0;
const SV_HEIGHT: f32 = 140.0;
const HUE_HEIGHT: f32 = 12.0;

#[derive(Clone)]
struct DragSv(gpui::EntityId);

impl Render for DragSv {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct DragHue(gpui::EntityId);

impl Render for DragHue {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub fn is_custom_role_color(color: &str) -> bool {
    !color.is_empty() && color != DEFAULT_ROLE_COLOR && !ROLE_COLOR_PRESETS.contains(&color)
}

pub fn hex_to_rgb(hex: &str) -> Option<(f32, f32, f32)> {
    let rgba = parse_role_color(hex)?;
    Some((rgba.r, rgba.g, rgba.b))
}

pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let segment = (h / 60.0) % 6.0;
    let x = c * (1.0 - (segment % 2.0 - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = match segment.floor() as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (r1 + m, g1 + m, b1 + m)
}

pub fn hsv_to_hex(h: f32, s: f32, v: f32) -> String {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    format!(
        "#{:02x}{:02x}{:02x}",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8
    )
}

fn hue_to_rgb(h: f32) -> gpui::Rgba {
    let (r, g, b) = hsv_to_rgb(h, 1.0, 1.0);
    gpui::Rgba { r, g, b, a: 1.0 }
}

const HUE_GRADIENT_RANGES: [(f32, f32); 6] = [
    (0.0, 60.0),
    (60.0, 120.0),
    (120.0, 180.0),
    (180.0, 240.0),
    (240.0, 300.0),
    (300.0, 360.0),
];

fn render_hue_gradient_bar() -> impl IntoElement {
    h_flex()
        .size_full()
        .children(HUE_GRADIENT_RANGES.iter().map(|(start, end)| {
            div()
                .flex_1()
                .h_full()
                .bg(linear_gradient(
                    90.0,
                    linear_color_stop(hue_to_rgb(*start), 0.0),
                    linear_color_stop(hue_to_rgb(*end), 1.0),
                ))
                .into_any_element()
        }))
}

pub fn render_role_color_section(
    page: &mut RoleSettingPage,
    _locale: &str,
    theme: &Theme,
    can_edit: bool,
    window: &mut Window,
    cx: &mut Context<RoleSettingPage>,
) -> impl IntoElement {
    page.render_role_color_controls(theme, can_edit, window, cx)
}

impl RoleSettingPage {
    fn render_role_color_controls(
        &mut self,
        theme: &Theme,
        can_edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.custom_color_picker_open {
            self.ensure_rgb_inputs(window, cx);
        }

        let selected_color = role_color_or_default(&self.draft_color);
        let is_custom_color = is_custom_role_color(&selected_color);
        let anchor_bounds = self.custom_color_anchor_bounds;
        let picker_open = self.custom_color_picker_open;

        h_flex()
            .gap_2()
            .items_start()
            .child(
                h_flex()
                    .flex_shrink_0()
                    .w(px(140.0))
                    .h(px(LARGE_SWATCH_HEIGHT))
                    .gap_2()
                    .child(self.render_large_color_swatch(
                        DEFAULT_ROLE_COLOR,
                        &selected_color,
                        false,
                        can_edit,
                        theme,
                        "role-color-default",
                        cx,
                    ))
                    .child(self.render_custom_color_swatch(
                        &selected_color,
                        is_custom_color,
                        can_edit,
                        theme,
                        cx,
                    )),
            )
            .child(self.render_preset_color_grid(&selected_color, can_edit, theme, cx))
            .when(picker_open && can_edit, |row| {
                row.child(deferred(
                    anchored()
                        .position(anchor_bounds.bottom_left())
                        .offset(point(px(0.0), px(8.0)))
                        .anchor(Anchor::TopLeft)
                        .snap_to_window_with_margin(px(8.0))
                        .child(
                            div()
                                .occlude()
                                .on_mouse_down_out(cx.listener(
                                    |this, event: &MouseDownEvent, _, cx| {
                                        if this.custom_color_anchor_bounds.contains(&event.position)
                                        {
                                            return;
                                        }
                                        this.close_custom_color_picker(cx);
                                    },
                                ))
                                .child(self.render_color_picker_popover(theme, window, cx)),
                        ),
                ))
            })
    }

    fn render_custom_color_swatch(
        &mut self,
        selected: &SharedString,
        is_custom: bool,
        can_edit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let color = selected.to_string();
        let page = cx.entity().clone();

        v_flex().flex_1().h_full().child(
            div()
                .id("role-color-custom")
                .relative()
                .w_full()
                .h_full()
                .rounded(px(4.0))
                .cursor_pointer()
                .when(can_edit, |el| {
                    el.on_click(cx.listener(move |this, _, window, cx| {
                        this.open_custom_color_picker(window, cx);
                    }))
                })
                .when(!can_edit, |el| el.opacity(0.5))
                .bg(parse_role_color(&color).unwrap_or(theme.text_muted))
                .child(
                    div().absolute().top_1().right_1().child(
                        Icon::new(IconName::PenEdit)
                            .size(px(10.0))
                            .text_color(gpui::rgb(0x000000)),
                    ),
                )
                .when(is_custom, |el| {
                    el.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::rgb(0xffffff))
                            .child("✓"),
                    )
                })
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            page.update(cx, |this, _| {
                                this.custom_color_anchor_bounds = bounds;
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
        )
    }

    fn render_color_picker_popover(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hue = self.picker_hue;
        let sat = self.picker_sat;
        let val = self.picker_val;
        let preview = hsv_to_hex(hue, sat, val);
        let hue_color = hue_to_rgb(hue);

        v_flex()
            .id("role-color-picker-popover")
            .w(px(240.0))
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .shadow_lg()
            .child(self.render_sv_field(hue, sat, val, hue_color, window, cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(24.0))
                            .rounded_full()
                            .border_1()
                            .border_color(theme.border)
                            .bg(parse_role_color(&preview).unwrap_or(theme.text_muted)),
                    )
                    .child(self.render_hue_slider(hue, window, cx)),
            )
            .child(self.render_rgb_inputs(theme, cx))
    }

    fn render_sv_field(
        &self,
        _hue: f32,
        sat: f32,
        val: f32,
        hue_color: gpui::Rgba,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cursor_x = sat * SV_WIDTH;
        let cursor_y = (1.0 - val) * SV_HEIGHT;
        let entity = cx.entity().clone();
        let entity_id = entity.entity_id();
        let entity_for_bounds = entity.clone();

        div()
            .id("role-color-sv-field")
            .relative()
            .w(px(SV_WIDTH))
            .h(px(SV_HEIGHT))
            .rounded(px(4.0))
            .overflow_hidden()
            .cursor_crosshair()
            .bg(hue_color)
            .child(div().absolute().inset_0().bg(linear_gradient(
                90.0,
                linear_color_stop(gpui::white(), 0.0),
                linear_color_stop(transparent_white(), 1.0),
            )))
            .child(div().absolute().inset_0().bg(linear_gradient(
                180.0,
                linear_color_stop(transparent_white(), 0.0),
                linear_color_stop(gpui::black(), 1.0),
            )))
            .child(
                div()
                    .absolute()
                    .left(px(cursor_x - 6.0))
                    .top(px(cursor_y - 6.0))
                    .size(px(12.0))
                    .rounded_full()
                    .border_2()
                    .border_color(gpui::white())
                    .shadow_sm(),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity_for_bounds.update(cx, |this, _| {
                            this.sv_field_bounds = bounds;
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(
                    &entity,
                    move |this, e: &gpui::MouseDownEvent, window, cx| {
                        this.update_sv_from_position(e.position, window, cx);
                    },
                ),
            )
            .on_drag(DragSv(entity_id), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(window.listener_for(
                &entity,
                move |this, e: &gpui::DragMoveEvent<DragSv>, window, cx| {
                    if e.drag(cx).0 != entity_id {
                        return;
                    }
                    this.update_sv_from_position(e.event.position, window, cx);
                },
            ))
    }

    fn render_hue_slider(
        &self,
        hue: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cursor_x = (hue / 360.0) * SV_WIDTH;
        let entity = cx.entity().clone();
        let entity_id = entity.entity_id();
        let entity_for_bounds = entity.clone();

        div()
            .id("role-color-hue-slider")
            .relative()
            .flex_1()
            .h(px(HUE_HEIGHT))
            .cursor_pointer()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .child(render_hue_gradient_bar()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-4.0))
                    .h(px(HUE_HEIGHT + 8.0))
                    .left(px(cursor_x - 1.5))
                    .w(px(3.0))
                    .rounded(px(1.5))
                    .bg(gpui::white())
                    .shadow_md(),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity_for_bounds.update(cx, |this, _| {
                            this.hue_slider_bounds = bounds;
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(
                    &entity,
                    move |this, e: &gpui::MouseDownEvent, window, cx| {
                        this.update_hue_from_position(e.position, window, cx);
                    },
                ),
            )
            .on_drag(DragHue(entity_id), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(window.listener_for(
                &entity,
                move |this, e: &gpui::DragMoveEvent<DragHue>, window, cx| {
                    if e.drag(cx).0 != entity_id {
                        return;
                    }
                    this.update_hue_from_position(e.event.position, window, cx);
                },
            ))
    }

    fn render_rgb_inputs(&self, theme: &Theme, _cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_end()
            .children(
                ["R", "G", "B"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| {
                        let input = match index {
                            0 => self.rgb_r_input.clone(),
                            1 => self.rgb_g_input.clone(),
                            _ => self.rgb_b_input.clone(),
                        };
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_center()
                                    .text_color(theme.text_secondary)
                                    .child(label),
                            )
                            .when_some(input, |col, input| {
                                col.child(
                                    div()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(theme.border)
                                        .bg(theme.tokens.bg_input_secondary)
                                        .child(Input::new(&input)),
                                )
                            })
                            .into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_center()
                            .text_color(theme.text_secondary)
                            .child("#"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.tokens.bg_input_secondary)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .child(hsv_to_hex(
                                self.picker_hue,
                                self.picker_sat,
                                self.picker_val,
                            )),
                    ),
            )
    }

    fn render_preset_color_grid(
        &self,
        selected: &SharedString,
        can_edit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let split = ROLE_COLOR_PRESETS.len().min(PRESET_COLUMNS);
        let (row_one, row_two) = ROLE_COLOR_PRESETS.split_at(split);

        v_flex()
            .flex_1()
            .gap(px(PRESET_GAP))
            .child(self.render_preset_color_row(row_one, selected, can_edit, theme, 0, cx))
            .child(self.render_preset_color_row(row_two, selected, can_edit, theme, split, cx))
    }

    fn render_preset_color_row(
        &self,
        colors: &[&str],
        selected: &SharedString,
        can_edit: bool,
        theme: &Theme,
        index_offset: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap(px(PRESET_GAP))
            .children(colors.iter().enumerate().map(|(index, color)| {
                self.render_color_swatch(
                    color,
                    selected,
                    false,
                    can_edit,
                    theme,
                    index_offset + index,
                    cx,
                )
                .into_any_element()
            }))
    }

    fn render_large_color_swatch(
        &self,
        color: &str,
        selected: &SharedString,
        is_custom: bool,
        can_edit: bool,
        theme: &Theme,
        id: impl Into<gpui::ElementId>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = id.into();
        let is_selected = selected.as_ref() == color && !is_custom;
        let color_value = color.to_string();
        let color_for_click = color_value.clone();
        div()
            .id(id)
            .flex_1()
            .h_full()
            .rounded(px(4.0))
            .cursor_pointer()
            .relative()
            .when(can_edit, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    this.close_custom_color_picker(cx);
                    this.set_draft_color(color_for_click.clone(), cx);
                }))
            })
            .when(!can_edit, |el| el.opacity(0.5))
            .bg(parse_role_color(&color_value).unwrap_or(theme.text_muted))
            .when(is_selected, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_base()
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui::rgb(0xffffff))
                        .child("✓"),
                )
            })
    }

    fn render_color_swatch(
        &self,
        color: &str,
        selected: &SharedString,
        is_custom: bool,
        can_edit: bool,
        theme: &Theme,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = selected.as_ref() == color && !is_custom;
        let color_value = color.to_string();
        let color_for_click = color_value.clone();
        div()
            .id(("role-color", index as u64))
            .size(px(PRESET_SWATCH_SIZE))
            .rounded(px(4.0))
            .cursor_pointer()
            .relative()
            .when(can_edit, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    this.close_custom_color_picker(cx);
                    this.set_draft_color(color_for_click.clone(), cx);
                }))
            })
            .when(!can_edit, |el| el.opacity(0.5))
            .bg(parse_role_color(&color_value).unwrap_or(theme.text_muted))
            .when(is_selected, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui::rgb(0xffffff))
                        .child("✓"),
                )
            })
    }

    pub(super) fn ensure_rgb_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rgb_r_input.is_some() {
            return;
        }

        let input_bg = cx.theme().tokens.bg_input_secondary;
        let (r, g, b) = hsv_to_rgb(self.picker_hue, self.picker_sat, self.picker_val);
        let r_val = ((r * 255.0).round() as u32).to_string();
        let g_val = ((g * 255.0).round() as u32).to_string();
        let b_val = ((b * 255.0).round() as u32).to_string();

        let rgb_r_input = cx.new(|cx| {
            InputState::new(window, cx)
                .height(px(28.0))
                .text_size(px(13.0))
                .borderless()
                .bg(input_bg)
        });
        let rgb_g_input = cx.new(|cx| {
            InputState::new(window, cx)
                .height(px(28.0))
                .text_size(px(13.0))
                .borderless()
                .bg(input_bg)
        });
        let rgb_b_input = cx.new(|cx| {
            InputState::new(window, cx)
                .height(px(28.0))
                .text_size(px(13.0))
                .borderless()
                .bg(input_bg)
        });

        rgb_r_input.update(cx, |input, cx| input.set_value(&r_val, window, cx));
        rgb_g_input.update(cx, |input, cx| input.set_value(&g_val, window, cx));
        rgb_b_input.update(cx, |input, cx| input.set_value(&b_val, window, cx));

        let sub_r = cx.subscribe(&rgb_r_input, |this, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.sync_picker_from_rgb_inputs(cx);
            }
        });
        let sub_g = cx.subscribe(&rgb_g_input, |this, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.sync_picker_from_rgb_inputs(cx);
            }
        });
        let sub_b = cx.subscribe(&rgb_b_input, |this, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.sync_picker_from_rgb_inputs(cx);
            }
        });

        self.rgb_r_input = Some(rgb_r_input);
        self.rgb_g_input = Some(rgb_g_input);
        self.rgb_b_input = Some(rgb_b_input);
        self._rgb_input_subs = vec![sub_r, sub_g, sub_b];
    }

    pub(super) fn sync_rgb_inputs_from_picker(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (r, g, b) = hsv_to_rgb(self.picker_hue, self.picker_sat, self.picker_val);
        let r_val = ((r * 255.0).round() as u32).to_string();
        let g_val = ((g * 255.0).round() as u32).to_string();
        let b_val = ((b * 255.0).round() as u32).to_string();

        if let Some(input) = &self.rgb_r_input {
            input.update(cx, |state, cx| {
                if state.value() != r_val {
                    state.set_value(&r_val, window, cx);
                }
            });
        }
        if let Some(input) = &self.rgb_g_input {
            input.update(cx, |state, cx| {
                if state.value() != g_val {
                    state.set_value(&g_val, window, cx);
                }
            });
        }
        if let Some(input) = &self.rgb_b_input {
            input.update(cx, |state, cx| {
                if state.value() != b_val {
                    state.set_value(&b_val, window, cx);
                }
            });
        }
    }

    pub(super) fn sync_picker_from_rgb_inputs(&mut self, cx: &mut Context<Self>) {
        let parse = |input: &Option<Entity<InputState>>| -> Option<u8> {
            let value: u8 = input.as_ref()?.read(cx).value().trim().parse().ok()?;
            Some(value)
        };
        let Some(r) = parse(&self.rgb_r_input) else {
            return;
        };
        let Some(g) = parse(&self.rgb_g_input) else {
            return;
        };
        let Some(b) = parse(&self.rgb_b_input) else {
            return;
        };
        let (h, s, v) = rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        self.picker_hue = h;
        self.picker_sat = s;
        self.picker_val = v;
        self.apply_picker_to_draft_color(cx);
    }

    fn update_sv_from_position(
        &mut self,
        position: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.sv_field_bounds;
        let inner_x = position.x - bounds.left();
        let inner_y = position.y - bounds.top();
        let width = bounds.size.width;
        let height = bounds.size.height;
        if width <= px(0.0) || height <= px(0.0) {
            return;
        }
        self.picker_sat = (inner_x / width).clamp(0.0, 1.0);
        self.picker_val = (1.0 - inner_y / height).clamp(0.0, 1.0);
        self.apply_picker_to_draft_color(cx);
        self.sync_rgb_inputs_from_picker(window, cx);
    }

    fn update_hue_from_position(
        &mut self,
        position: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.hue_slider_bounds;
        let inner_x = position.x - bounds.left();
        let width = bounds.size.width;
        if width <= px(0.0) {
            return;
        }
        self.picker_hue = ((inner_x / width) * 360.0).clamp(0.0, 360.0);
        self.apply_picker_to_draft_color(cx);
        self.sync_rgb_inputs_from_picker(window, cx);
    }
}
