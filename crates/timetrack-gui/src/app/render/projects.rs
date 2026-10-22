/* gui/app/render/projects.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The Projects tab: every project with its week and all-time totals, plus
//! an archive action. Archived projects stay listed, because their history
//! still counts (REQUIREMENTS §14).

use super::super::{TimetrackView, project_button_id};
use super::super::{service::Action, theme};
use gpui::prelude::*;
use gpui::{IntoElement, div, px, rgb, rgba};

impl TimetrackView {
    /// Projects: every project with its week and all-time totals, plus an
    /// archive action. Archived projects stay listed, because their history
    /// still counts (REQUIREMENTS §14).
    pub(crate) fn render_projects(&self) -> impl IntoElement {
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
}
