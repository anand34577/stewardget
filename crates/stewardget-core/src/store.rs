//! On-disk persistence: settings in a small JSON file written atomically, and
//! the download list, history and statistics in a SQLite database ([`crate::db`]).
//! Lists saved as JSON by earlier versions are moved into the database once.
//! An exclusive lock file prevents two front-ends from fighting over the same state.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::util::LockExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::db::{self, Db};
use crate::{Category, Config, DownloadOptions, Error, History, QueueId, Stats, Status, fsio};

const SETTINGS: &str = "settings.json";
const DOWNLOADS: &str = "downloads.json";
const STATS: &str = "stats.json";
const HISTORY: &str = "history.json";
const LOCK: &str = "stewardget.lock";
/// The lock file of Velox, as the app was called before 0.7.
const LEGACY_LOCK: &str = "velox.lock";

/// The persisted part of a download.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Record {
    pub id: u64,
    pub url: String,
    #[serde(default)]
    pub options: DownloadOptions,
    pub file_name: String,
    pub path: PathBuf,
    /// The path was chosen after probing and must not change anymore,
    /// otherwise the partial file would be orphaned.
    #[serde(default)]
    pub path_locked: bool,
    #[serde(default)]
    pub category: Category,
    /// Queue it belongs to; the main queue for lists written before queues existed.
    #[serde(default)]
    pub queue: QueueId,
    pub status: Status,
    pub total: Option<u64>,
    #[serde(default)]
    pub downloaded: u64,
    #[serde(default)]
    pub resumable: bool,
    pub error: Option<String>,
    pub added_at: u64,
    pub finished_at: Option<u64>,
    #[serde(default)]
    pub checksum_verified: bool,
    /// Waiting for a fresh link from the browser.
    #[serde(default)]
    pub awaiting_link: bool,
    /// The link was replaced by the user: resume as long as the size matches,
    /// even if the new server reports different validators.
    #[serde(default)]
    pub relaxed_resume: bool,
    /// Seconds spent transferring so far.
    #[serde(default)]
    pub active_secs: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct DownloadList {
    pub next_id: u64,
    pub downloads: Vec<Record>,
}

pub(crate) struct Store {
    dir: Option<PathBuf>,
    _lock: Option<File>,
    _legacy_lock: Option<File>,
    /// What went wrong reading a state file, for the user to be told.
    notice: std::sync::Mutex<Option<String>>,
    db: Option<std::sync::Mutex<Db>>,
}

impl Store {
    /// Opens (creating if needed) a data directory and takes its lock.
    pub fn open(dir: PathBuf) -> Result<Self, Error> {
        fsio::create_private_dir(&dir)?;
        let lock = File::options().create(true).truncate(false).write(true).open(dir.join(LOCK))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(Error::Locked(dir.display().to_string())),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        // A Velox still running on its old folder, which is then used as is (see `adopt_legacy_dir`).
        let legacy = File::options().write(true).open(dir.join(LEGACY_LOCK)).ok();
        if let Some(legacy) = &legacy {
            match legacy.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Err(Error::Locked(dir.display().to_string())),
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
        let opened = Db::open(&dir)?;
        Ok(Self {
            dir: Some(dir),
            _lock: Some(lock),
            _legacy_lock: legacy,
            notice: std::sync::Mutex::new(opened.notice),
            db: Some(std::sync::Mutex::new(opened.db)),
        })
    }

    /// A store that never touches the disk (used by one-shot CLI downloads).
    pub fn memory() -> Self {
        Self { dir: None, _lock: None, _legacy_lock: None, notice: Default::default(), db: None }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn load_config(&self) -> Option<Config> {
        self.read(SETTINGS)
    }

    pub fn save_config(&self, config: &Config) -> Result<(), Error> {
        self.write(SETTINGS, config)
    }

    pub fn load_downloads(&self) -> Option<DownloadList> {
        let db = self.db.as_ref()?;
        {
            let db = db.lock_ok();
            if db.has_data("downloads") {
                return Some(DownloadList { next_id: db.meta("next_id").unwrap_or(0), downloads: db.download_rows() });
            }
        }
        let legacy: DownloadList = self.read(DOWNLOADS)?;
        if self.save_downloads(&legacy).is_ok() {
            self.retire(DOWNLOADS);
        }
        Some(legacy)
    }

    pub fn save_downloads(&self, list: &DownloadList) -> Result<(), Error> {
        let Some(db) = &self.db else { return Ok(()) };
        let rows: Vec<(u64, &Record)> = list.downloads.iter().map(|r| (r.id, r)).collect();
        let mut db = db.lock_ok();
        db.save_downloads(&rows)?;
        db.set_meta("next_id", &list.next_id)
    }

    pub fn load_stats(&self) -> Option<Stats> {
        let db = self.db.as_ref()?;
        if let Some(stats) = db.lock_ok().meta("stats") {
            return Some(stats);
        }
        let legacy: Stats = self.read(STATS)?;
        if self.save_stats(&legacy).is_ok() {
            self.retire(STATS);
        }
        Some(legacy)
    }

    pub fn save_stats(&self, stats: &Stats) -> Result<(), Error> {
        let Some(db) = &self.db else { return Ok(()) };
        db.lock_ok().set_meta("stats", stats)
    }

    pub fn load_history(&self) -> Option<History> {
        let db = self.db.as_ref()?;
        {
            let db = db.lock_ok();
            if db.has_data("history") {
                return Some(History { entries: db.history_rows() });
            }
        }
        let legacy: History = self.read(HISTORY)?;
        if self.save_history(&legacy).is_ok() {
            self.retire(HISTORY);
        }
        Some(legacy)
    }

    pub fn save_history(&self, history: &History) -> Result<(), Error> {
        let Some(db) = &self.db else { return Ok(()) };
        let rows: Vec<(u64, &crate::HistoryEntry)> = history.entries.iter().map(|e| (e.id, e)).collect();
        db.lock_ok().save_history(&rows)
    }

    /// Sets a JSON file aside once its contents live in the database.
    fn retire(&self, name: &str) {
        if let Some(dir) = &self.dir {
            let path = dir.join(name);
            let mut moved = path.as_os_str().to_owned();
            moved.push(".migrated");
            let _ = std::fs::rename(&path, moved);
        }
    }

    /// Reads a state file. A damaged file is set aside (never overwritten)
    /// and the most recent backup is used instead.
    fn read<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let dir = self.dir.as_ref()?;
        let path = dir.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => return Some(value),
                Err(_) => {
                    let aside = dir.join(format!("{name}.corrupt-{}", crate::util::now_unix()));
                    let _ = std::fs::rename(&path, &aside);
                    let restored = std::fs::read(backup_path(&path)).ok().and_then(|b| serde_json::from_slice(&b).ok());
                    *self.notice.lock_ok() = Some(if restored.is_some() {
                        format!(
                            "{name} was damaged and was restored from the latest backup; the damaged file is kept as {}",
                            aside.display()
                        )
                    } else {
                        format!(
                            "{name} was damaged and no usable backup exists; the damaged file is kept as {}",
                            aside.display()
                        )
                    });
                    return restored;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        serde_json::from_slice(&std::fs::read(backup_path(&path)).ok()?).ok()
    }

    /// A problem found while loading the state files, if any.
    pub fn notice(&self) -> Option<String> {
        self.notice.lock_ok().clone()
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), Error> {
        let Some(dir) = &self.dir else { return Ok(()) };
        // Settings stay readable for hand edits; the lists grow with the library, so they are compact.
        let json = if name == SETTINGS { serde_json::to_vec_pretty(value) } else { serde_json::to_vec(value) }
            .map_err(|e| Error::Io(e.to_string()))?;
        let path = dir.join(name);
        // Keep a rolling backup, refreshed at most once a minute.
        let backup = backup_path(&path);
        let stale = std::fs::metadata(&backup)
            .and_then(|m| m.modified())
            .map_or(true, |t| t.elapsed().unwrap_or_default() > std::time::Duration::from_secs(60));
        if stale && path.exists() {
            let _ = std::fs::copy(&path, &backup);
        }
        fsio::write_atomic(&path, &json)?;
        Ok(())
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

/// Reads a data directory without locking it (for read-only CLI commands).
pub(crate) fn peek<T: DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    serde_json::from_slice(&std::fs::read(dir.join(name)).ok()?).ok()
}

pub(crate) fn peek_downloads(dir: &Path) -> Option<DownloadList> {
    if let Some(db) = db::peek(dir).filter(|db| db.has_data("downloads")) {
        return Some(DownloadList { next_id: db.meta("next_id").unwrap_or(0), downloads: db.download_rows() });
    }
    peek(dir, DOWNLOADS)
}

pub(crate) fn peek_stats(dir: &Path) -> Option<Stats> {
    db::peek(dir).and_then(|db| db.meta("stats")).or_else(|| peek(dir, STATS))
}

pub(crate) fn peek_history(dir: &Path) -> Option<History> {
    if let Some(db) = db::peek(dir).filter(|db| db.has_data("history")) {
        return Some(History { entries: db.history_rows() });
    }
    peek(dir, HISTORY)
}

pub(crate) fn peek_config(dir: &Path) -> Option<Config> {
    peek(dir, SETTINGS)
}

/// Moves the data folder of Velox, as the app was called before 0.7, to `new`
/// and returns the folder to use. Nothing moves once `new` exists, or while a
/// Velox still runs on the old folder: that folder is used meanwhile, and
/// [`Store::open`] keeps the two apps from sharing it.
pub(crate) fn adopt_legacy_dir(legacy: &Path, new: &Path) -> PathBuf {
    if new.exists() || !legacy.is_dir() {
        return new.to_path_buf();
    }
    let running = File::options()
        .write(true)
        .open(legacy.join(LEGACY_LOCK))
        .is_ok_and(|lock| matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    let moved = !running
        && new.parent().is_none_or(|parent| std::fs::create_dir_all(parent).is_ok())
        && std::fs::rename(legacy, new).is_ok();
    if !moved {
        return legacy.to_path_buf();
    }
    let _ = std::fs::remove_file(new.join(LEGACY_LOCK));
    // Windows keeps it in `velox\Velox\data`: drop the folders left empty.
    for dir in legacy.ancestors().skip(1).take(2) {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
    }
    // A browser may have loaded the extension from the old folder.
    if new.join(crate::extension::FOLDER).is_dir() {
        crate::extension::keep_copy(&legacy.join(crate::extension::FOLDER));
    }
    new.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velox_data_moves_to_the_new_folder_once() {
        let root = std::env::temp_dir().join(format!("stewardget-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (legacy, new) = (root.join("velox/Velox/data"), root.join("stewardget/StewardGet/data"));
        std::fs::create_dir_all(legacy.join(crate::extension::FOLDER)).unwrap();
        std::fs::write(legacy.join(SETTINGS), b"{}").unwrap();
        std::fs::write(legacy.join(LEGACY_LOCK), b"").unwrap();

        // Not while a Velox holds its lock: the old folder stays in use, and shared with nobody.
        let velox = File::options().write(true).open(legacy.join(LEGACY_LOCK)).unwrap();
        velox.lock().unwrap();
        assert_eq!(adopt_legacy_dir(&legacy, &new), legacy);
        assert!(matches!(Store::open(legacy.clone()), Err(Error::Locked(_))));
        drop(velox);

        assert_eq!(adopt_legacy_dir(&legacy, &new), new);
        assert!(new.join(SETTINGS).is_file() && !new.join(LEGACY_LOCK).exists());
        assert!(!root.join("velox/Velox/data").join(SETTINGS).exists());
        // The extension stays loadable from where the browser found it.
        assert!(legacy.join(crate::extension::FOLDER).join("manifest.json").is_file());
        // Later starts leave both folders alone.
        assert_eq!(adopt_legacy_dir(&legacy, &new), new);
        assert!(new.join(SETTINGS).is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn damaged_database_recovers_from_backup() {
        let dir = std::env::temp_dir().join(format!("stewardget-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(dir.clone()).unwrap();
        let list = DownloadList { next_id: 7, downloads: Vec::new() };
        store.save_downloads(&list).unwrap();
        store.db.as_ref().unwrap().lock_ok().backup_now().unwrap();
        drop(store);
        std::fs::write(dir.join(db::FILE), b"this is not a database").unwrap();
        let _ = std::fs::remove_file(dir.join(format!("{}-wal", db::FILE)));
        let store = Store::open(dir.clone()).unwrap();
        assert_eq!(store.load_downloads().unwrap().next_id, 7);
        let aside =
            std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".corrupt-"));
        assert!(aside, "the damaged file is kept for inspection");
        assert!(store.notice().is_some_and(|n| n.contains("restored")), "the user is told");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_lists_move_into_the_database_once() {
        let dir = std::env::temp_dir().join(format!("stewardget-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(DOWNLOADS), br#"{"next_id":42,"downloads":[]}"#).unwrap();
        let store = Store::open(dir.clone()).unwrap();
        assert_eq!(store.load_downloads().unwrap().next_id, 42);
        assert!(!dir.join(DOWNLOADS).exists() && dir.join("downloads.json.migrated").exists());
        drop(store);
        let store = Store::open(dir.clone()).unwrap();
        assert_eq!(store.load_downloads().unwrap().next_id, 42, "read from the database now");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_keeps_its_order_and_only_changes_are_written() {
        let dir = std::env::temp_dir().join(format!("stewardget-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(dir.clone()).unwrap();
        let entry = |id: u64| crate::HistoryEntry {
            id,
            url: format!("http://x/{id}"),
            host: "x".into(),
            file_name: format!("{id}.bin"),
            path: PathBuf::from(format!("/{id}.bin")),
            category: Category::Other,
            size: Some(id),
            status: Status::Completed,
            error: None,
            added_at: 0,
            finished_at: 0,
            active_secs: 0,
            connections: 1,
            checksum_verified: false,
            tags: Vec::new(),
            notes: Vec::new(),
        };
        let mut history = History { entries: vec![entry(3), entry(1), entry(2)] };
        store.save_history(&history).unwrap();
        history.entries.remove(1);
        history.entries.push(entry(9));
        store.save_history(&history).unwrap();
        let ids: Vec<u64> = store.load_history().unwrap().entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, [3, 2, 9]);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
