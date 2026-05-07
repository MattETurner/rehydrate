//! SQLite version log. Schema is in `migrations/0001_init.sql` and is
//! embedded at compile time. Migrations are applied by `Db::open`.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;

const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("../migrations/0001_init.sql"))];

pub struct Db {
    pub(crate) conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let mut db = Self { conn };
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                name TEXT PRIMARY KEY,\
                applied_at TEXT NOT NULL)",
            [],
        )?;
        for (name, sql) in MIGRATIONS {
            let already: Option<String> = self
                .conn
                .query_row(
                    "SELECT name FROM schema_migrations WHERE name = ?1",
                    params![name],
                    |r| r.get(0),
                )
                .optional()?;
            if already.is_some() {
                continue;
            }
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(name, applied_at) VALUES (?1, datetime('now'))",
                params![name],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_applies_initial_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("db.sqlite")).unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        // Tables exist:
        for table in [
            "documents",
            "versions",
            "folders",
            "sync_state",
            "blob_refs",
        ] {
            let n: i64 = db
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {table}");
        }
    }

    #[test]
    fn reopen_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("db.sqlite");
        let _ = Db::open(&p).unwrap();
        let _ = Db::open(&p).unwrap();
    }
}
