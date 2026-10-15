/* core/export.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! CSV export (REQUIREMENTS §10).
//!
//! One row per entry, with a header row. Times are ISO 8601 local timestamps
//! (`2026-09-29T14:00:00`), not epoch millis — a stakeholder opens this in
//! Excel, not in a hex editor. Duration appears as both `HH:MM:SS` and raw
//! minutes, and `source` is included so hand-entered time is visible.
//!
//! Pure like the rest of the core: takes the store, a scope, `now` and the
//! UTC offset explicitly, so it is testable without I/O or a clock.

use crate::aggregate::{iso_week_of, month_of};
use crate::format_duration;
use crate::model::{MS_PER_DAY, Store};
use crate::tz::Tz;

/// What an export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportScope {
    /// The ISO week containing `now`.
    Week,
    /// The calendar month containing `now`.
    Month,
    /// Everything.
    #[default]
    All,
}

impl ExportScope {
    /// Parse the CLI / D-Bus spelling.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "week" | "this-week" | "current-week" => Some(ExportScope::Week),
            "month" | "this-month" | "current-month" => Some(ExportScope::Month),
            "all" | "all-time" | "" => Some(ExportScope::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExportScope::Week => "week",
            ExportScope::Month => "month",
            ExportScope::All => "all",
        }
    }
}

/// Header row, in field order.
pub const CSV_HEADER: &str =
    "id,project,description,started_at,ended_at,duration_hms,duration_minutes,source,note";

/// Entries in `scope`, oldest first (the order a stakeholder reads).
pub fn entries_in_scope<'a>(
    store: &'a Store,
    scope: ExportScope,
    now_ms: i64,
    tz: &Tz,
) -> Vec<&'a crate::Entry> {
    let mut out: Vec<&crate::Entry> = match scope {
        ExportScope::All => store.entries.iter().collect(),
        ExportScope::Week => {
            let today = tz.day_of(now_ms);
            let week = iso_week_of(today);
            store
                .entries
                .iter()
                .filter(|e| {
                    let d = tz.day_of(e.started_at);
                    d >= week.start_day && d < week.start_day + 7
                })
                .collect()
        }
        ExportScope::Month => {
            let today = tz.day_of(now_ms);
            let (year, month) = month_of(today);
            store
                .entries
                .iter()
                .filter(|e| month_of(tz.day_of(e.started_at)) == (year, month))
                .collect()
        }
    };
    out.sort_by_key(|e| e.started_at);
    out
}

/// Stable identity of the ISO week containing `anchor_ms` (`2026-W41`).
///
/// Monday-start per ISO 8601, resolved through `tz` so both UIs agree: the
/// same anchor on either side of a zone boundary names different weeks. The
/// year is the ISO year, so 2021-01-01 (a Friday) is `2020-W53` while
/// 2026-01-01 (a Thursday) opens `2026-W01`.
pub fn week_id(anchor_ms: i64, tz: &Tz) -> String {
    let week = iso_week_of(tz.day_of(anchor_ms));
    format!("{:04}-W{:02}", week.year, week.week)
}

/// Stable identity of the calendar month containing `anchor_ms` (`2026-09`).
///
/// The local month, not the ISO year: 2025-12-29 sits in ISO week `2026-W01`
/// but still in month `2025-12`.
pub fn month_id(anchor_ms: i64, tz: &Tz) -> String {
    let (year, month) = month_of(tz.day_of(anchor_ms));
    format!("{year:04}-{month:02}")
}

/// Default file name for saving an export of `scope` anchored at `anchor_ms`.
///
/// Week and month names carry their period id so one week never silently
/// overwrites the last; all time ignores the anchor and takes the plain name.
pub fn default_export_filename(scope: ExportScope, anchor_ms: i64, tz: &Tz) -> String {
    match scope {
        ExportScope::Week => format!("timetrack-export-week-{}.csv", week_id(anchor_ms, tz)),
        ExportScope::Month => format!("timetrack-export-month-{}.csv", month_id(anchor_ms, tz)),
        ExportScope::All => "timetrack-export-all.csv".to_string(),
    }
}

/// Status line after a saved export, naming entry count, scope, period, and file.
///
/// All time has no period (its anchor is ignored), so `period_id` is unused
/// there and the status reads `(all time)`.
pub fn format_export_status(
    entry_count: usize,
    scope: ExportScope,
    period_id: &str,
    basename: &str,
) -> String {
    match scope {
        ExportScope::All => format!("exported {entry_count} entries (all time) to {basename}"),
        _ => format!(
            "exported {entry_count} entries ({} {period_id}) to {basename}",
            scope.as_str()
        ),
    }
}

/// Format a UTC instant as a local ISO 8601 timestamp without offset
/// (`2026-09-29T14:00:00`), per §10.
///
/// Uses the offset in force at that instant, so entries on either side of a
/// DST transition render with their own wall-clock time.
pub fn format_local_iso8601(utc_ms: i64, tz: &Tz) -> String {
    let local_ms = utc_ms + tz.offset_at_ms(utc_ms);
    let day = local_ms.div_euclid(MS_PER_DAY);
    let rem = local_ms.rem_euclid(MS_PER_DAY);
    let (y, m, d) = day_to_ymd(day);
    let (hh, mm, ss) = (
        rem / 3_600_000,
        (rem % 3_600_000) / 60_000,
        (rem % 60_000) / 1_000,
    );
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
}

/// Day index -> (year, month, day-of-month).
fn day_to_ymd(day: i64) -> (i64, u32, u32) {
    // Mirror aggregate.rs's walk so the two cannot disagree about month
    // lengths; this is the inverse of its day_of_month.
    let mut year = 1970;
    while days_to_year(year + 1) <= day && year < 10_000 {
        year += 1;
    }
    while days_to_year(year) > day && year > -10_000 {
        year -= 1;
    }
    let mut doy = day - days_to_year(year);
    let mut month = 1;
    loop {
        let len = days_in_month(year, month);
        if doy < len {
            break;
        }
        doy -= len;
        month += 1;
    }
    (year, month, (doy + 1) as u32)
}

fn days_to_year(year: i64) -> i64 {
    let mut days = 0;
    if year >= 1970 {
        for y in 1970..year {
            days += if is_leap(y) { 366 } else { 365 };
        }
    } else {
        for y in year..1970 {
            days -= if is_leap(y) { 366 } else { 365 };
        }
    }
    days
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// Escape one CSV field (RFC 4180 minimal): quote when it contains
/// `,`, `"`, or a line break, doubling inner quotes.
pub fn escape_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Render the store (or the scoped subset) as CSV, including the header.
pub fn export_csv(store: &Store, scope: ExportScope, now_ms: i64, tz: &Tz) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for e in entries_in_scope(store, scope, now_ms, tz) {
        let project = store
            .project(&e.project_id)
            .map(|p| p.name.as_str())
            .unwrap_or(e.project_id.as_str());
        let source = match e.source {
            crate::EntrySource::Manual => "manual",
            crate::EntrySource::QuickAdd => "quick_add",
        };
        let minutes = (e.duration_ms() / 60_000).to_string();
        let note = e.note.as_deref().unwrap_or("");
        let row = [
            escape_field(&e.id),
            escape_field(project),
            escape_field(&e.description),
            escape_field(&format_local_iso8601(e.started_at, tz)),
            escape_field(&format_local_iso8601(e.ended_at, tz)),
            escape_field(&format_duration(e.duration_ms())),
            minutes,
            source.to_string(),
            escape_field(note),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, EntrySource, Project};

    fn utc() -> Tz {
        Tz::utc()
    }

    fn store() -> Store {
        let mut s = Store::default();
        s.projects.push(Project {
            id: "p1".into(),
            name: "Work".into(),
            colour: None,
            archived: false,
        });
        s.entries.push(Entry {
            id: "e1".into(),
            project_id: "p1".into(),
            description: "review".into(),
            started_at: 0,
            ended_at: 3_600_000,
            source: EntrySource::Manual,
            note: None,
        });
        s.entries.push(Entry {
            id: "e2".into(),
            project_id: "p1".into(),
            description: "a,b \"quoted\"".into(),
            started_at: 3_600_000,
            ended_at: 3_600_000 + 15 * 60_000,
            source: EntrySource::QuickAdd,
            note: Some("n".into()),
        });
        s
    }

    #[test]
    fn header_lists_the_spec_columns() {
        assert_eq!(
            CSV_HEADER,
            "id,project,description,started_at,ended_at,duration_hms,duration_minutes,source,note"
        );
    }

    #[test]
    fn epoch_formats_as_midnight() {
        assert_eq!(format_local_iso8601(0, &utc()), "1970-01-01T00:00:00");
    }

    #[test]
    fn offset_shifts_the_wall_clock() {
        assert_eq!(
            format_local_iso8601(0, &Tz::fixed_ms(3_600_000)),
            "1970-01-01T01:00:00"
        );
        assert_eq!(
            format_local_iso8601(0, &Tz::fixed_ms(-3_600_000)),
            "1969-12-31T23:00:00"
        );
    }

    #[test]
    fn known_date_formats() {
        // 2026-09-29T14:00:00Z as epoch millis.
        let utc_ms = 1_790_690_400_000;
        assert_eq!(format_local_iso8601(utc_ms, &utc()), "2026-09-29T14:00:00");
        assert_eq!(
            format_local_iso8601(utc_ms, &Tz::fixed_ms(2 * 3_600_000)),
            "2026-09-29T16:00:00"
        );
    }

    #[test]
    fn named_zone_formats_with_its_own_offset_per_instant() {
        // Winter is +1, summer is +2 in Rome.
        let rome = Tz::parse("Europe/Rome").unwrap();
        assert_eq!(
            format_local_iso8601(1_768_478_400_000, &rome), // 2026-01-15 12:00 UTC
            "2026-01-15T13:00:00"
        );
        assert_eq!(
            format_local_iso8601(1_784_116_800_000, &rome), // 2026-07-15 12:00 UTC
            "2026-07-15T14:00:00"
        );
    }

    #[test]
    fn day_round_trips_for_a_range() {
        for day in [-800, -1, 0, 1, 20000] {
            let (y, m, d) = day_to_ymd(day);
            assert!((1..=12).contains(&m), "{day} -> {y}-{m}-{d}");
            assert!((1..=31).contains(&d), "{day} -> {y}-{m}-{d}");
        }
        assert_eq!(day_to_ymd(0), (1970, 1, 1));
        assert_eq!(day_to_ymd(-1), (1969, 12, 31));
    }

    #[test]
    fn commas_and_quotes_are_escaped() {
        assert_eq!(escape_field("a,b"), "\"a,b\"");
        assert_eq!(escape_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(escape_field("plain"), "plain");
    }

    #[test]
    fn export_has_header_plus_one_row_per_entry() {
        let s = store();
        let csv = export_csv(&s, ExportScope::All, 0, &utc());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], CSV_HEADER);
        assert!(lines[1].contains("review"));
        assert!(lines[1].contains("01:00:00"));
        assert!(lines[1].contains(",60,manual,"));
        assert!(lines[2].contains("quick_add"));
    }

    #[test]
    fn week_scope_excludes_other_weeks() {
        let s = store();
        // e1/e2 are in Jan 1970; "now" a fortnight later is a different week.
        let csv = export_csv(&s, ExportScope::Week, 14 * MS_PER_DAY, &utc());
        assert_eq!(csv.lines().count(), 1, "header only");
    }

    #[test]
    fn scope_parses_cli_spellings() {
        assert_eq!(ExportScope::parse("week"), Some(ExportScope::Week));
        assert_eq!(ExportScope::parse("month"), Some(ExportScope::Month));
        assert_eq!(ExportScope::parse("all"), Some(ExportScope::All));
        assert_eq!(ExportScope::parse("everything"), None);
    }

    #[test]
    fn week_id_formats_the_iso_week() {
        // 2026-10-08 is the Thursday of ISO week 41 (Mon 5 Oct - Sun 11 Oct).
        assert_eq!(week_id(1_791_460_800_000, &utc()), "2026-W41");
    }

    #[test]
    fn week_id_follows_the_iso_year_across_january_first() {
        // 2021-01-01 was a Friday, so it belongs to the last week of ISO
        // year 2020; 2026-01-01 was a Thursday, opening ISO year 2026.
        assert_eq!(week_id(1_609_502_400_000, &utc()), "2020-W53");
        assert_eq!(week_id(1_767_268_800_000, &utc()), "2026-W01");
        // ... and the Monday before it shares that week.
        assert_eq!(week_id(1_767_009_600_000, &utc()), "2026-W01");
    }

    #[test]
    fn week_id_uses_the_zone_offset() {
        // 2026-07-05 23:30 UTC is Sunday in UTC but Monday 00:30 at UTC+1,
        // so the two zones name different weeks for the same anchor.
        let anchor = 1_783_294_200_000;
        assert_eq!(week_id(anchor, &utc()), "2026-W27");
        assert_eq!(week_id(anchor, &Tz::fixed_ms(3_600_000)), "2026-W28");
    }

    #[test]
    fn month_id_formats_year_and_month() {
        assert_eq!(month_id(1_789_473_600_000, &utc()), "2026-09");
        assert_eq!(month_id(1_767_268_800_000, &utc()), "2026-01");
        // The local month is not the ISO year: 2025-12-29 sits in ISO week
        // 2026-W01 but still in calendar month 2025-12.
        assert_eq!(week_id(1_767_009_600_000, &utc()), "2026-W01");
        assert_eq!(month_id(1_767_009_600_000, &utc()), "2025-12");
    }

    #[test]
    fn default_filenames_carry_the_period() {
        assert_eq!(
            default_export_filename(ExportScope::Week, 1_791_460_800_000, &utc()),
            "timetrack-export-week-2026-W41.csv"
        );
        assert_eq!(
            default_export_filename(ExportScope::Month, 1_789_473_600_000, &utc()),
            "timetrack-export-month-2026-09.csv"
        );
        assert_eq!(
            default_export_filename(ExportScope::All, 1_791_460_800_000, &utc()),
            "timetrack-export-all.csv"
        );
    }

    #[test]
    fn export_status_names_count_scope_period_and_file() {
        assert_eq!(
            format_export_status(
                12,
                ExportScope::Week,
                "2026-W41",
                "timetrack-export-week-2026-W41.csv"
            ),
            "exported 12 entries (week 2026-W41) to timetrack-export-week-2026-W41.csv"
        );
        assert_eq!(
            format_export_status(
                3,
                ExportScope::Month,
                "2026-09",
                "timetrack-export-month-2026-09.csv"
            ),
            "exported 3 entries (month 2026-09) to timetrack-export-month-2026-09.csv"
        );
    }

    #[test]
    fn export_status_for_all_time_names_no_period() {
        // All time has no period (its anchor is ignored), so there is no
        // period id to name — the status says so plainly.
        assert_eq!(
            format_export_status(5, ExportScope::All, "all", "timetrack-export-all.csv"),
            "exported 5 entries (all time) to timetrack-export-all.csv"
        );
    }
}
