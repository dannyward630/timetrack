/* gui/main.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The TimeTrack desktop client, built on gpui.
//!
//! The entry point is `Application::new()`, which picks the platform for the
//! compositor it finds. That is only true of upstream gpui: the community fork
//! (gpui-ce) kept its backend in a separate `gpui_platform` crate and had no
//! `Application::new` at all. See the workspace manifest for the rest of the
//! migration.
//!
//! The bus connection deliberately lives on a dedicated thread with its own
//! async-io runtime (see `app.rs`); nothing bus-related is driven from a gpui
//! future, because gpui's executor threads have no tokio context and zbus'
//! `spawn_blocking` panics there.

mod app;
mod session;

use app::TimetrackView;
use gpui::{
    App, AppContext, Application, Bounds, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, px, size,
};

fn main() {
    // Wayland session check first (see the `wayland` skill): which backend
    // gpui is about to pick and whether the environment supports it. Logging
    // only — the backend choice itself stays inside gpui's `guess_compositor`
    // (non-empty WAYLAND_DISPLAY wins, then DISPLAY, else headless).
    session::log_startup();
    Application::new().run(|cx: &mut App| {
        // Tall enough for the 72px week total plus the quick-add row, the
        // per-project list and a few recent entries, without scrolling to
        // reach the buttons.
        let bounds = Bounds::centered(None, size(px(760.), px(720.)), cx);
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // No system title bar: the minimize, maximize and close
                // buttons live in the app's own header (see
                // `TimetrackView::render_titlebar`). Client decorations ask
                // the compositor to skip its frame; `titlebar: None` drops
                // the native title everywhere else.
                titlebar: None,
                window_decorations: Some(WindowDecorations::Client),
                // `Transparent` gives the window an alpha channel, so the
                // rounded content below actually cuts the corners out.
                // Without it the corners would paint black.
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |window, cx| {
                // Still names the window for the taskbar and pager, which
                // read this even with no frame drawn around it.
                window.set_window_title("TimeTrack");
                cx.new(TimetrackView::new)
            },
        );
    });
}
