use std::ops::Range;

use gpui::{
    Along, App, Axis, Background, Bounds, Context, DragMoveEvent, Empty, Entity, EntityId,
    EventEmitter, Hsla, MouseButton, MouseDownEvent, Pixels, Point, Render, RenderOnce, Window,
    canvas, div, prelude::*, px,
};

use super::stack::h_flex;
use crate::theme::ActiveTheme;

#[derive(Clone)]
struct DragThumb((EntityId, bool));

impl Render for DragThumb {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct DragSlider(EntityId);

impl Render for DragSlider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub enum SliderEvent {
    Change(SliderValue),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SliderValue {
    Single(f32),
    Range(f32, f32),
}

impl From<f32> for SliderValue {
    fn from(value: f32) -> Self {
        SliderValue::Single(value)
    }
}

impl From<(f32, f32)> for SliderValue {
    fn from(value: (f32, f32)) -> Self {
        SliderValue::Range(value.0, value.1)
    }
}

impl Default for SliderValue {
    fn default() -> Self {
        SliderValue::Single(0.)
    }
}

impl SliderValue {
    pub fn start(&self) -> f32 {
        match self {
            SliderValue::Single(value) => *value,
            SliderValue::Range(start, _) => *start,
        }
    }

    pub fn end(&self) -> f32 {
        match self {
            SliderValue::Single(value) => *value,
            SliderValue::Range(_, end) => *end,
        }
    }

    fn set_start(&mut self, value: f32) {
        if let SliderValue::Range(_, end) = self {
            *self = SliderValue::Range(value.min(*end), *end);
        } else {
            *self = SliderValue::Single(value);
        }
    }

    fn set_end(&mut self, value: f32) {
        if let SliderValue::Range(start, _) = self {
            *self = SliderValue::Range(*start, value.max(*start));
        } else {
            *self = SliderValue::Single(value);
        }
    }

    fn is_range(&self) -> bool {
        matches!(self, SliderValue::Range(_, _))
    }
}

fn is_horizontal(axis: Axis) -> bool {
    matches!(axis, Axis::Horizontal)
}

pub struct SliderState {
    min: f32,
    max: f32,
    step: f32,
    value: SliderValue,
    percentage: Range<f32>,
    bounds: Bounds<Pixels>,
}

impl SliderState {
    pub fn new() -> Self {
        Self {
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: SliderValue::default(),
            percentage: (0.0..0.0),
            bounds: Bounds::default(),
        }
    }

    pub fn min(mut self, min: f32) -> Self {
        self.min = min;
        self.update_thumb_pos();
        self
    }

    pub fn max(mut self, max: f32) -> Self {
        self.max = max;
        self.update_thumb_pos();
        self
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn default_value(mut self, value: impl Into<SliderValue>) -> Self {
        self.value = value.into();
        self.update_thumb_pos();
        self
    }

    pub fn set_value(
        &mut self,
        value: impl Into<SliderValue>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.value = value.into();
        self.update_thumb_pos();
        cx.notify();
    }

    pub fn value(&self) -> SliderValue {
        self.value
    }

    fn percentage_to_value(&self, percentage: f32) -> f32 {
        self.min + (self.max - self.min) * percentage
    }

    fn value_to_percentage(&self, value: f32) -> f32 {
        let range = self.max - self.min;
        if range <= 0.0 {
            0.0
        } else {
            (value - self.min) / range
        }
    }

    fn update_thumb_pos(&mut self) {
        match self.value {
            SliderValue::Single(value) => {
                let percentage = self.value_to_percentage(value.clamp(self.min, self.max));
                self.percentage = 0.0..percentage;
            }
            SliderValue::Range(start, end) => {
                let clamped_start = start.clamp(self.min, self.max);
                let clamped_end = end.clamp(self.min, self.max);
                self.percentage =
                    self.value_to_percentage(clamped_start)..self.value_to_percentage(clamped_end);
            }
        }
    }

    fn update_value_by_position(
        &mut self,
        axis: Axis,
        position: Point<Pixels>,
        is_start: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.bounds;
        let step = self.step;

        let inner_pos = if is_horizontal(axis) {
            position.x - bounds.left()
        } else {
            bounds.bottom() - position.y
        };
        let total_size = bounds.size.along(axis);
        let percentage = inner_pos.clamp(px(0.), total_size) / total_size;

        let percentage = if is_start {
            percentage.clamp(0.0, self.percentage.end)
        } else {
            percentage.clamp(self.percentage.start, 1.0)
        };

        let value = self.percentage_to_value(percentage);
        let value = (value / step).round() * step;

        if is_start {
            self.percentage.start = percentage;
            self.value.set_start(value);
        } else {
            self.percentage.end = percentage;
            self.value.set_end(value);
        }
        cx.emit(SliderEvent::Change(self.value));
        cx.notify();
    }
}

impl Default for SliderState {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<SliderEvent> for SliderState {}

impl Render for SliderState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(IntoElement)]
pub struct Slider {
    state: Entity<SliderState>,
    axis: Axis,
    disabled: bool,
    percentage_positioning: bool,
}

impl Slider {
    pub fn new(state: &Entity<SliderState>) -> Self {
        Self {
            axis: Axis::Horizontal,
            state: state.clone(),
            disabled: false,
            percentage_positioning: false,
        }
    }

    pub fn horizontal(mut self) -> Self {
        self.axis = Axis::Horizontal;
        self
    }

    pub fn vertical(mut self) -> Self {
        self.axis = Axis::Vertical;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn percentage_positioning(mut self) -> Self {
        self.percentage_positioning = true;
        self
    }

    fn render_thumb(
        &self,
        start_pos: Pixels,
        percentage: f32,
        is_start: bool,
        bar_color: Background,
        thumb_color: Hsla,
        window: &mut Window,
        _cx: &mut App,
    ) -> impl IntoElement {
        let entity_id = self.state.entity_id();
        let axis = self.axis;
        let id = ("slider-thumb", is_start as u32);

        if self.disabled {
            return div().id(id);
        }

        div()
            .id(id)
            .absolute()
            .when(is_horizontal(axis), |this| {
                this.top(px(-5.))
                    .when(self.percentage_positioning, |this| {
                        this.left(gpui::relative(percentage))
                    })
                    .when(!self.percentage_positioning, |this| this.left(start_pos))
                    .ml(-px(8.))
            })
            .when(!is_horizontal(axis), |this| {
                this.bottom(start_pos).left(px(-5.)).mb(-px(8.))
            })
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .rounded_full()
            .bg(bar_color.opacity(0.5))
            .size_4()
            .p(px(1.))
            .child(
                div()
                    .flex_shrink_0()
                    .size_full()
                    .rounded_full()
                    .bg(thumb_color),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_drag(DragThumb((entity_id, is_start)), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(window.listener_for(
                &self.state,
                move |view, e: &DragMoveEvent<DragThumb>, window, cx| {
                    let DragThumb((id, is_start)) = e.drag(cx);
                    if *id != entity_id {
                        return;
                    }
                    view.update_value_by_position(axis, e.event.position, *is_start, window, cx)
                },
            ))
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let axis = self.axis;
        let entity_id = self.state.entity_id();
        let state = self.state.read(cx);
        let is_range = state.value().is_range();
        let percentage_start = state.percentage.start;
        let percentage_end = state.percentage.end;
        let bar_size = state.bounds.size.along(axis);
        let bar_start = percentage_start * bar_size;
        let bar_end = percentage_end * bar_size;

        let bar_color: Background = Hsla::from(cx.theme().brand).into();
        let thumb_color: Hsla = if self.percentage_positioning {
            cx.theme().tokens.text_theme_primary.into()
        } else {
            cx.theme().text_primary.into()
        };

        div()
            .id(("slider", self.state.entity_id()))
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .when(!is_horizontal(axis), |this| this.h(px(120.)))
            .when(is_horizontal(axis), |this| this.w_full())
            .child(
                h_flex()
                    .id("slider-bar-container")
                    .when(!self.disabled, |this| {
                        this.on_mouse_down(
                            MouseButton::Left,
                            window.listener_for(
                                &self.state,
                                move |state, e: &MouseDownEvent, window, cx| {
                                    let mut is_start = false;
                                    if is_range {
                                        let inner_pos = if is_horizontal(axis) {
                                            e.position.x - state.bounds.left()
                                        } else {
                                            state.bounds.bottom() - e.position.y
                                        };
                                        let center = (bar_end - bar_start) / 2.0 + bar_start;
                                        is_start = inner_pos < center;
                                    }
                                    state.update_value_by_position(
                                        axis, e.position, is_start, window, cx,
                                    )
                                },
                            ),
                        )
                    })
                    .when(!self.disabled && !is_range, |this| {
                        this.on_drag(DragSlider(entity_id), |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| drag.clone())
                        })
                        .on_drag_move(window.listener_for(
                            &self.state,
                            move |view, e: &DragMoveEvent<DragSlider>, window, cx| {
                                let DragSlider(id) = e.drag(cx);
                                if *id != entity_id {
                                    return;
                                }
                                view.update_value_by_position(
                                    axis,
                                    e.event.position,
                                    false,
                                    window,
                                    cx,
                                )
                            },
                        ))
                    })
                    .when(is_horizontal(axis), |this| {
                        this.items_center().h_6().w_full()
                    })
                    .when(!is_horizontal(axis), |this| {
                        this.justify_center().w_6().h_full()
                    })
                    .flex_shrink_0()
                    .child(
                        div()
                            .id("slider-bar")
                            .relative()
                            .when(is_horizontal(axis), |this| this.w_full().h_1p5())
                            .when(!is_horizontal(axis), |this| this.h_full().w_1p5())
                            .bg(bar_color.opacity(0.2))
                            .rounded_full()
                            .child(
                                div()
                                    .absolute()
                                    .when(is_horizontal(axis), |this| {
                                        this.h_full()
                                            .when(self.percentage_positioning, |this| {
                                                this.left(gpui::relative(percentage_start))
                                                    .right(gpui::relative(1.0 - percentage_end))
                                            })
                                            .when(!self.percentage_positioning, |this| {
                                                this.left(bar_start).right(bar_size - bar_end)
                                            })
                                    })
                                    .when(!is_horizontal(axis), |this| {
                                        this.w_full().bottom(bar_start).top(bar_size - bar_end)
                                    })
                                    .bg(bar_color)
                                    .rounded_full(),
                            )
                            .when(is_range, |this| {
                                this.child(self.render_thumb(
                                    bar_start,
                                    percentage_start,
                                    true,
                                    bar_color,
                                    thumb_color,
                                    window,
                                    cx,
                                ))
                            })
                            .child(self.render_thumb(
                                bar_end,
                                percentage_end,
                                false,
                                bar_color,
                                thumb_color,
                                window,
                                cx,
                            ))
                            .child({
                                let state = self.state.clone();
                                canvas(
                                    move |bounds, _, cx| state.update(cx, |r, _| r.bounds = bounds),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full()
                            }),
                    ),
            )
    }
}
