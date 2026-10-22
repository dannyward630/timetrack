/* gui/app/chrome.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The window's own chrome: the draggable titlebar regions and the window
//! controls.
//!
//! The window has no system title bar (see `main.rs`), so the drag handle,
//! the tab strip and the minimize/maximize/close buttons live in one row
//! that this module builds.

use super::theme;
use gpui::prelude::*;
use gpui::{ElementId, MouseButton, Window, WindowControlArea, div, rgba};

/// A region of the integrated titlebar that drags the undecorated window.
///
/// Left-drag moves the window, right-click opens the window menu, and a
/// double-click toggles maximize, like a system title bar. The platform
/// ignores `window_control_area` on Linux, so the move starts here
/// explicitly; where the marker is honoured it does the same job.
pub(super) fn titlebar_drag(id: &'static str, stretch: bool, label: &str) -> impl IntoElement {
    let mut region = div()
        .id(ElementId::from(id))
        .px_2()
        .py_1()
        .text_sm()
        .text_color(rgba(theme::MUTED_TEXT))
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        })
        .on_mouse_down(MouseButton::Right, |ev, window, _| {
            window.show_window_menu(ev.position);
        })
        .on_click(|ev, window, _| {
            if ev.click_count() == 2 {
                window.zoom_window();
            }
        });
    if stretch {
        region = region.flex_1();
    }
    region.child(label.to_string())
}

/// The maximize button's glyph: a square to expand, nested squares to
/// restore, following the window's current state.
pub(super) fn maximize_glyph(is_maximized: bool) -> &'static str {
    if is_maximized { "▣" } else { "□" }
}

/// One of the integrated window controls: minimize, maximize or close.
///
/// Neutral grey, never the action green (which the smoke check counts), with
/// the close glyph in red. The `window_control_area` marker is honoured on
/// platforms with native hit-testing; the click handler below is what acts on
/// Linux, where the marker is ignored.
pub(super) fn window_button(
    id: &'static str,
    label: &str,
    area: WindowControlArea,
    danger: bool,
    on_click: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    div()
        .id(ElementId::from(id))
        .px_3()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .text_color(if danger {
            rgba(theme::DANGER)
        } else {
            rgba(theme::ROW_TEXT)
        })
        .child(label.to_string())
        .window_control_area(area)
        .on_click(move |_, window, _| on_click(window))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximize_button_names_the_action_it_will_take() {
        // A square to expand, nested squares to restore: the button always
        // states what pressing it does, following the window's state.
        assert_eq!(maximize_glyph(false), "□");
        assert_eq!(maximize_glyph(true), "▣");
    }
}
