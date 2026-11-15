//! Finding, announcing and installing a newer release.
//!
//! Kept apart from the rest of the app because none of it has anything to do
//! with translating: it is a small state machine a background task advances and
//! the settings window reads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::Context;
use tracing::{error, info, warn};

use super::App;
use crate::features::updater::{check_for_update, download_and_apply, UpdateInfo};
use crate::shared::i18n::t;
use crate::shared::utils::notify;

#[derive(Debug, Clone)]
pub(super) enum UpdateState {
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading { received: u64, total: u64 },
    /// Installed and waiting to be restarted. The restart happens on the UI
    /// thread so the settings are saved and the tray icon removed first.
    Installed(PathBuf),
    Error(String),
}

pub(super) type SharedUpdate = Arc<Mutex<UpdateState>>;

/// What the About page shows for the current state.
pub(super) struct UpdateRow {
    pub status: String,
    pub can_check: bool,
    pub can_install: bool,
    /// How far the download has got, when one is running with a known size.
    pub progress: Option<f32>,
    /// Where the build to install lives.
    pub url: Option<String>,
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

pub(super) fn describe(state: &UpdateState) -> UpdateRow {
    let row = |status: String, can_check: bool| UpdateRow {
        status,
        can_check,
        can_install: false,
        progress: None,
        url: None,
    };

    match state {
        UpdateState::Checking => row(t("Checking for updates…").to_string(), false),
        UpdateState::UpToDate => row(
            t("You are on the latest version ({version})")
                .replace("{version}", env!("CARGO_PKG_VERSION")),
            true,
        ),
        UpdateState::Available(info) => UpdateRow {
            status: if info.size > 0 {
                t("Version {version} is available · {size} MB")
                    .replace("{version}", &info.version)
                    .replace("{size}", &megabytes(info.size))
            } else {
                t("Version {version} is available").replace("{version}", &info.version)
            },
            can_check: true,
            can_install: true,
            progress: None,
            url: Some(info.url.clone()),
        },
        UpdateState::Downloading { received, total } => UpdateRow {
            // A ten-megabyte download with no sign of movement looks like a
            // hang. Bytes when the server gives a length, and a running total
            // when it does not.
            status: if *total > 0 {
                t("Downloading the update… {done} of {total} MB")
                    .replace("{done}", &megabytes(*received))
                    .replace("{total}", &megabytes(*total))
            } else {
                t("Downloading the update… {done} MB").replace("{done}", &megabytes(*received))
            },
            can_check: false,
            can_install: false,
            progress: (*total > 0).then(|| *received as f32 / *total as f32),
            url: None,
        },
        UpdateState::Installed(_) => row(t("Update installed — restarting…").to_string(), false),
        // Already a sentence written for the user; wrapping it in "Error:" only
        // makes it look like a crash report.
        UpdateState::Error(message) => row(message.clone(), true),
    }
}

pub(super) fn spawn_check(rt: &tokio::runtime::Runtime, state: SharedUpdate, ctx: Context) {
    rt.spawn(async move {
        let next = match check_for_update().await {
            Ok(Some(info)) => {
                info!(version = %info.version, "Update available");
                UpdateState::Available(info)
            }
            Ok(None) => UpdateState::UpToDate,
            Err(e) => {
                warn!(error = %e, "Update check failed");
                UpdateState::Error(e)
            }
        };
        *state.lock().unwrap_or_else(|p| p.into_inner()) = next;
        ctx.request_repaint();
    });
}

impl App {
    pub(super) fn update_row(&self) -> UpdateRow {
        describe(&self.update.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(super) fn check_for_update(&self, ctx: &Context) {
        *self.update.lock().unwrap_or_else(|e| e.into_inner()) = UpdateState::Checking;
        spawn_check(&self.rt, Arc::clone(&self.update), ctx.clone());
    }

    pub(super) fn install_update(&self, ctx: &Context, url: String) {
        *self.update.lock().unwrap_or_else(|e| e.into_inner()) = UpdateState::Downloading {
            received: 0,
            total: 0,
        };
        let state = Arc::clone(&self.update);
        let repaint = ctx.clone();
        self.rt.spawn(async move {
            let progress_state = Arc::clone(&state);
            let progress_ctx = repaint.clone();
            let outcome = download_and_apply(&url, move |received, total| {
                *progress_state.lock().unwrap_or_else(|p| p.into_inner()) =
                    UpdateState::Downloading { received, total };
                // The window is idle while this runs; without a nudge the bar
                // would only move when something else happened to wake it.
                progress_ctx.request_repaint();
            })
            .await;

            *state.lock().unwrap_or_else(|p| p.into_inner()) = match outcome {
                Ok(path) => UpdateState::Installed(path),
                Err(e) => {
                    error!(error = %e, "Update failed");
                    UpdateState::Error(e)
                }
            };
            repaint.request_repaint();
        });
    }

    /// Starts the newly installed build and quits this one.
    ///
    /// Done here rather than inside the installer, which runs on a worker
    /// thread and used to call `process::exit(0)` — that skipped saving the
    /// settings, left the tray icon behind, and from the outside looked exactly
    /// like the app crashing after a download.
    pub(super) fn restart_if_updated(&mut self, ctx: &Context) {
        let path = {
            let state = self.update.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                UpdateState::Installed(path) => path.clone(),
                _ => return,
            }
        };

        // The new copy opens on the About page, so the first thing the user sees
        // is the version they just installed rather than an app that vanished.
        match std::process::Command::new(&path)
            .arg("--settings")
            .arg("about")
            .spawn()
        {
            Ok(_) => {
                info!(path = %path.display(), "Restarting into the new version");
                self.shutdown(ctx);
            }
            Err(e) => {
                error!(error = %e, "Could not start the new version");
                *self.update.lock().unwrap_or_else(|p| p.into_inner()) = UpdateState::Error(
                    t("The update is installed but could not be started — open the program again")
                        .to_string(),
                );
            }
        }
    }

    /// Tells the user a new release exists, once per release.
    ///
    /// The app has no window of its own most of the time, so the only place a
    /// notice would otherwise appear is a settings page nobody has open. The
    /// version is remembered so the same release does not announce itself at
    /// every launch.
    pub(super) fn announce_update(&mut self) {
        if !self.settings.notify_about_updates {
            return;
        }

        let available = {
            let state = self.update.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                UpdateState::Available(info) => Some(info.version.clone()),
                _ => None,
            }
        };
        let Some(version) = available else { return };

        if self.settings.notified_version == version {
            return;
        }
        self.settings.notified_version.clone_from(&version);
        self.settings_dirty = true;

        info!(%version, "Notifying about a new release");
        notify::show(
            "Sakura Screen Translator",
            &t("Version {version} is available. You can install it in Settings → About.")
                .replace("{version}", &version),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_with_a_known_size_reports_how_far_it_has_got() {
        let row = describe(&UpdateState::Downloading {
            received: 5 * 1024 * 1024,
            total: 10 * 1024 * 1024,
        });
        assert_eq!(row.progress, Some(0.5));
        assert!(!row.can_check && !row.can_install);
    }

    #[test]
    fn a_download_of_unknown_size_has_no_bar_to_draw() {
        let row = describe(&UpdateState::Downloading {
            received: 1024,
            total: 0,
        });
        assert_eq!(row.progress, None);
    }

    #[test]
    fn only_an_available_release_can_be_installed() {
        let available = describe(&UpdateState::Available(UpdateInfo {
            version: "9.9.9".into(),
            url: "https://example.test/build".into(),
            size: 0,
        }));
        assert!(available.can_install);
        assert_eq!(available.url.as_deref(), Some("https://example.test/build"));

        for state in [
            UpdateState::Checking,
            UpdateState::UpToDate,
            UpdateState::Error("nope".into()),
        ] {
            assert!(!describe(&state).can_install, "{state:?}");
        }
    }

    #[test]
    fn a_failed_check_can_be_tried_again() {
        assert!(describe(&UpdateState::Error("offline".into())).can_check);
    }
}
