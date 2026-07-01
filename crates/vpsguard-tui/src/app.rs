//! TUI application state, independent of rendering and IO.

use std::collections::BTreeSet;

use vpsguard_core::State;

/// One module row in the dashboard.
pub struct Row {
    pub name: String,
    pub summary: String,
    pub state: State,
    #[allow(dead_code)]
    pub detail: String,
}

/// A confirmable action pending operator approval.
#[derive(Clone, Copy)]
pub enum Action {
    Apply,
    Uninstall,
}

/// Messages sent from background workers to the UI.
pub enum Msg {
    Rows(Vec<Row>),
    Log(String),
    Busy(bool),
}

/// Dashboard state.
#[derive(Default)]
pub struct App {
    pub rows: Vec<Row>,
    pub selected: usize,
    pub marked: BTreeSet<String>,
    pub log: Vec<String>,
    pub busy: bool,
    pub confirm: Option<Action>,
    pub should_quit: bool,
}

impl App {
    pub fn next(&mut self) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + 1) % self.rows.len();
        }
    }

    pub fn prev(&mut self) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + self.rows.len() - 1) % self.rows.len();
        }
    }

    pub fn current(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn toggle_mark(&mut self) {
        if let Some(row) = self.rows.get(self.selected) {
            if !self.marked.remove(&row.name) {
                self.marked.insert(row.name.clone());
            }
        }
    }

    /// Modules an action targets: the marked set, or all when none are marked.
    pub fn targets(&self) -> Vec<String> {
        if self.marked.is_empty() {
            self.rows.iter().map(|r| r.name.clone()).collect()
        } else {
            self.marked.iter().cloned().collect()
        }
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        let max = 500;
        if self.log.len() > max {
            self.log.drain(0..self.log.len() - max);
        }
    }

    /// (compliant, applicable) counts for the security score.
    pub fn score(&self) -> (usize, usize) {
        let applicable = self
            .rows
            .iter()
            .filter(|r| r.state != State::NotApplicable)
            .count();
        let compliant = self
            .rows
            .iter()
            .filter(|r| r.state == State::Compliant)
            .count();
        (compliant, applicable)
    }
}
