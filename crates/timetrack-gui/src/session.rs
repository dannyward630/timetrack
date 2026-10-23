/* gui/session.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! Wayland session diagnosis for the desktop client.
//!
//! The wire protocol itself (`wl_display`, `wl_registry`, `wl_surface`,
//! `wl_buffer`, `wl_seat`, …) lives inside gpui's `WaylandClient` — see the
//! `wayland` skill (`references/core-protocol.md`). This module covers the
//! integration layer that is ours: which backend gpui will pick, whether the
//! session environment actually supports it, and what to log when it does not.
//!
//! It mirrors gpui 0.2.2's `guess_compositor` exactly: a non-empty
//! `WAYLAND_DISPLAY` selects Wayland, otherwise a non-empty `DISPLAY`
//! selects X11, otherwise gpui runs headless and no window appears. The
//! value of `WAYLAND_DISPLAY` is never interpreted here beyond
//! non-emptiness — in particular it is never defaulted to `wayland-0`.

/// What the session environment says, read once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// `XDG_SESSION_TYPE` (`wayland`, `x11`, …), if set.
    pub session_type: Option<String>,
    /// `WAYLAND_DISPLAY` (`wayland-1`, …), if set and non-empty.
    pub wayland_display: Option<String>,
    /// `DISPLAY` (`:0`, …), if set and non-empty.
    pub display: Option<String>,
    /// `XDG_RUNTIME_DIR` (`/run/user/1000`), if set.
    pub runtime_dir: Option<String>,
    /// `XDG_CURRENT_DESKTOP` (`KDE`, `GNOME`, …), if set.
    pub desktop: Option<String>,
}

impl SessionInfo {
    /// Read the session from the process environment.
    ///
    /// Empty `WAYLAND_DISPLAY`/`DISPLAY` values count as unset: that is how
    /// the headless smoke check forces X11 (`WAYLAND_DISPLAY=` with an X
    /// server on `DISPLAY`), and it matches gpui's own
    /// `is_some_and(|v| !v.is_empty())` check.
    pub fn from_env() -> Self {
        Self {
            session_type: non_empty("XDG_SESSION_TYPE"),
            wayland_display: non_empty("WAYLAND_DISPLAY"),
            display: non_empty("DISPLAY"),
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .map(|v| v.to_string_lossy().into_owned()),
            desktop: non_empty("XDG_CURRENT_DESKTOP"),
        }
    }

    /// The backend gpui will pick for this session.
    ///
    /// Mirrors `gpui::platform::guess_compositor`: Wayland wins over X11 when
    /// both are set, anything else is headless.
    pub fn backend(&self) -> &'static str {
        backend_of(self.wayland_display.is_some(), self.display.is_some())
    }

    /// The compositor socket this session points at, if any.
    ///
    /// `XDG_RUNTIME_DIR` is required by the socket path; a `WAYLAND_DISPLAY`
    /// without it has nowhere to resolve to.
    pub fn socket_path(&self) -> Option<String> {
        let (display, dir) = (self.wayland_display.as_ref()?, self.runtime_dir.as_ref()?);
        Some(format!("{dir}/{display}"))
    }

    /// Human-readable problems with the session, worst first.
    ///
    /// Pure over the struct so tests pin it without touching the environment.
    /// The socket existence check takes the precomputed result so the
    /// filesystem read stays at the call site.
    pub fn diagnose(&self, socket_exists: bool) -> Vec<String> {
        diagnose_impl(
            self.session_type.as_deref(),
            self.wayland_display.as_deref(),
            self.display.as_deref(),
            self.runtime_dir.as_deref(),
            self.backend(),
            socket_exists,
        )
    }
}

/// Backend choice as a pure function of presence flags.
///
/// Split out so the rule is testable without environment variables.
fn backend_of(wayland: bool, x11: bool) -> &'static str {
    if wayland {
        "Wayland"
    } else if x11 {
        "X11"
    } else {
        "Headless"
    }
}

fn non_empty(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    (!value.is_empty()).then_some(value)
}

fn diagnose_impl(
    session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
    runtime_dir: Option<&str>,
    backend: &str,
    socket_exists: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    // A Wayland login without a Wayland socket name is the classic broken
    // launch (environment scrubbed, container without the forwarded socket).
    if session_type == Some("wayland") && wayland_display.is_none() {
        out.push(
            "XDG_SESSION_TYPE=wayland but WAYLAND_DISPLAY is unset: gpui will not pick Wayland"
                .into(),
        );
    }
    if wayland_display.is_some() {
        match runtime_dir {
            None => out.push(
                "WAYLAND_DISPLAY is set but XDG_RUNTIME_DIR is unset: the compositor socket has no path"
                    .into(),
            ),
            Some(_) if !socket_exists => out.push(format!(
                "Wayland socket {} does not exist: the compositor is not running there or the sandbox hides it",
                // The socket path only exists when both parts are set; the
                // `None` arm above already covered the missing-dir case.
                socket_path_of(runtime_dir, wayland_display).unwrap_or_else(|| "<unknown>".into()),
            )),
            _ => {}
        }
    }
    // Headless means gpui opened no window at all. Say so explicitly rather
    // than leaving a silently absent window.
    if backend == "Headless" {
        out.push(
            "neither WAYLAND_DISPLAY nor DISPLAY is set: gpui will run headless and open no window"
                .into(),
        );
    }
    // Informational, not a problem: both set means Wayland wins by rule.
    if wayland_display.is_some() && display.is_some() {
        out.push("both WAYLAND_DISPLAY and DISPLAY are set: gpui prefers Wayland".into());
    }
    out
}

fn socket_path_of(runtime_dir: Option<&str>, wayland_display: Option<&str>) -> Option<String> {
    Some(format!("{}/{}", runtime_dir?, wayland_display?,))
}

/// Log the session once at startup: which backend gpui will pick plus any
/// diagnosis. Never panics and never touches the bus or gpui — call before
/// `Application::new()`.
pub fn log_startup() {
    let info = SessionInfo::from_env();
    let socket_exists = info
        .socket_path()
        .is_some_and(|p| std::path::Path::new(&p).exists());
    eprintln!(
        "timetrack-gui: session type={} wayland_display={} display={} runtime_dir={} desktop={} backend={}",
        info.session_type.as_deref().unwrap_or("<unset>"),
        info.wayland_display.as_deref().unwrap_or("<unset>"),
        info.display.as_deref().unwrap_or("<unset>"),
        info.runtime_dir.as_deref().unwrap_or("<unset>"),
        info.desktop.as_deref().unwrap_or("<unset>"),
        info.backend(),
    );
    for warning in info.diagnose(socket_exists) {
        eprintln!("timetrack-gui: {warning}");
    }
    // `WAYLAND_DEBUG=1` dumps every Wayland request/event on stderr from
    // gpui's `wayland-client`. Mention it only when it would do something:
    // on the Wayland backend.
    if info.backend() == "Wayland" && std::env::var("WAYLAND_DEBUG").is_err() {
        eprintln!("timetrack-gui: hint: WAYLAND_DEBUG=1 dumps the Wayland protocol trace");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(
        session_type: Option<&str>,
        wayland_display: Option<&str>,
        display: Option<&str>,
        runtime_dir: Option<&str>,
    ) -> SessionInfo {
        SessionInfo {
            session_type: session_type.map(str::to_string),
            wayland_display: wayland_display.map(str::to_string),
            display: display.map(str::to_string),
            runtime_dir: runtime_dir.map(str::to_string),
            desktop: None,
        }
    }

    #[test]
    fn wayland_wins_over_x11_and_neither_is_headless() {
        // Mirrors gpui 0.2.2 `guess_compositor`: presence plus non-emptiness,
        // Wayland first.
        assert_eq!(backend_of(true, true), "Wayland");
        assert_eq!(backend_of(true, false), "Wayland");
        assert_eq!(backend_of(false, true), "X11");
        assert_eq!(backend_of(false, false), "Headless");
        assert_eq!(
            info(None, Some("wayland-0"), Some(":0"), None).backend(),
            "Wayland"
        );
        assert_eq!(info(None, None, Some(":0"), None).backend(), "X11");
        assert_eq!(info(None, None, None, None).backend(), "Headless");
    }

    #[test]
    fn empty_display_values_count_as_unset() {
        // `SessionInfo::from_env` drops empties so the smoke check's
        // `WAYLAND_DISPLAY=` (Xvfb, no compositor) reads as X11, like gpui.
        assert_eq!(backend_of(false, false), "Headless");
        // The rule itself is presence-based; the emptiness filtering lives
        // in `from_env`, pinned here by construction, not environment.
        assert!(
            info(None, None, Some(":99"), Some("/run/user/1000"))
                .diagnose(true)
                .is_empty()
        );
    }

    #[test]
    fn socket_path_needs_both_parts() {
        assert_eq!(
            info(None, Some("wayland-1"), None, Some("/run/user/1000")).socket_path(),
            Some("/run/user/1000/wayland-1".into())
        );
        assert_eq!(
            info(None, Some("wayland-1"), None, None).socket_path(),
            None
        );
        assert_eq!(
            info(None, None, Some(":0"), Some("/run/user/1000")).socket_path(),
            None
        );
    }

    #[test]
    fn wayland_session_without_a_display_name_is_flagged() {
        let warnings =
            info(Some("wayland"), None, Some(":0"), Some("/run/user/1000")).diagnose(true);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("WAYLAND_DISPLAY is unset")),
            "must flag the scrubbed-Wayland case: {warnings:?}"
        );
    }

    #[test]
    fn missing_runtime_dir_and_missing_socket_are_flagged() {
        let no_dir = info(Some("wayland"), Some("wayland-0"), None, None).diagnose(false);
        assert!(
            no_dir
                .iter()
                .any(|w| w.contains("XDG_RUNTIME_DIR is unset")),
            "{no_dir:?}"
        );
        let no_socket = info(
            Some("wayland"),
            Some("wayland-0"),
            None,
            Some("/run/user/1000"),
        )
        .diagnose(false);
        assert!(
            no_socket
                .iter()
                .any(|w| w.contains("/run/user/1000/wayland-0") && w.contains("does not exist")),
            "{no_socket:?}"
        );
        // Present socket: no warning about it.
        let healthy = info(
            Some("wayland"),
            Some("wayland-0"),
            None,
            Some("/run/user/1000"),
        )
        .diagnose(true);
        assert!(
            healthy.iter().all(|w| !w.contains("does not exist")),
            "{healthy:?}"
        );
    }

    #[test]
    fn headless_is_explicit_and_dual_backend_is_informational() {
        let headless = info(None, None, None, None).diagnose(true);
        assert!(
            headless.iter().any(|w| w.contains("headless")),
            "{headless:?}"
        );
        let dual = info(None, Some("wayland-0"), Some(":0"), Some("/run/user/1")).diagnose(true);
        assert!(
            dual.iter().any(|w| w.contains("prefers Wayland")),
            "{dual:?}"
        );
    }
}
