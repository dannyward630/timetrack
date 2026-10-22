/* gui/app/dialogs/entry.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The manual-entry dialog (REQUIREMENTS §5 methods 1 and 2, one form).
//!
//! Plain keystroke editing (type to append, backspace to delete) rather than
//! a toolkit text field: the window already routes every key through one
//! focused root, and three small fields do not need IME or cursor movement.
//! Everything here is pure state over strings, so the whole interaction is
//! unit-testable without a window.

/// Which field of the entry dialog receives keystrokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogField {
    Description,
    Start,
    End,
}

impl DialogField {
    pub(crate) fn next(self) -> Self {
        match self {
            DialogField::Description => DialogField::Start,
            DialogField::Start => DialogField::End,
            DialogField::End => DialogField::Description,
        }
    }

    pub(crate) fn prev(self) -> Self {
        match self {
            DialogField::Description => DialogField::End,
            DialogField::Start => DialogField::Description,
            DialogField::End => DialogField::Start,
        }
    }
}

/// The manual-entry dialog (REQUIREMENTS §5 methods 1 and 2, one form).
///
/// Method 2 is method 1 with "end = now" pre-filled, so a new dialog starts
/// with `start = "-60"`, `end = "now"` and the user edits from there. In edit
/// mode the times start empty, meaning "keep the current endpoint" — the
/// `keep_*` labels say what that is — which is the dialog form of `shorten`:
/// unspecified endpoints are left alone and the entry is always kept.
///
/// Plain keystroke editing (type to append, backspace to delete) rather than
/// a toolkit text field: the window already routes every key through one
/// focused root, and three small fields do not need IME or cursor movement.
/// Everything here is pure state over strings, so the whole interaction is
/// unit-testable without a window.
#[derive(Debug, Clone)]
pub(crate) struct EntryDialog {
    /// `None` for a new entry; `Some(id)` when editing that entry.
    pub(crate) edit_id: Option<String>,
    /// The project a new entry goes to, fixed when the dialog opens.
    pub(crate) project_id: String,
    pub(crate) project_name: String,
    pub(crate) description: String,
    pub(crate) start: String,
    pub(crate) end: String,
    /// Edit mode only: the current endpoints, for the "keep" fallback and
    /// the labels. New mode leaves these at zero and never reads them.
    pub(crate) orig_start: i64,
    pub(crate) orig_end: i64,
    pub(crate) orig_description: String,
    pub(crate) field: DialogField,
    pub(crate) error: Option<String>,
}

impl EntryDialog {
    /// A new-entry dialog for `project`, with "end = now" pre-filled.
    pub(crate) fn new(project_id: String, project_name: String) -> Self {
        EntryDialog {
            edit_id: None,
            project_id,
            project_name,
            description: String::new(),
            start: "-60".to_string(),
            end: "now".to_string(),
            orig_start: 0,
            orig_end: 0,
            orig_description: String::new(),
            field: DialogField::Description,
            error: None,
        }
    }

    /// An edit dialog for the entry with these current values. Times start
    /// empty, meaning "keep the current endpoint".
    pub(crate) fn edit(
        id: String,
        project_name: String,
        description: String,
        started_at: i64,
        ended_at: i64,
    ) -> Self {
        EntryDialog {
            edit_id: Some(id),
            project_id: String::new(),
            project_name,
            description: description.clone(),
            start: String::new(),
            end: String::new(),
            orig_start: started_at,
            orig_end: ended_at,
            orig_description: description,
            field: DialogField::Description,
            error: None,
        }
    }

    /// Label for the start row: the hint in edit mode says what empty keeps.
    pub(crate) fn start_hint(&self, tz: &timetrack_core::Tz) -> String {
        match &self.edit_id {
            None => "(-90, 90, now, HH:MM)".to_string(),
            Some(_) => format!(
                "(empty keeps {})",
                timetrack_core::format_local_hm(self.orig_start, tz)
            ),
        }
    }

    /// Label for the end row, as above.
    pub(crate) fn end_hint(&self, tz: &timetrack_core::Tz) -> String {
        match &self.edit_id {
            None => "(-90, 90, now, HH:MM)".to_string(),
            Some(_) => format!(
                "(empty keeps {})",
                timetrack_core::format_local_hm(self.orig_end, tz)
            ),
        }
    }

    /// Append typed text to the focused field. Any edit clears a stale error:
    /// the message described the text as it was, not as it is now.
    pub(crate) fn type_text(&mut self, ch: &str) {
        let target = match self.field {
            DialogField::Description => &mut self.description,
            DialogField::Start => &mut self.start,
            DialogField::End => &mut self.end,
        };
        target.push_str(ch);
        self.error = None;
    }

    /// Delete the last character of the focused field.
    pub(crate) fn backspace(&mut self) {
        let target = match self.field {
            DialogField::Description => &mut self.description,
            DialogField::Start => &mut self.start,
            DialogField::End => &mut self.end,
        };
        target.pop();
        self.error = None;
    }
}

/// What saving a dialog means, resolved purely from its strings.
///
/// Pure over the dialog plus an explicit clock and offset, so the whole
/// save path — parsing, the end-before-start refusal, the keep-fallback — is
/// testable without a view or a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    /// Create `[started_at, ended_at]` on `project_id` with `description`.
    Create {
        project_id: String,
        description: String,
        started_at: i64,
        ended_at: i64,
    },
    /// Move `id`'s endpoints, and rewrite the description when present.
    Update {
        id: String,
        started_at: i64,
        ended_at: i64,
        description: Option<String>,
    },
    /// Nothing changed; close without calling the service.
    NoChange,
    /// Keep the dialog open and show this instead.
    Invalid(String),
}

pub(crate) fn resolve_save(dlg: &EntryDialog, now: i64, tz: &timetrack_core::Tz) -> SaveOutcome {
    match &dlg.edit_id {
        None => {
            if dlg.start.trim().is_empty() {
                return SaveOutcome::Invalid("start cannot be empty".to_string());
            }
            if dlg.end.trim().is_empty() {
                return SaveOutcome::Invalid("end cannot be empty".to_string());
            }
            let started_at = match timetrack_core::parse_moment(&dlg.start, "start", now, tz) {
                Ok(t) => t,
                Err(e) => return SaveOutcome::Invalid(e.to_string()),
            };
            let ended_at = match timetrack_core::parse_moment(&dlg.end, "end", now, tz) {
                Ok(t) => t,
                Err(e) => return SaveOutcome::Invalid(e.to_string()),
            };
            if ended_at < started_at {
                return SaveOutcome::Invalid(
                    timetrack_core::RuleError::NegativeDuration.to_string(),
                );
            }
            SaveOutcome::Create {
                project_id: dlg.project_id.clone(),
                description: dlg.description.clone(),
                started_at,
                ended_at,
            }
        }
        Some(id) => {
            let started_at = if dlg.start.trim().is_empty() {
                dlg.orig_start
            } else {
                match timetrack_core::parse_moment(&dlg.start, "start", now, tz) {
                    Ok(t) => t,
                    Err(e) => return SaveOutcome::Invalid(e.to_string()),
                }
            };
            let ended_at = if dlg.end.trim().is_empty() {
                dlg.orig_end
            } else {
                match timetrack_core::parse_moment(&dlg.end, "end", now, tz) {
                    Ok(t) => t,
                    Err(e) => return SaveOutcome::Invalid(e.to_string()),
                }
            };
            if ended_at < started_at {
                return SaveOutcome::Invalid(
                    timetrack_core::RuleError::NegativeDuration.to_string(),
                );
            }
            let description =
                (dlg.description != dlg.orig_description).then(|| dlg.description.clone());
            if started_at == dlg.orig_start && ended_at == dlg.orig_end && description.is_none() {
                return SaveOutcome::NoChange;
            }
            SaveOutcome::Update {
                id: id.clone(),
                started_at,
                ended_at,
                description,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testsupport::new_dialog;

    #[test]
    fn a_new_dialog_prefills_end_at_now() {
        // Method 2 is method 1 with "end = now" pre-filled (§5): the common
        // case needs no typing at all.
        let dlg = new_dialog();
        assert_eq!(dlg.start, "-60");
        assert_eq!(dlg.end, "now");
        assert_eq!(dlg.field, DialogField::Description);
    }

    #[test]
    fn typing_appends_and_backspace_deletes() {
        let mut dlg = new_dialog();
        dlg.type_text("smoke");
        assert_eq!(dlg.description, "smoke");
        dlg.backspace();
        assert_eq!(dlg.description, "smok");
    }

    #[test]
    fn tab_cycles_through_the_fields() {
        let mut dlg = new_dialog();
        assert_eq!(dlg.field, DialogField::Description);
        dlg.field = dlg.field.next();
        assert_eq!(dlg.field, DialogField::Start);
        dlg.field = dlg.field.next();
        assert_eq!(dlg.field, DialogField::End);
        dlg.field = dlg.field.next();
        assert_eq!(dlg.field, DialogField::Description);
        dlg.field = dlg.field.prev();
        assert_eq!(dlg.field, DialogField::End);
    }

    #[test]
    fn saving_a_new_dialog_creates_the_interval() {
        let mut dlg = new_dialog();
        dlg.description = "review".into();
        // now = 12:00 UTC; -60 → 11:00, now → 12:00.
        let now = 12 * 3_600_000;
        let utc = timetrack_core::Tz::utc();
        match resolve_save(&dlg, now, &utc) {
            SaveOutcome::Create {
                project_id,
                description,
                started_at,
                ended_at,
            } => {
                assert_eq!(project_id, "p1");
                assert_eq!(description, "review");
                assert_eq!(started_at, 11 * 3_600_000);
                assert_eq!(ended_at, now);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn a_backwards_interval_stays_open_with_an_error() {
        let mut dlg = new_dialog();
        dlg.start = "now".into();
        dlg.end = "-60".into();
        let utc = timetrack_core::Tz::utc();
        match resolve_save(&dlg, 12 * 3_600_000, &utc) {
            SaveOutcome::Invalid(msg) => assert!(msg.contains("must not end before")),
            other => panic!("expected Invalid, got {other:?}"),
        }
        // And the dialog keeps the error for the panel to show.
        dlg.error = Some("x".into());
        dlg.field = dlg.field.next();
        assert_eq!(dlg.error, Some("x".into()));
    }

    #[test]
    fn typing_clears_a_stale_error() {
        let mut dlg = new_dialog();
        dlg.error = Some("bad".into());
        dlg.type_text("x");
        assert_eq!(dlg.error, None);
    }

    #[test]
    fn an_untouched_edit_dialog_is_a_no_op() {
        let dlg = EntryDialog::edit("e1".into(), "Work".into(), "kept".into(), 0, 3_600_000);
        let utc = timetrack_core::Tz::utc();
        assert_eq!(
            resolve_save(&dlg, 12 * 3_600_000, &utc),
            SaveOutcome::NoChange
        );
    }

    #[test]
    fn empty_times_keep_the_current_endpoints() {
        // The dialog form of `shorten`: only the typed endpoint moves, and
        // the entry is never at risk.
        let mut dlg = EntryDialog::edit("e1".into(), "Work".into(), "kept".into(), 0, 3_600_000);
        dlg.end = "-60".into();
        let now = 12 * 3_600_000;
        let utc = timetrack_core::Tz::utc();
        match resolve_save(&dlg, now, &utc) {
            SaveOutcome::Update {
                id,
                started_at,
                ended_at,
                description,
            } => {
                assert_eq!(id, "e1");
                assert_eq!(started_at, 0, "untouched start is kept");
                assert_eq!(ended_at, now - 3_600_000);
                assert_eq!(description, None, "untouched text sends no SetText");
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn rewritten_text_sends_a_description_update() {
        let mut dlg = EntryDialog::edit("e1".into(), "Work".into(), "typo".into(), 0, 3_600_000);
        dlg.description = "review".into();
        let utc = timetrack_core::Tz::utc();
        match resolve_save(&dlg, 12 * 3_600_000, &utc) {
            SaveOutcome::Update { description, .. } => {
                assert_eq!(description.as_deref(), Some("review"))
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn the_keep_hints_name_the_current_times() {
        // 01:00 and 02:00 UTC.
        let dlg = EntryDialog::edit(
            "e1".into(),
            "Work".into(),
            String::new(),
            3_600_000,
            7_200_000,
        );
        let utc = timetrack_core::Tz::utc();
        assert_eq!(dlg.start_hint(&utc), "(empty keeps 01:00)");
        assert_eq!(dlg.end_hint(&utc), "(empty keeps 02:00)");
    }

    #[test]
    fn the_keep_hints_use_the_named_zone() {
        // 22:30 UTC is 00:30 the next day in Rome (summer, +2).
        let rome = timetrack_core::Tz::parse("Europe/Rome").unwrap();
        let dlg = EntryDialog::edit(
            "e1".into(),
            "Work".into(),
            String::new(),
            1_783_290_600_000,
            1_783_290_600_000 + 3_600_000,
        );
        assert_eq!(dlg.start_hint(&rome), "(empty keeps 00:30)");
    }
}
