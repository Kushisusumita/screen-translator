pub mod checker;
pub mod installer;

pub use checker::{check_for_update, UpdateInfo};
pub use installer::{cleanup_previous_version, download_and_apply};
