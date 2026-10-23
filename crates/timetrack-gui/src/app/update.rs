/* gui/app/update.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The AppImage self-update state: what the check found, and what `U` does.
//!
//! The check itself runs on a background thread (see `service.rs`): it is a
//! blocking HTTPS call and must never run on a gpui future. This module is
//! the rest -- plain state over strings, so the whole key interaction is
//! unit-testable without a window.
//!
//! Installing is a destructive action, so `U` behaves like the merge and
//! delete confirmations: the first press only arms it, the second one sends
//! the install. The download then runs on its own thread and reports back
//! over the same channel as everything else.

/// A newer release carrying this machine's AppImage asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AvailableUpdate {
    pub(super) version: String,
    pub(super) url: String,
}

/// What `U` does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UpdateKey {
    /// Show this in the footer.
    Note(String),
    /// Download this asset and rename it over the running AppImage.
    Install { url: String, version: String },
}

/// The update half of the view state.
#[derive(Debug, Clone, Default)]
pub(super) struct UpdateState {
    /// A newer release with an asset for this arch, if the check found one.
    pub(super) available: Option<AvailableUpdate>,
    /// The first `U` arms the install; the second one sends it.
    pub(super) confirmed: bool,
    /// A download is in flight on its own thread.
    pub(super) installing: bool,
    /// The installed version, once the download renamed over the image.
    pub(super) installed: Option<String>,
    /// The check ran and found nothing newer.
    pub(super) checked: bool,
    /// Whether this process is an AppImage at all. A host `cargo build`
    /// has no release to update to, so `U` says so instead of checking.
    pub(super) applies: bool,
    /// Whether the automatic check is opted out (`TIMETRACK_NO_UPDATE`).
    /// `U` reports this rather than pretending a check is still running.
    pub(super) disabled: bool,
}

impl UpdateState {
    /// Handle a `U` press. Pure: the caller turns `Install` into an action
    /// and `Note` into a footer status.
    pub(super) fn on_u(&mut self) -> UpdateKey {
        if let Some(version) = self.installed.clone() {
            return UpdateKey::Note(format!("v{version} installed — relaunch to run it"));
        }
        if self.installing {
            return UpdateKey::Note("downloading the update…".to_string());
        }
        if let Some(found) = self.available.clone() {
            if !self.confirmed {
                self.confirmed = true;
                return UpdateKey::Note(format!(
                    "press U again to download v{} and replace this AppImage",
                    found.version
                ));
            }
            self.installing = true;
            return UpdateKey::Install {
                url: found.url,
                version: found.version,
            };
        }
        if !self.applies {
            return UpdateKey::Note(
                "updates apply to the AppImage; this is a host build".to_string(),
            );
        }
        if self.disabled {
            return UpdateKey::Note(
                "automatic update checks are off (TIMETRACK_NO_UPDATE=1)".to_string(),
            );
        }
        if !self.checked {
            return UpdateKey::Note("still checking for updates…".to_string());
        }
        UpdateKey::Note("already up to date".to_string())
    }

    /// Whether the footer hints should offer `U`.
    pub(super) fn offer_update(&self) -> Option<&str> {
        if self.installed.is_some() || self.installing {
            None
        } else {
            self.available.as_ref().map(|a| a.version.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available() -> UpdateState {
        UpdateState {
            available: Some(AvailableUpdate {
                version: "0.2.0".into(),
                url: "https://example.com/TimeTrack-x86_64.AppImage".into(),
            }),
            applies: true,
            checked: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_u_only_arms_the_install() {
        let mut state = available();
        let key = state.on_u();
        assert!(
            matches!(key, UpdateKey::Note(_)),
            "arming must not install: {key:?}"
        );
        assert!(state.confirmed);
        assert!(!state.installing);
    }

    #[test]
    fn the_second_u_sends_the_install() {
        let mut state = available();
        let _ = state.on_u();
        let key = state.on_u();
        assert_eq!(
            key,
            UpdateKey::Install {
                url: "https://example.com/TimeTrack-x86_64.AppImage".into(),
                version: "0.2.0".into(),
            }
        );
        assert!(state.installing);
    }

    #[test]
    fn while_downloading_u_only_reports_it() {
        let mut state = available();
        let _ = state.on_u();
        let _ = state.on_u();
        assert!(matches!(state.on_u(), UpdateKey::Note(_)));
        assert!(state.installing, "a third press must not re-send");
    }

    #[test]
    fn after_install_u_reminds_to_relaunch() {
        let mut state = UpdateState {
            installed: Some("0.2.0".into()),
            applies: true,
            ..Default::default()
        };
        assert!(matches!(state.on_u(), UpdateKey::Note(_)));
        assert_eq!(state.offer_update(), None);
    }

    #[test]
    fn a_host_build_says_so() {
        let mut state = UpdateState::default();
        assert_eq!(
            state.on_u(),
            UpdateKey::Note("updates apply to the AppImage; this is a host build".into())
        );
    }

    #[test]
    fn a_current_appimage_says_so_once_checked() {
        let mut state = UpdateState {
            applies: true,
            checked: true,
            ..Default::default()
        };
        assert_eq!(state.on_u(), UpdateKey::Note("already up to date".into()));
    }

    #[test]
    fn an_opted_out_appimage_says_so() {
        let mut state = UpdateState {
            applies: true,
            disabled: true,
            ..Default::default()
        };
        assert_eq!(
            state.on_u(),
            UpdateKey::Note("automatic update checks are off (TIMETRACK_NO_UPDATE=1)".into())
        );
    }

    #[test]
    fn before_the_check_finishes_u_waits() {
        let mut state = UpdateState {
            applies: true,
            ..Default::default()
        };
        assert_eq!(
            state.on_u(),
            UpdateKey::Note("still checking for updates…".into())
        );
    }
}
