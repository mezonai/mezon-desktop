use gpui::SharedString;
/// Label — stateless text element with size / weight / colour variants.
use gpui::{FontWeight, Rgba, div, prelude::*};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSize {
    Xs,  // text-xs  (~10px)
    Sm,  // text-sm  (~12px)
    Md,  // text-base (~14px, default)
    Lg,  // text-lg  (~16px)
    Xl,  // text-xl  (~20px)
    Xl2, // text-2xl (~24px)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelWeight {
    Normal,
    Medium,
    SemiBold,
    Bold,
}

pub struct Label {
    text: SharedString,
    size: LabelSize,
    weight: LabelWeight,
    color: Option<Rgba>,
    muted: bool,
    secondary: bool,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: LabelSize::Md,
            weight: LabelWeight::Normal,
            color: None,
            muted: false,
            secondary: false,
        }
    }

    pub fn size(mut self, size: LabelSize) -> Self {
        self.size = size;
        self
    }

    pub fn weight(mut self, weight: LabelWeight) -> Self {
        self.weight = weight;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// Use `text_secondary` colour from theme.
    pub fn secondary(mut self) -> Self {
        self.secondary = true;
        self
    }

    /// Use `text_muted` colour from theme.
    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    pub fn bold(mut self) -> Self {
        self.weight = LabelWeight::Bold;
        self
    }

    pub fn semibold(mut self) -> Self {
        self.weight = LabelWeight::SemiBold;
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let color = if let Some(c) = self.color {
            c
        } else if self.muted {
            theme.text_muted
        } else if self.secondary {
            theme.text_secondary
        } else {
            theme.text_primary
        };

        let weight = match self.weight {
            LabelWeight::Normal => FontWeight::NORMAL,
            LabelWeight::Medium => FontWeight::MEDIUM,
            LabelWeight::SemiBold => FontWeight::SEMIBOLD,
            LabelWeight::Bold => FontWeight::BOLD,
        };

        let el = div().font_weight(weight).text_color(color).child(self.text);

        match self.size {
            LabelSize::Xs => el.text_xs(),
            LabelSize::Sm => el.text_sm(),
            LabelSize::Md => el.text_base(),
            LabelSize::Lg => el.text_lg(),
            LabelSize::Xl => el.text_xl(),
            LabelSize::Xl2 => el.text_2xl(),
        }
    }
}
