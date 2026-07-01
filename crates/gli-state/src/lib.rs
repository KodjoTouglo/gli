//! Local SQLite history of apply/rollback events.
//!
//! History is best-effort: callers record events but must not fail their main
//! operation if recording fails. The database is a single file (bundled SQLite,
//! no system dependency) created on first use.

#![forbid(unsafe_code)]

use std::path::Path;

use rusqlite::Connection;
use thiserror::Error;

/// Default on-disk location for the history database.
pub const DEFAULT_PATH: &str = "/var/lib/gli/history.db";

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A recorded action against a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub timestamp: String,
    /// "apply", "rollback", etc.
    pub action: String,
    pub module: String,
    pub summary: String,
    pub ok: bool,
}

/// Append-only history store.
pub struct History {
    conn: Connection,
}

impl History {
    /// Open (creating parent dirs and schema) the history database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// Open an in-memory database (tests).
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                 id      INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts      TEXT NOT NULL DEFAULT (datetime('now')),
                 action  TEXT NOT NULL,
                 module  TEXT NOT NULL,
                 summary TEXT NOT NULL,
                 ok      INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    /// Record one event.
    pub fn record(&self, action: &str, module: &str, summary: &str, ok: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (action, module, summary, ok) VALUES (?1, ?2, ?3, ?4)",
            (action, module, summary, ok as i64),
        )?;
        Ok(())
    }

    /// Most recent events, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT ts, action, module, summary, ok
             FROM events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(Event {
                timestamp: row.get(0)?,
                action: row.get(1)?,
                module: row.get(2)?,
                summary: row.get(3)?,
                ok: row.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back_newest_first() {
        let h = History::in_memory().unwrap();
        h.record("apply", "ssh", "3 changes", true).unwrap();
        h.record("apply", "firewall", "rejected", false).unwrap();

        let events = h.recent(10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].module, "firewall");
        assert!(!events[0].ok);
        assert_eq!(events[1].module, "ssh");
        assert!(events[1].ok);
    }

    #[test]
    fn recent_respects_limit() {
        let h = History::in_memory().unwrap();
        for i in 0..5 {
            h.record("apply", "ssh", &format!("run {i}"), true).unwrap();
        }
        assert_eq!(h.recent(3).unwrap().len(), 3);
    }

    #[test]
    fn empty_history_reads_empty() {
        let h = History::in_memory().unwrap();
        assert!(h.recent(10).unwrap().is_empty());
    }
}
