/* gui/app/theme.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The GUI's colour palette.
//!
//! The render code used to repeat the same hex literals in a dozen places.
//! Naming them here keeps the palette in one file: a colour is changed in
//! one edit, and a colour that appears in two places is visibly the same
//! one.
//!
//! The colours are in `0xRRGGBBAA` form, which is what gpui's `rgba`
//! takes; the opaque ones end in `ff`.

use gpui::{Rgba, rgb, rgba};

/// The window's background.
pub(super) const WINDOW_BG: u32 = 0x11111bff;

/// The panel background: the titlebar and the prompt panels.
pub(super) const PANEL_BG: u32 = 0x1a1a26ff;

/// The raised background: the selected row, the selected tab, the focused
/// field, and the track the per-project bars fill.
pub(super) const RAISED_BG: u32 = 0x2a2a3aff;

/// The muted grey: the secondary labels, and a project that has no colour
/// of its own.
pub(super) const MUTED_TEXT: u32 = 0x7a7a90ff;

/// The accent: the durations and the highlighted numbers.
pub(super) const ACCENT: u32 = 0x7fd3ffff;

/// The action green: the quick-add, export and start-service buttons.
pub(super) const ACTION: u32 = 0x2ea043ff;

/// The unarchive button's green, translucent: the quiet half of the
/// archive pair.
pub(super) const ACTION_FADED: u32 = 0x2ea04366;

/// The dimmest grey: the hints under the fields.
pub(super) const HINT_TEXT: u32 = 0x6a6a80ff;

/// The danger red: the close button.
pub(super) const DANGER: u32 = 0xe06c75ff;

/// The grey of the secondary text in a row.
pub(super) const ROW_TEXT: u32 = 0x9a9ab0ff;

/// The bright grey of the emphasised text.
pub(super) const STRONG_TEXT: u32 = 0xc8c8d8ff;

/// The body text colour.
pub(super) const BODY_TEXT: u32 = 0xe6e6f0ff;

/// The warning amber: the over-24h notice and the inline errors.
pub(super) const WARNING: u32 = 0xd29922ff;

/// A project's own colour, or the muted grey when it has none.
///
/// The colour comes from the service in `0xRRGGBB` form, so it is
/// converted here rather than at the call site.
pub(super) fn project_colour(colour: Option<u32>) -> Rgba {
    match colour {
        Some(c) => rgb(c),
        None => rgba(MUTED_TEXT),
    }
}
