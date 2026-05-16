mod checker;
mod installer;

pub use checker::{check_for_update, UpdateInfo};
pub use installer::download_and_apply;
