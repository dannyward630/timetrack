/* gui/app.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The main window. The bridge to the service -- the messages, the commands
//! and the thread that owns the bus connection -- is in `service.rs`.
//!
//! GPUI is single-threaded and owns the UI, but zbus is async, so the two meet
//! across a thread boundary: a background thread owns an async-io runtime and
//! the bus connection and forwards snapshots over a channel, while the UI
//! drains that channel during `render`. The view holds no authoritative state
//! -- every pixel it draws comes from a `Snapshot` the service produced, which
//! is what keeps the GUI and the CLI consistent when both are open.
//!
//! # Home is the week total, and nothing is above it
//!
//! The largest element on the Home tab is the current ISO week's total, and it
//! is the first thing in the column. No clock, no greeting, no header
//! (REQUIREMENTS §7). The number is the answer the app exists to give, so it
//! gets the top of the screen and everything else is quieter.
//!
//! # Nothing is running
//!
//! There is no timer state here, because there is none in the model. The Home
//! tab's primary control is the 5/15/30/60 quick-add row.

use self::chrome::{maximize_glyph, titlebar_drag, window_button};
use self::dialogs::delete::{DeleteConfirm, delete_offer};
use self::dialogs::entry::{DialogField, EntryDialog, SaveOutcome, resolve_save};
use self::dialogs::merge::{MergeConfirm, merge_offer};
use self::dialogs::project::{ProjectPrompt, ProjectSave, resolve_project_prompt};
use self::dialogs::split::{SplitPrompt, resolve_split};
use self::export::picker::ExportPicker;
use self::export::save::{export_location_text, export_preview_entries};
use self::keys::printable_char;
use self::service::{Action, Msg, export_request, now_ms, spawn_service_thread};
use self::status::{StatusKind, StatusMsg, is_connection_notice};
use self::tabs::{Tab, cycle_tab};
use gpui::prelude::*;
use gpui::{
    AsyncApp, Context, ElementId, Entity, FocusHandle, InteractiveElement, IntoElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px, rgb, rgba,
};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;
use timetrack_core::QUICK_ADD_MS;
use timetrack_proto::{ProjectView, Snapshot};

/// How often the service is polled for a new snapshot.
const REFRESH: Duration = Duration::from_millis(1000);

/// The snapshot's zone: prefer the name, fall back to the fixed offset for
/// old services that send no name.
fn snapshot_tz(snap: &timetrack_proto::Snapshot) -> timetrack_core::Tz {
    timetrack_core::Tz::from_snapshot(&snap.tz, snap.local_offset_ms)
}

/// The newest quick-add, which is what undo removes.
///
/// A free function over the snapshot rather than a method on the view, so the
/// rule is testable without constructing a GPUI view (which needs a live
/// service connection). Entries arrive newest first, so the first match is the
/// newest.
fn newest_quick_add(snap: &Snapshot) -> Option<String> {
    snap.entries
        .iter()
        .find(|e| e.source == timetrack_proto::EntrySource::QuickAdd)
        .map(|e| e.id.clone())
}

/// Which prompt, if any, currently owns the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenPrompt {
    Entry,
    Project,
    Split,
    Merge,
    Delete,
}

/// Which prompt is open, if one is.
///
/// The key handler and the renderer ask this same question, so a prompt that
/// captures the keyboard is always one that is on screen.
///
/// They used to disagree. The key handler dispatched to the prompts on every
/// tab, while the renderer drew them inside `render_home` -- so opening one on
/// the Projects or Export tab produced a modal that was never drawn and that
/// swallowed every keystroke: the window looked frozen and only `Esc` got
/// out. One question, asked once, is what keeps the two in step.
///
/// At most one prompt is open at a time (each one swallows the keys that
/// would open another), so the order below is the priority the key handler
/// already used: the entry dialog first, then the project prompt, the split
/// prompt, the merge confirmation and the delete confirmation.
fn which_prompt(
    entry: bool,
    project: bool,
    split: bool,
    merge: bool,
    delete: bool,
) -> Option<OpenPrompt> {
    if entry {
        Some(OpenPrompt::Entry)
    } else if project {
        Some(OpenPrompt::Project)
    } else if split {
        Some(OpenPrompt::Split)
    } else if merge {
        Some(OpenPrompt::Merge)
    } else if delete {
        Some(OpenPrompt::Delete)
    } else {
        None
    }
}

/// The element id of one of a project's row buttons.
///
/// Built from the project id itself rather than from a number parsed out of
/// it. The core mints ids as `p1`, `p2`, ..., so parsing the numeric suffix
/// was unique in practice -- but any id that was not `p<digits>` fell back to
/// `0`, which would have given two such projects the same element id, and
/// gpui would have treated the second button as the first.
fn project_button_id(kind: &'static str, project_id: &str) -> ElementId {
    ElementId::from((ElementId::from(kind), project_id.to_string()))
}

/// Width of the per-project bar track on Home.
const PROJECT_BAR_TRACK_PX: f32 = 120.0;

/// Bar fill width for one per-project row: longest fills the track.
///
/// Pure arithmetic so the proportion is unit-testable; the renderer only
/// turns the number into pixels.
fn bar_width(ms: i64, max_ms: i64) -> f32 {
    if max_ms <= 0 {
        return 0.0;
    }
    (ms.max(0) as f32 / max_ms as f32) * PROJECT_BAR_TRACK_PX
}

pub struct TimetrackView {
    /// Snapshots, on their own bounded channel: a stale one is worth
    /// nothing, so the queue never grows (see `spawn_service_thread`).
    snapshots: Receiver<Snapshot>,
    /// Errors, statuses and action results: never dropped.
    rx: Receiver<Msg>,
    actions: Sender<Action>,
    snapshot: Snapshot,
    status: Option<StatusMsg>,
    selected: usize,
    tab: Tab,
    /// Index into the active projects, for quick-add attribution.
    project_cursor: usize,
    /// Entity handle, used to attach click handlers from inside `render`
    /// (which only has `&self`).
    entity: Option<Entity<Self>>,
    /// Focus for the root element.
    ///
    /// This is not optional polish. gpui dispatches a key event along the
    /// ancestor path of the *focused* node only, so without a focused element
    /// the root's `on_key_down` never runs and the whole keyboard is dead --
    /// the window looks alive, tabs respond to clicks, and nothing else
    /// responds to anything. The handle is focused on the first frame.
    focus: FocusHandle,
    /// Whether the one-time focus-on-render has happened. Re-focusing every
    /// frame would steal focus back from anything focusable the user reaches
    /// later, which would break Tab navigation.
    /// The manual-entry dialog, when open. Modal: while this is `Some` every
    /// keystroke goes to the dialog and the app shortcuts are suspended.
    dialog: Option<EntryDialog>,
    /// The project prompt, when open. Modal like the dialog: `None` creates,
    /// `Some` with an id renames.
    project_prompt: Option<ProjectPrompt>,
    /// The split prompt, when open. Modal like the dialog.
    split_prompt: Option<SplitPrompt>,
    /// A pending merge confirmation. Only `m` (confirm), escape (cancel)
    /// and `q` (quit) do anything while this is `Some`.
    merge_confirm: Option<MergeConfirm>,
    /// A pending delete confirmation. Only `d` (confirm), escape (cancel)
    /// and `q` (quit) do anything while this is `Some`.
    delete_confirm: Option<DeleteConfirm>,
    /// The Export tab's scope-plus-anchor picker (spec #19). The anchor
    /// defaults to now at view creation; all time ignores it.
    export_picker: ExportPicker,
    /// Where the last export save went, and whether the portal granted it.
    /// `None` before the first save. Drives the Export tab's location line:
    /// a portal grant names its chosen path, anything else (direct save or
    /// no save yet) keeps the home-directory wording.
    export_last_dest: Option<String>,
    export_last_via_portal: bool,
}

impl TimetrackView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (snapshots, rx, actions) = spawn_service_thread();
        let view = TimetrackView {
            snapshots,
            rx,
            actions,
            snapshot: Snapshot::default(),
            status: None,
            selected: 0,
            tab: Tab::Home,
            project_cursor: 0,
            entity: None,
            focus: cx.focus_handle(),
            dialog: None,
            project_prompt: None,
            split_prompt: None,
            merge_confirm: None,
            delete_confirm: None,
            export_picker: ExportPicker::new(now_ms()),
            export_last_dest: None,
            export_last_via_portal: false,
        };
        view.schedule_poll(cx);
        view
    }

    /// Re-render a few times a second so a queued service snapshot shows up
    /// promptly.
    ///
    /// `cx.spawn` hands the future a `WeakEntity`, so the entity is not kept
    /// alive by its own timer task; if the window closes the upgrade fails and
    /// the loop ends.
    fn schedule_poll(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                // `update` alone does not re-render: gpui only paints after
                // `notify`, so a drained snapshot with no notify would sit in
                // state until the next pointer motion happened to repaint.
                if this
                    .update(cx, |view, cx| {
                        if view.drain() {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    // NOTE: the service connection deliberately does NOT happen inside a
    // `cx.spawn` future. gpui drives those on its own executor threads, and
    // zbus' blocking/async-io backend is fine there but its tokio backend is
    // not. Everything bus-related is confined to the dedicated std::thread in
    // `spawn_service_thread`, which owns an async-io runtime of its own.

    /// Take a fresh snapshot. Returns whether anything visible changed.
    fn apply_snapshot(&mut self, s: Snapshot) -> bool {
        let mut changed = false;
        if s != self.snapshot {
            self.snapshot = s;
            changed = true;
        }
        // The typed day mirrors the anchor's local day: re-mirror
        // it from the snapshot zone now that a real zone exists.
        // User edits stay intact — only the just-opened value is
        // fixed (see `refresh_from_snapshot`).
        let tz = snapshot_tz(&self.snapshot);
        let before = self.export_picker.anchor_text.clone();
        self.export_picker.refresh_from_snapshot(&tz);
        if self.export_picker.anchor_text != before {
            changed = true;
        }
        // A routine snapshot must not wipe feedback from the last
        // action: snapshots arrive every second, so clearing here
        // made error reports from the service thread unreadable.
        //
        // Two notices are the exception, because a snapshot is
        // what proves them over: the service is there again
        // ("service is not running"), and it is speaking the
        // interface this client speaks ("different versions" --
        // the user reinstalled whichever side was older). An
        // ordinary action error survives until the next one.
        if self
            .status
            .as_ref()
            .is_some_and(|s| is_connection_notice(s.kind))
        {
            self.status = None;
            changed = true;
        }
        // The lists can shrink under the cursor when something is
        // removed, which would otherwise index out of bounds.
        let entries = self.snapshot.entries.len();
        if self.selected >= entries {
            let clamped = entries.saturating_sub(1);
            if clamped != self.selected {
                self.selected = clamped;
                changed = true;
            }
        }
        let projects = self.active_projects().len();
        if self.project_cursor >= projects {
            let clamped = projects.saturating_sub(1);
            if clamped != self.project_cursor {
                self.project_cursor = clamped;
                changed = true;
            }
        }
        changed
    }

    /// Fold every queued message into one refresh. Several may be pending and
    /// only the newest snapshot matters.
    ///
    /// Returns whether anything visible changed, so the background poll can
    /// skip the re-render when the service had nothing new to say instead of
    /// repainting several times a second forever.
    fn drain(&mut self) -> bool {
        let mut changed = false;
        // Set the status only when it differs: the service thread resends
        // nothing, but snapshots arrive every second and must not count as
        // change on their own.
        let set_status = |status: &mut Option<StatusMsg>, note: StatusMsg, changed: &mut bool| {
            if status.as_ref() != Some(&note) {
                *status = Some(note);
                *changed = true;
            }
        };
        // The snapshot channel is bounded, so it holds at most one. Take the
        // newest one waiting and leave the rest: a snapshot a second old is
        // worth nothing, and the next tick's arrives.
        let mut newest: Option<Snapshot> = None;
        while let Ok(s) = self.snapshots.try_recv() {
            newest = Some(s);
        }
        if let Some(s) = newest {
            changed |= self.apply_snapshot(s);
        }

        loop {
            match self.rx.try_recv() {
                Ok(Msg::Error(note)) => set_status(&mut self.status, note, &mut changed),
                Ok(Msg::Status(note)) => {
                    set_status(&mut self.status, StatusMsg::local(note), &mut changed)
                }
                Ok(Msg::ExportSaved {
                    status,
                    dest,
                    via_portal,
                }) => {
                    set_status(&mut self.status, StatusMsg::local(status), &mut changed);
                    if self.export_last_dest.as_deref() != Some(dest.as_str())
                        || self.export_last_via_portal != via_portal
                    {
                        self.export_last_dest = Some(dest);
                        self.export_last_via_portal = via_portal;
                        changed = true;
                    }
                }
                Ok(Msg::ActionOk) => {
                    // A success dismisses the previous error -- but only one
                    // the actions own. The "service is not running" notice
                    // belongs to the connection, not to any action.
                    if !self.service_missing() && self.status.is_some() {
                        self.status = None;
                        changed = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    set_status(
                        &mut self.status,
                        StatusMsg::local("the service thread stopped"),
                        &mut changed,
                    );
                    break;
                }
            }
        }
        changed
    }

    fn send(&self, a: Action) {
        let _ = self.actions.send(a);
    }

    /// Projects offered for new entries: archived ones are hidden, because
    /// archiving is about not choosing them, not about erasing their history
    /// (REQUIREMENTS §14).
    fn active_projects(&self) -> Vec<&ProjectView> {
        self.snapshot
            .projects
            .iter()
            .filter(|p| !p.archived)
            .collect()
    }

    fn current_project(&self) -> Option<&ProjectView> {
        self.active_projects().get(self.project_cursor).copied()
    }

    /// Which prompt is open, if one is.
    ///
    /// The key handler and the renderer both ask this, so a prompt that
    /// captures the keyboard is always one that is drawn -- on whichever tab
    /// is showing.
    fn open_prompt(&self) -> Option<OpenPrompt> {
        which_prompt(
            self.dialog.is_some(),
            self.project_prompt.is_some(),
            self.split_prompt.is_some(),
            self.merge_confirm.is_some(),
            self.delete_confirm.is_some(),
        )
    }

    /// Handle a key press. Returns false when the app should quit.
    fn on_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        // A prompt that is open owns the keyboard, including the letters the
        // app shortcuts are bound to. Which prompt is open is one question
        // (`open_prompt`), and the renderer asks the same one -- so a prompt
        // that takes the keyboard is always one that is on screen.
        match self.open_prompt() {
            Some(OpenPrompt::Entry) => return self.on_dialog_key(ev),
            Some(OpenPrompt::Project) => return self.on_project_key(ev),
            Some(OpenPrompt::Split) => return self.on_split_key(ev),
            // A merge or a delete waits its turn: the anchor field is typed
            // into first, and each is answered by its own key handler below.
            _ => {}
        }
        // The anchor day field owns the keyboard while it is being typed
        // in, like the modal prompts do.
        if self.export_picker.editing {
            return self.on_anchor_key(ev);
        }
        // A pending merge or delete only listens for its own answer.
        match self.open_prompt() {
            Some(OpenPrompt::Merge) => return self.on_merge_key(ev),
            Some(OpenPrompt::Delete) => return self.on_delete_key(ev),
            _ => {}
        }
        // Tab cycles the three tabs (§11); shift goes backward. The Tab key
        // carries no printable char, so this keys on the key name, like the
        // dialog's field cycling does.
        if ev.keystroke.key.as_str() == "tab" {
            self.tab = cycle_tab(self.tab, ev.keystroke.modifiers.shift);
            return true;
        }
        let Some(ch) = ev.keystroke.key_char.as_deref() else {
            return true;
        };
        match ch {
            "q" => return false,

            // 1-4 map to the quick-add buckets (REQUIREMENTS §6).
            "1" => self.quick_add(QUICK_ADD_MS[0]),
            "2" => self.quick_add(QUICK_ADD_MS[1]),
            "3" => self.quick_add(QUICK_ADD_MS[2]),
            "4" => self.quick_add(QUICK_ADD_MS[3]),

            // The manual-entry dialog (REQUIREMENTS §5): `a` for a new
            // interval, `e` to adjust the selected entry.
            "a" => self.open_new_dialog(),
            "e" => self.open_edit_dialog(),

            // Projects (§11): `n` names a new one, `r` renames the current
            // one. No numbered placeholders anywhere: every name is typed.
            "n" => self.open_project_prompt(),
            "r" => self.open_rename_prompt(),

            // Undo removes only the quick-added entry; delete is explicit.
            "u" => match newest_quick_add(&self.snapshot) {
                Some(id) => self.send(Action::UndoQuickAdd(id)),
                None => self.status = Some(StatusMsg::local("nothing to undo")),
            },
            "d" => self.open_delete_confirm(),

            // Split and merge (REQUIREMENTS §8).
            "s" => self.open_split_prompt(),
            "m" => self.open_merge_confirm(),

            // Export anchor picker (spec #19): one press moves one period,
            // and `t` opens the typed day. Only on the Export tab; elsewhere
            // these keys stay unbound.
            "[" => {
                if self.tab == Tab::Export {
                    self.step_export_anchor(-1);
                }
            }
            "]" => {
                if self.tab == Tab::Export {
                    self.step_export_anchor(1);
                }
            }
            "t" => {
                if self.tab == Tab::Export {
                    self.export_picker.start_editing();
                }
            }

            "h" => self.project_cursor = self.project_cursor.saturating_sub(1),
            "l" => {
                if self.project_cursor + 1 < self.active_projects().len() {
                    self.project_cursor += 1;
                }
            }
            "j" => {
                if self.selected + 1 < self.snapshot.entries.len() {
                    self.selected += 1;
                }
            }
            "k" => self.selected = self.selected.saturating_sub(1),

            _ => {}
        }
        true
    }

    /// Handle a key press while the entry dialog is open. Always returns true:
    /// `q` types a "q" here rather than quitting, and `escape` cancels.
    fn on_dialog_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        match ev.keystroke.key.as_str() {
            "escape" => {
                self.dialog = None;
            }
            "enter" => self.dialog_save(),
            "tab" => {
                if let Some(dlg) = self.dialog.as_mut() {
                    if ev.keystroke.modifiers.shift {
                        dlg.field = dlg.field.prev();
                    } else {
                        dlg.field = dlg.field.next();
                    }
                }
            }
            "backspace" => {
                if let Some(dlg) = self.dialog.as_mut() {
                    dlg.backspace();
                }
            }
            _ => {
                // Printable text, and only that: with control, alt or the
                // platform modifier held the keystroke is a shortcut, not
                // input, and key_char of a special key must not leak in.
                if let Some(ch) = printable_char(ev) {
                    if let Some(dlg) = self.dialog.as_mut() {
                        dlg.type_text(ch);
                    }
                }
            }
        }
        true
    }

    /// Open a new-entry dialog for the current project.
    fn open_new_dialog(&mut self) {
        let current = self
            .current_project()
            .map(|p| (p.id.clone(), p.name.clone()));
        match current {
            Some((id, name)) => {
                self.status = None;
                self.dialog = Some(EntryDialog::new(id, name));
            }
            None => {
                self.status = Some(StatusMsg::local(
                    "create a project on the Projects tab before adding time",
                ));
            }
        }
    }

    /// Open an edit dialog for the selected entry: new endpoints, or new
    /// text, or both. Empty times keep the current endpoints.
    fn open_edit_dialog(&mut self) {
        match self.snapshot.entries.get(self.selected).cloned() {
            Some(e) => {
                let project = self
                    .snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == e.project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| e.project_id.clone());
                self.status = None;
                self.dialog = Some(EntryDialog::edit(
                    e.id,
                    project,
                    e.description,
                    e.started_at,
                    e.ended_at,
                ));
            }
            None => self.status = Some(StatusMsg::local("nothing to edit")),
        }
    }

    /// Open the project prompt for a new, typed name.
    fn open_project_prompt(&mut self) {
        self.status = None;
        self.project_prompt = Some(ProjectPrompt {
            edit_id: None,
            name: String::new(),
            error: None,
        });
    }

    /// Open the project prompt to rename the current project, prefilled so
    /// small fixes keep most of the name.
    fn open_rename_prompt(&mut self) {
        let current = self
            .current_project()
            .map(|p| (p.id.clone(), p.name.clone()));
        match current {
            Some((id, name)) => {
                self.status = None;
                self.project_prompt = Some(ProjectPrompt {
                    edit_id: Some(id),
                    name,
                    error: None,
                });
            }
            None => self.status = Some(StatusMsg::local("nothing to rename")),
        }
    }

    /// Handle a key press while the project prompt is open. Always returns
    /// true: `q` types a "q" here rather than quitting, and `escape` cancels.
    fn on_project_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        match ev.keystroke.key.as_str() {
            "escape" => {
                self.project_prompt = None;
            }
            "enter" => self.project_save(),
            "backspace" => {
                if let Some(prompt) = self.project_prompt.as_mut() {
                    prompt.name.pop();
                    prompt.error = None;
                }
            }
            _ => {
                // Printable text, and only that: same gate as the other
                // prompts, so shortcuts never leak into the name.
                if let Some(ch) = printable_char(ev) {
                    if let Some(prompt) = self.project_prompt.as_mut() {
                        prompt.name.push_str(ch);
                        prompt.error = None;
                    }
                }
            }
        }
        true
    }

    /// Validate the project prompt and send it. Stays open with an error on
    /// empty input; closes on success. Duplicate names travel to the service,
    /// which owns uniqueness and reports them typed.
    fn project_save(&mut self) {
        let Some(prompt) = self.project_prompt.clone() else {
            return;
        };
        match resolve_project_prompt(&prompt) {
            Err(msg) => {
                if let Some(open) = self.project_prompt.as_mut() {
                    open.error = Some(msg);
                }
            }
            Ok(ProjectSave::Create { name }) => {
                self.send(Action::AddProject(name));
                self.project_prompt = None;
            }
            Ok(ProjectSave::Update { id, name }) => {
                self.send(Action::UpdateProject { id, name });
                self.project_prompt = None;
            }
        }
    }

    /// Open the split prompt for the selected entry, if there is one.
    fn open_split_prompt(&mut self) {
        match self.snapshot.entries.get(self.selected).cloned() {
            Some(e) => {
                self.status = None;
                self.split_prompt = Some(SplitPrompt {
                    entry_id: e.id,
                    started_at: e.started_at,
                    ended_at: e.ended_at,
                    at: String::new(),
                    error: None,
                });
            }
            None => self.status = Some(StatusMsg::local("nothing to split")),
        }
    }

    /// Handle a key press while the split prompt is open. Always returns
    /// true: `q` types a "q" here rather than quitting, and `escape` cancels.
    fn on_split_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        match ev.keystroke.key.as_str() {
            "escape" => {
                self.split_prompt = None;
            }
            "enter" => self.split_save(),
            "backspace" => {
                if let Some(prompt) = self.split_prompt.as_mut() {
                    prompt.at.pop();
                    prompt.error = None;
                }
            }
            _ => {
                // Printable text, and only that: same gate as the dialog, so
                // shortcuts and special keys never leak into the moment.
                if let Some(ch) = printable_char(ev) {
                    if let Some(prompt) = self.split_prompt.as_mut() {
                        prompt.at.push_str(ch);
                        prompt.error = None;
                    }
                }
            }
        }
        true
    }

    /// Validate the split prompt and send it. Stays open with an error on
    /// invalid input; closes on success.
    fn split_save(&mut self) {
        let Some(prompt) = self.split_prompt.clone() else {
            return;
        };
        let tz = snapshot_tz(&self.snapshot);
        match resolve_split(&prompt, now_ms(), &tz) {
            Err(msg) => {
                if let Some(open) = self.split_prompt.as_mut() {
                    open.error = Some(msg);
                }
            }
            Ok(at_ms) => {
                self.send(Action::SplitEntry {
                    id: prompt.entry_id,
                    at_ms,
                });
                self.split_prompt = None;
            }
        }
    }

    /// Offer the merge confirmation for the selection, if the selected entry
    /// overlaps or touches anything on its project.
    fn open_merge_confirm(&mut self) {
        match merge_offer(&self.snapshot, self.selected) {
            Some(confirm) => {
                self.status = None;
                self.merge_confirm = Some(confirm);
            }
            None => {
                self.status = Some(StatusMsg::local("nothing to merge with it"));
            }
        }
    }

    /// Handle a key press while a merge confirmation is pending. Only the
    /// confirmation's own answers do anything; anything else is swallowed so
    /// no other action can sneak in mid-confirmation.
    fn on_merge_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        if ev.keystroke.key.as_str() == "escape" {
            self.merge_confirm = None;
            return true;
        }
        match ev.keystroke.key_char.as_deref() {
            Some("m") => {
                if let Some(confirm) = self.merge_confirm.take() {
                    self.send(Action::MergeEntries { ids: confirm.ids });
                }
                true
            }
            Some("q") => false,
            _ => true,
        }
    }

    /// Offer the delete confirmation for the selection, if there is one.
    /// The first `d` only opens the banner; a second `d` executes it.
    fn open_delete_confirm(&mut self) {
        match delete_offer(&self.snapshot, self.selected) {
            Some(confirm) => {
                self.status = None;
                self.delete_confirm = Some(confirm);
            }
            None => {
                self.status = Some(StatusMsg::local("nothing to delete"));
            }
        }
    }

    /// Handle a key press while a delete confirmation is pending. Only the
    /// confirmation's own answers do anything; anything else is swallowed so
    /// no other action can sneak in mid-confirmation.
    fn on_delete_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        if ev.keystroke.key.as_str() == "escape" {
            self.delete_confirm = None;
            return true;
        }
        match ev.keystroke.key_char.as_deref() {
            Some("d") => {
                if let Some(confirm) = self.delete_confirm.take() {
                    self.send(Action::DeleteEntry(confirm.id));
                }
                true
            }
            Some("q") => false,
            _ => true,
        }
    }

    /// Validate the dialog and send the action(s). Stays open with an error
    /// on invalid input; closes on success or when nothing changed.
    fn dialog_save(&mut self) {
        let Some(dlg) = self.dialog.as_ref() else {
            return;
        };
        let (orig_start, orig_end) = (dlg.orig_start, dlg.orig_end);
        let tz = snapshot_tz(&self.snapshot);
        match resolve_save(dlg, now_ms(), &tz) {
            SaveOutcome::Invalid(msg) => {
                if let Some(dlg) = self.dialog.as_mut() {
                    dlg.error = Some(msg);
                }
            }
            SaveOutcome::NoChange => {
                self.dialog = None;
            }
            SaveOutcome::Create {
                project_id,
                description,
                started_at,
                ended_at,
            } => {
                self.send(Action::AddEntry {
                    project: project_id,
                    description,
                    started_at,
                    ended_at,
                });
                self.dialog = None;
            }
            SaveOutcome::Update {
                id,
                started_at,
                ended_at,
                description,
            } => {
                // Only call for what actually changed: an identical SetTimes
                // would persist and acknowledge a no-op write.
                if started_at != orig_start || ended_at != orig_end {
                    self.send(Action::SetTimes {
                        id: id.clone(),
                        started_at,
                        ended_at,
                    });
                }
                if let Some(description) = description {
                    self.send(Action::SetText { id, description });
                }
                self.dialog = None;
            }
        }
    }

    /// Move the export anchor one period in `delta` (±1 per press).
    fn step_export_anchor(&mut self, delta: i32) {
        let tz = snapshot_tz(&self.snapshot);
        self.export_picker.step(delta, &tz);
    }

    /// Handle a key press while the anchor day field is open. Always returns
    /// true: `q` types a "q" here rather than quitting, and `escape` cancels.
    fn on_anchor_key(&mut self, ev: &gpui::KeyDownEvent) -> bool {
        match ev.keystroke.key.as_str() {
            "escape" => {
                let tz = snapshot_tz(&self.snapshot);
                self.export_picker.cancel_editing(&tz);
            }
            "enter" => {
                let tz = snapshot_tz(&self.snapshot);
                self.export_picker.commit(&tz);
            }
            "backspace" => self.export_picker.backspace(),
            // Tab stays in the field: enter sets the day, escape cancels.
            "tab" => {}
            _ => {
                // Printable text, and only that: same gate as the dialogs, so
                // shortcuts and special keys never leak into the day.
                if let Some(ch) = printable_char(ev) {
                    self.export_picker.type_text(ch);
                }
            }
        }
        true
    }

    /// Add one of the quick-add buckets to the selected project.
    fn quick_add(&mut self, duration_ms: i64) {
        match self.current_project() {
            Some(p) => {
                let project = p.id.clone();
                self.send(Action::QuickAdd {
                    project,
                    ms: duration_ms,
                });
            }
            None => {
                self.status = Some(StatusMsg::local(
                    "create a project on the Projects tab before adding time",
                ));
            }
        }
    }

    /// Whether the service has not been seen yet, in which case the window
    /// offers to start it instead of just complaining. Keys on the status
    /// kind, never on its wording.
    fn service_missing(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.kind == StatusKind::NoService)
    }

    fn try_start_service(&self) {
        self.send(Action::StartService);
    }

    fn footer(&self) -> String {
        self.status
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_else(|| {
            if self.dialog.is_some() {
                "type to edit · tab next field · enter save · esc cancel".to_string()
            } else if self.split_prompt.is_some() {
                "type a moment · enter split · esc cancel".to_string()
            } else if self.merge_confirm.is_some() {
                "m to confirm the merge · esc to cancel".to_string()
            } else if self.delete_confirm.is_some() {
                "d to confirm the delete · esc to cancel".to_string()
            } else if self.project_prompt.is_some() {
                "type a name · enter save · esc cancel".to_string()
            } else if self.export_picker.editing {
                "type a day YYYY-MM-DD · enter set · esc cancel".to_string()
            } else {
                "1-4 quick add · a add · e edit · s split · m merge · n project · r rename · u undo · d delete · tab tabs · j/k move · h/l project · q quit"
                    .to_string()
            }
        })
    }
}

impl Render for TimetrackView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain here as well as in the poll loop: a frame already being built
        // for an event should pick up any snapshot waiting in the channel
        // rather than painting one frame stale.
        self.drain();

        // The root holds the focus. `FocusHandle::focus` needs a
        // `&mut Window`, which `new` does not have, so it happens here:
        // gpui cannot dispatch a key event until a frame exists to dispatch
        // it against, and the first render is that frame. Without this the
        // keyboard is dead until the user clicks something focusable.
        //
        // It is re-asserted on every frame after that, not just the first.
        // The root is the only focusable node in this window, so re-asserting
        // cannot take the focus from anything else, and a focus lost to a
        // platform event would otherwise leave the keyboard dead with no way
        // back -- gpui dispatches key events along the ancestor path of the
        // focused node only, and there is no other node to fall back to.
        if !self.focus.is_focused(window) {
            self.focus.focus(window);
        }

        let this = cx.entity();
        self.entity = Some(this.clone());
        let maximized = window.is_maximized();

        // The titlebar is the only chrome above the content, and it is chrome
        // rather than data -- the week total is still the first *number* on
        // screen and nothing of substance sits above it.
        //
        // The `.id()` and `.track_focus()` are load-bearing, not decoration:
        // gpui dispatches key events along the ancestor path of the focused
        // node, and an element with no id is not in the dispatch tree at all.
        //
        // The window itself is transparent (see `main.rs`). When floating,
        // the panel below rounds all four corners; the titlebar rounds
        // its own top because the panel clip does not round children. A
        // maximized window stays square and full-bleed: rounded corners
        // would leave gaps at the screen edges.
        div()
            .id("root")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .text_color(rgba(theme::BODY_TEXT))
            .on_key_down(move |ev, _window, cx| {
                // `update` alone never repaints: it only runs the closure,
                // and a frame is scheduled solely by `notify`. Without it a
                // keystroke changes state that no frame ever shows.
                this.update(cx, |view, cx| {
                    if !view.on_key(ev) {
                        cx.quit();
                    } else {
                        cx.notify();
                    }
                });
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .bg(rgba(theme::WINDOW_BG))
                    .when(!maximized, |el| el.rounded_xl().overflow_hidden())
                    .child(self.render_titlebar(window))
                    // The prompt panels sit above the tab content, so a prompt
                    // opened on any tab is on screen. They are drawn here
                    // rather than inside the tab because the shortcuts that
                    // open them are global.
                    .child(self.render_prompts())
                    .child(match self.tab {
                        Tab::Home => self.render_home().into_any_element(),
                        Tab::Projects => self.render_projects().into_any_element(),
                        Tab::Export => self.render_export().into_any_element(),
                    })
                    .child(self.render_footer()),
            )
    }
}

impl TimetrackView {
    /// The integrated titlebar: the window has no system title bar (see
    /// `main.rs`), so this row is the drag handle, the tab strip and the
    /// minimize/maximize/close buttons in one.
    ///
    /// Only the title and the spacer drag. The tabs and buttons are siblings
    /// of those regions, never children, so pressing one can never start a
    /// move as a side effect of the event bubbling up to a draggable parent.
    fn render_titlebar(&self, window: &Window) -> impl IntoElement {
        let mut row = div()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .bg(rgba(theme::PANEL_BG))
            // The panel clip does not round children, so the titlebar
            // rounds its own top to meet the panel's corners. Square when
            // maximized, like the panel itself.
            .when(!window.is_maximized(), |el| el.rounded_t_xl());
        row = row.child(titlebar_drag("titlebar-drag", false, "TimeTrack"));
        for tab in Tab::all() {
            let is_selected = tab == self.tab;
            row = row.child(
                div()
                    .id(ElementId::from(tab.label()))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_selected {
                        rgba(theme::RAISED_BG)
                    } else {
                        rgba(0x00000000)
                    })
                    .text_color(if is_selected {
                        rgb(0xffffff)
                    } else {
                        rgba(theme::MUTED_TEXT)
                    })
                    .child(tab.label())
                    .when(!is_selected, move |el| {
                        let entity = self.entity.clone();
                        el.on_click(move |_ev, _window, cx| {
                            if let Some(entity) = entity.clone() {
                                entity.update(cx, |view, cx| {
                                    view.tab = tab;
                                    cx.notify();
                                });
                            }
                        })
                    }),
            );
        }
        row = row.child(titlebar_drag("titlebar-spacer", true, ""));
        row = row
            .child(window_button(
                "window-min",
                "−",
                WindowControlArea::Min,
                false,
                |window| window.minimize_window(),
            ))
            .child(window_button(
                "window-max",
                maximize_glyph(window.is_maximized()),
                WindowControlArea::Max,
                false,
                |window| window.zoom_window(),
            ))
            .child(window_button(
                "window-close",
                "×",
                WindowControlArea::Close,
                true,
                |window| window.remove_window(),
            ));
        row
    }

    /// The prompt panels, drawn above the tab content.
    ///
    /// They live here, in the shared wrapper, rather than inside
    /// `render_home`: a prompt can be opened from any tab (the shortcuts are
    /// global), so drawing it only on Home produced a modal that was never
    /// painted and that took the keyboard anyway.
    ///
    /// Which panel is drawn is the same question the key handler asks
    /// (`open_prompt`), so the two cannot drift apart.
    fn render_prompts(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .when_some(self.dialog.as_ref(), |el, dlg| {
                el.child(self.render_entry_dialog(dlg))
            })
            .when_some(self.project_prompt.as_ref(), |el, prompt| {
                el.child(self.render_project_prompt(prompt))
            })
            .when_some(self.split_prompt.as_ref(), |el, prompt| {
                el.child(self.render_split_prompt(prompt))
            })
            .when_some(self.merge_confirm.as_ref(), |el, confirm| {
                el.child(self.render_merge_confirm(confirm))
            })
            .when_some(self.delete_confirm.as_ref(), |el, confirm| {
                el.child(self.render_delete_confirm(confirm))
            })
    }

    /// The manual-entry dialog (REQUIREMENTS §5 methods 1 and 2, one form).
    ///
    /// A bordered panel at the top of Home, above the week total: while it is
    /// open it owns the keyboard (see `on_dialog_key`), so it sits where the
    /// eye already is rather than competing from below. Keyboard-driven like
    /// everything else here — no mouse targets, no toolkit text field.
    fn render_entry_dialog(&self, dlg: &EntryDialog) -> impl IntoElement {
        let tz = snapshot_tz(&self.snapshot);
        let title = match &dlg.edit_id {
            None => format!("NEW ENTRY → {}", dlg.project_name),
            Some(id) => format!("EDIT ENTRY {id} · {}", dlg.project_name),
        };
        let field_row = |label: &str, value: &str, hint: &str, focused: bool| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_1()
                .child(
                    div()
                        .w(px(110.))
                        .text_color(if focused {
                            rgb(0xffffff)
                        } else {
                            rgba(theme::MUTED_TEXT)
                        })
                        .child(format!("{} {label}", if focused { ">" } else { " " })),
                )
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(if focused {
                            rgba(theme::RAISED_BG)
                        } else {
                            rgba(0x00000000)
                        })
                        .text_color(rgba(theme::BODY_TEXT))
                        .child(format!("{value}{}", if focused { "_" } else { "" })),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgba(theme::HINT_TEXT))
                        .child(hint.to_string()),
                )
        };

        let mut panel = div()
            .mx_6()
            .mt_3()
            .mb_1()
            .px_4()
            .py_3()
            .rounded_md()
            .flex()
            .flex_col()
            .bg(rgba(theme::PANEL_BG))
            .child(div().text_color(rgba(theme::ACCENT)).child(title))
            .child(field_row(
                "what",
                &dlg.description,
                "optional note",
                dlg.field == DialogField::Description,
            ))
            .child(field_row(
                "start",
                &dlg.start,
                &dlg.start_hint(&tz),
                dlg.field == DialogField::Start,
            ))
            .child(field_row(
                "end",
                &dlg.end,
                &dlg.end_hint(&tz),
                dlg.field == DialogField::End,
            ))
            .child(
                div()
                    .pt_2()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child("type to edit · tab next field · enter save · esc cancel"),
            );
        if let Some(err) = &dlg.error {
            panel = panel.child(
                div()
                    .pt_1()
                    .text_color(rgba(theme::WARNING))
                    .child(err.clone()),
            );
        }
        panel
    }

    /// The split prompt: one moment field, same panel styling as the entry
    /// dialog, above the week total where the eye already is.
    fn render_split_prompt(&self, prompt: &SplitPrompt) -> impl IntoElement {
        let mut panel = div()
            .mx_6()
            .mt_3()
            .mb_1()
            .px_4()
            .py_3()
            .rounded_md()
            .flex()
            .flex_col()
            .bg(rgba(theme::PANEL_BG))
            .child(
                div()
                    .text_color(rgba(theme::ACCENT))
                    .child(format!("SPLIT ENTRY {}", prompt.entry_id)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(div().w(px(110.)).text_color(rgb(0xffffff)).child("> at"))
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(theme::RAISED_BG))
                            .text_color(rgba(theme::BODY_TEXT))
                            .child(format!("{}_", prompt.at)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgba(theme::HINT_TEXT))
                            .child("(-90, 90, now, HH:MM)"),
                    ),
            )
            .child(
                div()
                    .pt_2()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child("type a moment · enter split · esc cancel"),
            );
        if let Some(err) = &prompt.error {
            panel = panel.child(
                div()
                    .pt_1()
                    .text_color(rgba(theme::WARNING))
                    .child(err.clone()),
            );
        }
        panel
    }

    /// The merge confirmation banner: what would merge and how the total
    /// changes, computed when the confirmation opened. The amount is the
    /// point (§8): merging overlapping entries shrinks the total, and the
    /// user states the overlap was a mistake by confirming.
    fn render_merge_confirm(&self, confirm: &MergeConfirm) -> impl IntoElement {
        div()
            .mx_6()
            .mt_3()
            .mb_1()
            .px_4()
            .py_3()
            .rounded_md()
            .flex()
            .flex_col()
            .bg(rgba(theme::PANEL_BG))
            .child(
                div()
                    .text_color(rgba(theme::ACCENT))
                    .child(confirm.summary.clone()),
            )
    }

    /// The delete confirmation banner: exactly which entry would go, so a
    /// destructive action is never one keystroke with no reading step.
    fn render_delete_confirm(&self, confirm: &DeleteConfirm) -> impl IntoElement {
        div()
            .mx_6()
            .mt_3()
            .mb_1()
            .px_4()
            .py_3()
            .rounded_md()
            .flex()
            .flex_col()
            .bg(rgba(theme::PANEL_BG))
            .child(
                div()
                    .text_color(rgba(theme::ACCENT))
                    .child(confirm.summary.clone()),
            )
    }

    /// The project prompt: one name field, same panel styling as the other
    /// prompts, above the week total where the eye already is.
    fn render_project_prompt(&self, prompt: &ProjectPrompt) -> impl IntoElement {
        let title = match &prompt.edit_id {
            None => "NEW PROJECT".to_string(),
            Some(_) => "RENAME PROJECT".to_string(),
        };
        let mut panel = div()
            .mx_6()
            .mt_3()
            .mb_1()
            .px_4()
            .py_3()
            .rounded_md()
            .flex()
            .flex_col()
            .bg(rgba(theme::PANEL_BG))
            .child(div().text_color(rgba(theme::ACCENT)).child(title))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .child(div().w(px(110.)).text_color(rgb(0xffffff)).child("> name"))
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(theme::RAISED_BG))
                            .text_color(rgba(theme::BODY_TEXT))
                            .child(format!("{}_", prompt.name)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgba(theme::HINT_TEXT))
                            .child("a short name, e.g. Work"),
                    ),
            )
            .child(
                div()
                    .pt_2()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child("type a name · enter save · esc cancel"),
            );
        if let Some(err) = &prompt.error {
            panel = panel.child(
                div()
                    .pt_1()
                    .text_color(rgba(theme::WARNING))
                    .child(err.clone()),
            );
        }
        panel
    }

    /// Projects: every project with its week and all-time totals, plus an
    /// archive action. Archived projects stay listed, because their history
    /// still counts (REQUIREMENTS §14).
    fn render_projects(&self) -> impl IntoElement {
        // A "New project" row rather than a dead action: it opens the naming
        // prompt, so every project name is typed -- never invented.
        let new_project = {
            let entity = self.entity.clone();
            div()
                .id("new-project")
                .mx_6()
                .my_3()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .bg(rgba(theme::ACTION))
                .text_color(rgb(0xffffff))
                .child("New project")
                .when_some(entity, |this, entity| {
                    this.on_click(move |_ev, _window, cx| {
                        entity.update(cx, |view, cx| {
                            view.open_project_prompt();
                            cx.notify();
                        });
                    })
                })
        };

        if self.snapshot.projects.is_empty() {
            return div()
                .flex_1()
                .flex_col()
                .px_6()
                .py_4()
                .child(
                    div()
                        .text_color(rgba(theme::HINT_TEXT))
                        .child("No projects yet — create one to start tracking time."),
                )
                .child(new_project)
                .into_any_element();
        }

        let rows = self.snapshot.projects.iter().map(|p| {
            // Both columns come from the service's aggregates, never re-summed
            // here: the snapshot may carry only recent entries while the
            // aggregates cover the whole store, and re-summing under-reports
            // a large store's all-time column next to a correct week column.
            let (week, all) = self.snapshot.project_totals(&p.id);
            let id = p.id.clone();
            let name = p.name.clone();
            // The project's colour, if it has one, else the neutral row grey:
            // recolours set from the CLI show up here without another action.
            let dot = theme::project_colour(p.colour);
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_6()
                .py_2()
                .child(div().w(px(12.)).h(px(12.)).rounded_md().bg(dot))
                .child(
                    div()
                        .flex_1()
                        .text_color(if p.archived {
                            rgba(theme::HINT_TEXT)
                        } else {
                            rgba(theme::BODY_TEXT)
                        })
                        .child(if p.archived {
                            format!("{name}  (archived)")
                        } else {
                            name.clone()
                        }),
                )
                .child(
                    div()
                        .w(px(110.))
                        .text_color(rgba(theme::ACCENT))
                        .child(timetrack_core::format_duration(week)),
                )
                .child(
                    div()
                        .w(px(110.))
                        .text_color(rgba(theme::MUTED_TEXT))
                        .child(timetrack_core::format_duration(all)),
                )
                // Archiving is offered only for live projects; an archived one
                // offers the reverse instead, so archiving is reversible
                // without the CLI.
                .when(!p.archived, {
                    let entity = self.entity.clone();
                    let project_id = id.clone();
                    move |el| {
                        el.child(
                            div()
                                .id(project_button_id("archive", &p.id))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgba(0x3a3a4aff))
                                .text_color(rgba(theme::STRONG_TEXT))
                                .child("Archive")
                                .on_click(move |_ev, _window, cx| {
                                    if let Some(entity) = entity.clone() {
                                        entity.update(cx, |view, cx| {
                                            view.send(Action::ArchiveProject(project_id.clone()));
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                    }
                })
                .when(p.archived, {
                    let entity = self.entity.clone();
                    let project_id = id.clone();
                    move |el| {
                        el.child(
                            div()
                                .id(project_button_id("unarchive", &p.id))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgba(theme::ACTION_FADED))
                                .text_color(rgba(theme::STRONG_TEXT))
                                .child("Unarchive")
                                .on_click(move |_ev, _window, cx| {
                                    if let Some(entity) = entity.clone() {
                                        entity.update(cx, |view, cx| {
                                            view.send(Action::UnarchiveProject(project_id.clone()));
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                    }
                })
        });

        div()
            .id("projects-scroll")
            .flex_1()
            .flex_col()
            .overflow_y_scroll()
            .child(
                div()
                    .px_6()
                    .pt_2()
                    .pb_1()
                    .text_sm()
                    .text_color(rgba(theme::MUTED_TEXT))
                    .child("PROJECTS   (this week · all time)"),
            )
            .child(div().flex().flex_col().children(rows))
            .child(new_project)
            .into_any_element()
    }

    /// One scope pill on the Export tab. Neutral rather than green: green is
    /// the export-action colour the smoke check counts.
    fn render_scope_pill(
        &self,
        scope: timetrack_core::ExportScope,
        id: &'static str,
        label: &'static str,
    ) -> impl IntoElement {
        let selected = self.export_picker.scope == scope;
        let entity = self.entity.clone();
        div()
            .id(ElementId::from(id))
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .bg(if selected {
                rgba(theme::RAISED_BG)
            } else {
                rgba(0x00000000)
            })
            .text_color(if selected {
                rgb(0xffffff)
            } else {
                rgba(theme::MUTED_TEXT)
            })
            .child(label)
            .when_some(entity, move |el, entity| {
                el.on_click(move |_ev, _window, cx| {
                    entity.update(cx, |view, cx| {
                        let tz = snapshot_tz(&view.snapshot);
                        view.export_picker.set_scope(scope, &tz);
                        cx.notify();
                    });
                })
            })
    }

    /// One anchor stepper button. Inert (present but clickless) for all time,
    /// which ignores the anchor. Neutral rather than green, as above.
    fn render_step_button(
        &self,
        id: &'static str,
        label: &'static str,
        delta: i32,
    ) -> impl IntoElement {
        let active = self.export_picker.is_active();
        let entity = self.entity.clone();
        let mut button = div()
            .id(ElementId::from(id))
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgba(0x3a3a4aff))
            .text_color(if active {
                rgba(theme::STRONG_TEXT)
            } else {
                rgba(theme::HINT_TEXT)
            })
            .child(label);
        if active {
            button = button
                .cursor_pointer()
                .when_some(entity, move |el, entity| {
                    el.on_click(move |_ev, _window, cx| {
                        entity.update(cx, |view, cx| {
                            view.step_export_anchor(delta);
                            cx.notify();
                        });
                    })
                });
        } else {
            button = button.cursor_default();
        }
        button
    }

    /// Export. The service owns aggregation, so what is shown here is exactly
    /// what a CSV would contain, rather than a second aggregation that could
    /// disagree with the totals on Home.
    fn render_export(&self) -> impl IntoElement {
        let mut body = div()
            .id("export-scroll")
            .flex_1()
            .flex_col()
            .px_6()
            .py_2()
            .overflow_y_scroll();

        body = body.child(
            div()
                .text_sm()
                .text_color(rgba(theme::MUTED_TEXT))
                .child("EXPORT"),
        );
        body = body.child(
            div()
                .pt_2()
                .text_color(rgba(theme::STRONG_TEXT))
                .child(format!(
                    "{} entries · {} this week · {} all time",
                    self.snapshot.entries.len(),
                    timetrack_core::format_duration(self.snapshot.week.total_ms),
                    timetrack_core::format_duration(self.snapshot.total_ms),
                )),
        );

        // Scope plus anchor: the stepper and the typed day edit one shared
        // anchor, and all time visibly ignores it. The preview and the
        // export buttons below follow this scope plus anchor.
        let tz = snapshot_tz(&self.snapshot);
        let picker_active = self.export_picker.is_active();
        body = body.child(
            div()
                .pt_4()
                .text_sm()
                .text_color(rgba(theme::MUTED_TEXT))
                .child("SCOPE"),
        );
        {
            let mut row = div().flex().gap_2().pt_1();
            for (scope, id, label) in [
                (timetrack_core::ExportScope::Week, "scope-week", "Week"),
                (timetrack_core::ExportScope::Month, "scope-month", "Month"),
                (timetrack_core::ExportScope::All, "scope-all", "All"),
            ] {
                row = row.child(self.render_scope_pill(scope, id, label));
            }
            body = body.child(row);
        }
        body = body.child(
            div()
                .pt_3()
                .text_sm()
                .text_color(rgba(theme::MUTED_TEXT))
                .child("ANCHOR"),
        );
        {
            let mut row = div().flex().items_center().gap_2().pt_1();
            row = row.child(self.render_step_button("anchor-prev", "< prev", -1));
            row = row.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(if picker_active {
                        rgb(0xffffff)
                    } else {
                        rgba(theme::HINT_TEXT)
                    })
                    .child(self.export_picker.period_label(&tz)),
            );
            row = row.child(self.render_step_button("anchor-next", "next >", 1));
            body = body.child(row);
        }
        {
            let field_text = if self.export_picker.editing {
                format!("{}_", self.export_picker.anchor_text)
            } else {
                self.export_picker.anchor_text.clone()
            };
            let entity = self.entity.clone();
            let mut field = div()
                .id(ElementId::from("anchor-field"))
                .flex_1()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(if self.export_picker.editing {
                    rgba(theme::RAISED_BG)
                } else {
                    rgba(0x00000000)
                })
                .text_color(if picker_active {
                    rgba(theme::BODY_TEXT)
                } else {
                    rgba(theme::HINT_TEXT)
                })
                .child(format!("day {field_text}"));
            if picker_active {
                field = field.cursor_pointer().when_some(entity, |el, entity| {
                    el.on_click(move |_ev, _window, cx| {
                        entity.update(cx, |view, cx| {
                            view.export_picker.start_editing();
                            cx.notify();
                        });
                    })
                });
            } else {
                field = field.cursor_default();
            }
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .child(field)
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgba(theme::HINT_TEXT))
                            .child("(YYYY-MM-DD)"),
                    ),
            );
        }
        if picker_active {
            body = body.child(
                div()
                    .pt_1()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child("t type a day · [ previous period · ] next period"),
            );
        } else {
            body = body.child(
                div()
                    .pt_1()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child("anchor ignored for all time"),
            );
        }
        if let Some(err) = &self.export_picker.error {
            body = body.child(
                div()
                    .pt_1()
                    .text_color(rgba(theme::WARNING))
                    .child(err.clone()),
            );
        }

        // The columns a CSV export will carry, shown as a header so the shape
        // is visible without opening a file.
        body = body.child(
            div()
                .pt_4()
                .text_sm()
                .text_color(rgba(theme::MUTED_TEXT))
                .child("CSV COLUMNS"),
        );
        body = body.child(
            div()
                .pt_1()
                .text_color(rgba(theme::ROW_TEXT))
                .child(timetrack_core::CSV_HEADER),
        );

        // Export actions: fetch the CSV over D-Bus and write it to a
        // period-stamped name the status line reports. Scopes match the
        // CLI's `timetrack export --scope week|month|all`; each button saves
        // its own scope with the shared anchor (all time ignores it), so the
        // buttons keep working while the preview follows the picker.
        {
            let mut row = div().flex().gap_2().pt_3();
            for (i, button_scope) in [
                timetrack_core::ExportScope::Week,
                timetrack_core::ExportScope::Month,
                timetrack_core::ExportScope::All,
            ]
            .iter()
            .enumerate()
            {
                let entity = self.entity.clone();
                let button_scope = *button_scope;
                let label = format!("Export {}", button_scope.as_str());
                row = row.child(
                    div()
                        .id(ElementId::from(("export", i as u64)))
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgba(theme::ACTION))
                        .text_color(rgb(0xffffff))
                        .child(label)
                        .when_some(entity, move |el, entity| {
                            el.on_click(move |_ev, _window, cx| {
                                entity.update(cx, |view, cx| {
                                    // Everything about the request is computed
                                    // here, from one snapshot and one zone:
                                    // the destination used to be computed at
                                    // render time and captured, so a zone
                                    // that changed between the last paint and
                                    // the click would have put the file in a
                                    // directory named for a different week.
                                    let tz = snapshot_tz(&view.snapshot);
                                    let request = export_request(
                                        button_scope,
                                        view.export_picker.anchor_ms,
                                        now_ms(),
                                        &tz,
                                    );
                                    view.send(Action::ExportCsv(request));
                                    cx.notify();
                                });
                            })
                        }),
                );
            }
            body = body.child(row);
            body = body.child(
                div()
                    .pt_1()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child(export_location_text(
                        self.export_last_dest
                            .as_deref()
                            .map(|dest| (dest, self.export_last_via_portal)),
                    )),
            );
        }

        // A preview of the first rows of the scoped export, so what is seen
        // is what the stakeholder gets: the picker's scope plus anchor,
        // oldest first (all time ignores the anchor).
        body = body.child(
            div()
                .pt_4()
                .text_sm()
                .text_color(rgba(theme::MUTED_TEXT))
                .child("PREVIEW"),
        );
        let preview = export_preview_entries(
            &self.snapshot,
            self.export_picker.scope,
            self.export_picker.anchor_ms,
            &tz,
        );
        for e in preview.iter().take(20) {
            let project = self
                .snapshot
                .projects
                .iter()
                .find(|p| p.id == e.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| e.project_id.clone());
            body = body.child(
                div()
                    .flex()
                    .gap_3()
                    .py_1()
                    .text_color(rgba(theme::ROW_TEXT))
                    .child(e.id.clone())
                    .child(project)
                    .child(timetrack_core::format_duration(e.duration_ms()))
                    .child(e.label().to_string()),
            );
        }
        if preview.len() > 20 {
            body = body.child(
                div()
                    .pt_1()
                    .text_sm()
                    .text_color(rgba(theme::HINT_TEXT))
                    .child(format!("… and {} more", preview.len() - 20)),
            );
        }
        body.into_any_element()
    }

    fn render_footer(&self) -> impl IntoElement {
        if self.service_missing() {
            let Some(this) = self.entity.clone() else {
                return div().into_any_element();
            };
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .px_6()
                .py_3()
                .child(
                    div().text_sm().text_color(rgba(theme::STRONG_TEXT)).child(
                        self.status
                            .as_ref()
                            .map(|s| s.text.clone())
                            .unwrap_or_default(),
                    ),
                )
                .child(
                    div()
                        .id("start-service")
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgba(theme::ACTION))
                        .text_color(rgb(0xffffff))
                        .on_click(move |_ev, _window, cx| {
                            this.update(cx, |view, cx| {
                                view.try_start_service();
                                cx.notify();
                            });
                        })
                        .child("Start the service"),
                )
                .into_any_element();
        }
        div()
            .px_6()
            .py_2()
            .text_sm()
            .text_color(rgba(theme::HINT_TEXT))
            .child(self.footer())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::testsupport::*;
    use super::*;
    use timetrack_proto::EntrySource;

    #[test]
    fn quick_add_buckets_are_5_15_30_60() {
        // The Home row and the number keys are hardcoded to these, so a change
        // to the core constant must break this test rather than the UI.
        let mins: Vec<i64> = QUICK_ADD_MS.iter().map(|m| m / 60_000).collect();
        assert_eq!(mins, [5, 15, 30, 60]);
    }

    #[test]
    fn no_running_state_survives_anywhere() {
        // A regression guard: the stopwatch's running entry must not creep
        // back in, because there is no such thing any more (REQUIREMENTS §7).
        // The snapshot has no `running` field, so there is nothing that could
        // resurrect one: every entry is closed (REQUIREMENTS §7).
        let snap = Snapshot::default();
        assert!(snap.entries.is_empty());
        for e in &snap.entries {
            // Every entry carries a real end, never an Option.
            let _: i64 = e.ended_at;
        }
    }

    #[test]
    fn undo_targets_the_newest_quick_add() {
        let snap = Snapshot {
            // Newest first, as the service sends them.
            entries: vec![
                entry("e3", EntrySource::QuickAdd, 2_000),
                entry("e2", EntrySource::Manual, 1_000),
                entry("e1", EntrySource::QuickAdd, 0),
            ],
            ..Default::default()
        };
        assert_eq!(newest_quick_add(&snap).as_deref(), Some("e3"));
    }

    #[test]
    fn there_is_nothing_to_undo_without_a_quick_add() {
        let snap = Snapshot {
            entries: vec![entry("e1", EntrySource::Manual, 0)],
            ..Default::default()
        };
        assert_eq!(newest_quick_add(&snap), None);
    }

    // --- typed status kinds (no string parsing) ---

    #[test]
    fn only_connection_notices_are_cleared_by_a_snapshot() {
        // A snapshot proves the service is back, and that it is speaking this
        // client's interface -- so those two notices go. An action error
        // survives, because snapshots arrive every second.
        assert!(is_connection_notice(StatusKind::NoService));
        assert!(is_connection_notice(StatusKind::VersionMismatch));
        assert!(!is_connection_notice(StatusKind::Service));
        assert!(!is_connection_notice(StatusKind::Local));
    }

    // --- which prompt is open ---

    #[test]
    fn no_open_prompt_means_no_prompt() {
        assert_eq!(which_prompt(false, false, false, false, false), None);
    }

    #[test]
    fn each_open_prompt_is_the_one_that_owns_the_keyboard() {
        assert_eq!(
            which_prompt(true, false, false, false, false),
            Some(OpenPrompt::Entry)
        );
        assert_eq!(
            which_prompt(false, true, false, false, false),
            Some(OpenPrompt::Project)
        );
        assert_eq!(
            which_prompt(false, false, true, false, false),
            Some(OpenPrompt::Split)
        );
        assert_eq!(
            which_prompt(false, false, false, true, false),
            Some(OpenPrompt::Merge)
        );
        assert_eq!(
            which_prompt(false, false, false, false, true),
            Some(OpenPrompt::Delete)
        );
    }

    #[test]
    fn the_entry_dialog_wins_over_every_other_prompt() {
        // The key handler used to check the prompts one by one, in this
        // order; the shared question has to keep it.
        assert_eq!(
            which_prompt(true, true, true, true, true),
            Some(OpenPrompt::Entry)
        );
        assert_eq!(
            which_prompt(false, true, true, true, true),
            Some(OpenPrompt::Project)
        );
        assert_eq!(
            which_prompt(false, false, true, true, true),
            Some(OpenPrompt::Split)
        );
        assert_eq!(
            which_prompt(false, false, false, true, true),
            Some(OpenPrompt::Merge)
        );
    }

    // --- the project row buttons ---

    #[test]
    fn two_projects_never_share_a_button_element_id() {
        // The old id parsed the numeric suffix out of the project id and
        // fell back to 0 for anything else, so two ids that were not
        // `p<digits>` would have collided.
        assert_ne!(
            project_button_id("archive", "p1"),
            project_button_id("archive", "p2")
        );
        assert_ne!(
            project_button_id("archive", "imported"),
            project_button_id("archive", "other")
        );
        assert_ne!(
            project_button_id("archive", "imported"),
            project_button_id("unarchive", "imported"),
            "the two buttons of one project must differ too"
        );
    }

    #[test]
    fn one_project_always_gets_the_same_button_element_id() {
        assert_eq!(
            project_button_id("archive", "p7"),
            project_button_id("archive", "p7")
        );
    }

    // --- tabs and bars (§11) ---

    #[test]
    fn bars_fill_the_track_proportionally() {
        assert_eq!(bar_width(3_600_000, 3_600_000), PROJECT_BAR_TRACK_PX);
        assert_eq!(bar_width(1_800_000, 3_600_000), PROJECT_BAR_TRACK_PX / 2.0);
        assert_eq!(bar_width(0, 3_600_000), 0.0);
        // No entries (or no time) anywhere: zero width, never NaN.
        assert_eq!(bar_width(0, 0), 0.0);
    }

    #[test]
    fn an_export_request_names_one_period_everywhere() {
        // The destination used to be computed at render time and captured in
        // the click closure, so it could come from a different snapshot than
        // the period the request names. Everything is derived together now.
        let scope = timetrack_core::ExportScope::Week;
        let anchor = day_anchor("2026-10-08");
        // `now` in a different week, so the request names a *chosen* period
        // and takes the anchored call rather than the current-period one.
        let now = day_anchor("2026-11-05");
        let request = export_request(scope, anchor, now, &utc_tz());
        assert_eq!(request.period_id, "2026-W41");
        assert_eq!(request.suggested_name, "timetrack-export-week-2026-W41.csv");
        assert!(
            request.path.ends_with("timetrack-export-week-2026-W41.csv"),
            "the fallback path names the same week: {}",
            request.path
        );
        assert_eq!(
            request.anchor_ms,
            Some(anchor),
            "a chosen period uses the anchored call"
        );
    }

    #[test]
    fn an_export_request_for_all_time_ignores_the_anchor() {
        let anchor = day_anchor("2026-10-08");
        let request = export_request(timetrack_core::ExportScope::All, anchor, anchor, &utc_tz());
        assert_eq!(request.period_id, "all time");
        assert_eq!(request.suggested_name, "timetrack-export-all.csv");
        assert_eq!(request.anchor_ms, None, "all time uses the old call");
    }
}

#[cfg(test)]
mod testsupport;

mod status;

mod tabs;

mod chrome;

mod keys;

mod service;

mod theme;

mod dialogs;

mod export;

mod render;
