pub mod compositions;
pub mod primitives;

use std::sync::Arc;

use gpui::{App, Window};

pub enum NavOp {
    Push(String),
    Replace(String),
    Back,
}

pub type NavigateFn = Arc<dyn Fn(NavOp, &mut App) + Send + Sync>;
pub type WindowAction = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>;
pub type TextChangeHandler = Arc<dyn Fn(&str, &mut Window, &mut App) + Send + Sync>;
pub type ToggleHandler = Arc<dyn Fn(bool, &mut Window, &mut App) + Send + Sync>;
pub type OtpCompleteHandler = Arc<dyn Fn(String, &mut Window, &mut App) + Send + Sync>;

// Flatten everything under `components::*`
pub use compositions::*;
pub use primitives::*;
