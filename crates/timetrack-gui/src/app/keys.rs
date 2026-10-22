/* gui/app/keys.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The keyboard's one gate: what a key press typed.
//!
//! Every prompt that types into a field asks this one question, so a
//! shortcut can never be typed as text.

use gpui::KeyDownEvent;

/// The printable character a key press produced, if any.
///
/// With control, alt or the platform modifier held the keystroke is a
/// shortcut, not input, and the `key_char` of a special key must not leak
/// into a field. Both cases answer `None`.
pub(super) fn printable_char(ev: &KeyDownEvent) -> Option<&str> {
    let m = &ev.keystroke.modifiers;
    if m.control || m.alt || m.platform {
        return None;
    }
    ev.keystroke.key_char.as_deref()
}
