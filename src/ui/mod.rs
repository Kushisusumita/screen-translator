//! Sakura design system — tokens, platform adaptation, icons, widgets.
//!
//! This layer knows nothing about translation or screen capture. It is the only
//! place that decides what the product looks like, so a visual change lands here
//! and nowhere else.

pub mod icons;
pub mod platform;
pub mod spin;
pub mod theme;
pub mod widgets;

pub use platform::Platform;
pub use theme::Theme;
