/* cli/lib.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! Library half of the TimeTrack CLI.
//!
//! The TUI lives here and `main.rs` is a thin wrapper around it, so the
//! one-shot commands and the TUI share one crate. The split is not what
//! makes the TUI's tests run, though: `cargo test` builds a binary's own
//! `#[cfg(test)]` modules too, as the binary-only `timetrack-tray` crate
//! shows with its 27 unit tests.

pub mod tui;
