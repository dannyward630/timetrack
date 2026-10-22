/* gui/app/tabs.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The three tabs of the window (REQUIREMENTS §13), and the order they
//! cycle in.

/// The three tabs (REQUIREMENTS §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Home,
    Projects,
    Export,
}

impl Tab {
    pub(super) fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Projects => "Projects",
            Tab::Export => "Export",
        }
    }

    pub(super) fn all() -> [Tab; 3] {
        [Tab::Home, Tab::Projects, Tab::Export]
    }
}

/// The tab after Tab / Shift+Tab from `current`, wrapping around both ends.
///
/// Free of the view so the cycle order is unit-testable: the key handler
/// only forwards the shift state.
pub(super) fn cycle_tab(current: Tab, backward: bool) -> Tab {
    let tabs = Tab::all();
    let at = tabs.iter().position(|t| *t == current).unwrap_or(0);
    let next = if backward {
        (at + tabs.len() - 1) % tabs.len()
    } else {
        (at + 1) % tabs.len()
    };
    tabs[next]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_exactly_three_tabs() {
        assert_eq!(Tab::all().len(), 3);
        assert_eq!(Tab::all()[0], Tab::Home);
    }

    #[test]
    fn tab_labels_match_the_requirements() {
        assert_eq!(Tab::Home.label(), "Home");
        assert_eq!(Tab::Projects.label(), "Projects");
        assert_eq!(Tab::Export.label(), "Export");
    }

    #[test]
    fn every_tab_has_a_distinct_label() {
        // Two tabs sharing a label would make the strip ambiguous.
        let labels: Vec<&str> = Tab::all().iter().map(|t| t.label()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len());
    }

    #[test]
    fn tab_cycles_forward_through_all_three() {
        assert_eq!(cycle_tab(Tab::Home, false), Tab::Projects);
        assert_eq!(cycle_tab(Tab::Projects, false), Tab::Export);
        assert_eq!(cycle_tab(Tab::Export, false), Tab::Home);
    }

    #[test]
    fn tab_cycles_backward_through_all_three() {
        assert_eq!(cycle_tab(Tab::Home, true), Tab::Export);
        assert_eq!(cycle_tab(Tab::Export, true), Tab::Projects);
        assert_eq!(cycle_tab(Tab::Projects, true), Tab::Home);
    }
}
