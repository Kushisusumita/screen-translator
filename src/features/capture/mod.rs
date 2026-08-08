pub mod overlay;
pub mod screenshot;
pub mod window_pick;

pub use overlay::{Geometry, OverlayState};
pub use screenshot::{
    capture_desktop_image, capture_region_for_ocr, foreground_window_bounds, virtual_desktop,
    Bounds,
};
