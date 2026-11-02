//! SQLite storage for the lists that grow with the library: the download list,
//! the history of finished downloads and the statistics.
//!
//! Each download and history entry is one row holding its JSON, so a save only
//! writes the rows that changed instead of rewriting everything. Settings stay
//! in `settings.json`, which people edit by hand.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Error;

pub(crate) const FILE: &str = "state.db";
/// The copy kept next to the database, refreshed at most this often.
const BACKUP_EVERY: Duration = Duration::from_secs(600);

fn err(e: rusqlite::Error) -> Error {
    Error::Io(e.to_string())
}

fn json_err(e: serde_json::Error) -> Error {
    Error::Io(e.to_string())
}

/// The open database and what was last written, so saves only touch changes.
pub(crate) struct Db {
    conn: Connection,
    path: PathBuf,
    downloads: HashMap<u64, String>,
    history: HashMap<u64, (i64, String)>,
    meta: HashMap<String, String>,
    last_backup: Option<Instant>,
}

/// What opening the database found.
pub(crate) struct Opened {
    pub db: Db,
    /// Set when the file was damaged and had to be replaced.
    pub notice: Option<String>,
}

impl Db {
    pub fn open(dir: &Path) -> Result<Opened, Error> {
        let path = dir.join(FILE);
        let mut notice = None;
        let conn = match Self::connect(&path) {
            Ok(conn) => conn,
            Err(_) => {
                let aside = dir.join(format!("{FILE}.corrupt-{}", crate::util::now_unix()));
                for suffix in ["", "-wal", "-shm"] {
                    let from = PathBuf::from(format!("{}{suffix}", path.display()));
                    let to = PathBuf::from(format!("{}{suffix}", aside.display()));
                    let _ = std::fs::rename(from, to);
                }
                let backup = backup_path(&path);
                let restored = std::fs::copy(&backup, &path).is_ok() && Self::connect(&path).is_ok();
                notice = Some(if restored {
                    format!(
                        "The download database was damaged and was restored from the latest backup; the damaged file is kept as {}",
                        aside.display()
                    )
                } else {
                    let _ = std::fs::remove_file(&path);
                    format!(
                        "The download database was damaged and no usable backup exists; the damaged file is kept as {}",
                        aside.display()
                    )
                });
                Self::connect(&path)?
            }
        };
        Ok(Opened { db: Self::load(conn, path), notice })
    }

    fn connect(path: &Path) -> Result<Connection, Error> {
        let conn = Connection::open(path).map_err(err)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(err)?;
        // Write-ahead logging keeps the file consistent through a crash or power cut.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0)).map_err(err)?;
        conn.execute_batch(
            "PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS downloads (id INTEGER PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, seq INTEGER NOT NULL, json TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS history_seq ON history (seq);
             CREATE TABLE IF NOT EXISTS meta (name TEXT PRIMARY KEY, json TEXT NOT NULL);",
        )
        .map_err(err)?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(err)?;
        if check != "ok" {
            return Err(Error::Io(format!("integrity check failed: {check}")));
        }
        Ok(conn)
    }

    fn load(conn: Connection, path: PathBuf) -> Db {
        let mut db = Db {
            conn,
            path,
            downloads: HashMap::new(),
            history: HashMap::new(),
            meta: HashMap::new(),
            last_backup: None,
        };
        db.refresh_cache();
        db
    }

    /// Re-reads what is stored, so the next save writes only what differs from it.
    fn refresh_cache(&mut self) {
        self.downloads.clear();
        self.history.clear();
        self.meta.clear();
        if let Ok(mut s) = self.conn.prepare("SELECT id, json FROM downloads")
            && let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?)))
        {
            self.downloads.extend(rows.flatten());
        }
        if let Ok(mut s) = self.conn.prepare("SELECT id, seq, json FROM history")
            && let Ok(rows) =
                s.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, (r.get::<_, i64>(1)?, r.get::<_, String>(2)?))))
        {
            self.history.extend(rows.flatten());
        }
        if let Ok(mut s) = self.conn.prepare("SELECT name, json FROM meta")
            && let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            self.meta.extend(rows.flatten());
        }
    }

    /// Whether anything was ever saved in the given table (`downloads`, `history` or a meta name).
    pub fn has_data(&self, table: &str) -> bool {
        match table {
            "downloads" => !self.downloads.is_empty() || self.meta.contains_key("next_id"),
            "history" => !self.history.is_empty() || self.meta.contains_key("history"),
            name => self.meta.contains_key(name),
        }
    }

    pub fn meta<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        serde_json::from_str(self.meta.get(name)?).ok()
    }

    pub fn set_meta<T: Serialize>(&mut self, name: &str, value: &T) -> Result<(), Error> {
        let json = serde_json::to_string(value).map_err(json_err)?;
        if self.meta.get(name) == Some(&json) {
            return Ok(());
        }
        self.conn
            .execute(
                "INSERT INTO meta (name, json) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET json = ?2",
                params![name, json],
            )
            .map_err(err)?;
        self.meta.insert(name.to_owned(), json);
        self.backup_if_due();
        Ok(())
    }

    /// Every stored row of `downloads`, in id order.
    pub fn download_rows<T: DeserializeOwned>(&self) -> Vec<T> {
        let mut ids: Vec<_> = self.downloads.keys().copied().collect();
        ids.sort_unstable();
        ids.iter().filter_map(|id| serde_json::from_str(&self.downloads[id]).ok()).collect()
    }

    /// Replaces the stored downloads with `rows` (id and value), writing only what changed.
    pub fn save_downloads<T: Serialize>(&mut self, rows: &[(u64, &T)]) -> Result<(), Error> {
        let mut next: HashMap<u64, String> = HashMap::with_capacity(rows.len());
        for (id, value) in rows {
            next.insert(*id, serde_json::to_string(value).map_err(json_err)?);
        }
        let tx = self.conn.transaction().map_err(err)?;
        {
            let mut upsert = tx
                .prepare_cached(
                    "INSERT INTO downloads (id, json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET json = ?2",
                )
                .map_err(err)?;
            for (id, json) in &next {
                if self.downloads.get(id) != Some(json) {
                    upsert.execute(params![*id as i64, json]).map_err(err)?;
                }
            }
            let mut delete = tx.prepare_cached("DELETE FROM downloads WHERE id = ?1").map_err(err)?;
            for id in self.downloads.keys().filter(|id| !next.contains_key(id)) {
                delete.execute(params![*id as i64]).map_err(err)?;
            }
        }
        tx.commit().map_err(err)?;
        self.downloads = next;
        self.backup_if_due();
        Ok(())
    }

    /// Every stored history entry, oldest first.
    pub fn history_rows<T: DeserializeOwned>(&self) -> Vec<T> {
        let mut rows: Vec<_> = self.history.values().collect();
        rows.sort_by_key(|(seq, _)| *seq);
        rows.iter().filter_map(|(_, json)| serde_json::from_str(json).ok()).collect()
    }

    /// Replaces the stored history with `rows` (id and value, oldest first).
    pub fn save_history<T: Serialize>(&mut self, rows: &[(u64, &T)]) -> Result<(), Error> {
        let mut next: HashMap<u64, (i64, String)> = HashMap::with_capacity(rows.len());
        for (seq, (id, value)) in rows.iter().enumerate() {
            next.insert(*id, (seq as i64, serde_json::to_string(value).map_err(json_err)?));
        }
        let tx = self.conn.transaction().map_err(err)?;
        {
            let mut upsert = tx
                .prepare_cached(
                    "INSERT INTO history (id, seq, json) VALUES (?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET seq = ?2, json = ?3",
                )
                .map_err(err)?;
            for (id, row) in &next {
                if self.history.get(id) != Some(row) {
                    upsert.execute(params![*id as i64, row.0, row.1]).map_err(err)?;
                }
            }
            let mut delete = tx.prepare_cached("DELETE FROM history WHERE id = ?1").map_err(err)?;
            for id in self.history.keys().filter(|id| !next.contains_key(id)) {
                delete.execute(params![*id as i64]).map_err(err)?;
            }
        }
        tx.commit().map_err(err)?;
        self.history = next;
        // Marks the (possibly empty) history as saved.
        self.set_meta("history", &true)?;
        Ok(())
    }

    /// Copies the database next to itself when the last copy is old.
    fn backup_if_due(&mut self) {
        if self.last_backup.is_some_and(|t| t.elapsed() < BACKUP_EVERY) {
            return;
        }
        self.last_backup = Some(Instant::now());
        let _ = self.backup_now();
    }

    pub fn backup_now(&self) -> Result<(), Error> {
        let backup = backup_path(&self.path);
        let tmp = PathBuf::from(format!("{}.tmp", backup.display()));
        let _ = std::fs::remove_file(&tmp);
        self.conn.execute("VACUUM INTO ?1", params![tmp.to_string_lossy()]).map_err(err)?;
        crate::fsio::rename(&tmp, &backup)?;
        Ok(())
    }
}

fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", path.display()))
}

/// Reads a table without taking the data folder's lock (for read-only commands).
/// `None` when there is no database yet.
pub(crate) fn peek(dir: &Path) -> Option<Db> {
    let path = dir.join(FILE);
    if !path.is_file() {
        return None;
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.busy_timeout(Duration::from_secs(2)).ok()?;
    // A missing table means an unfinished database: nothing to show.
    let _: Option<i64> = conn.query_row("SELECT 1 FROM downloads LIMIT 1", [], |r| r.get(0)).optional().ok()?;
    Some(Db::load(conn, path))
}
