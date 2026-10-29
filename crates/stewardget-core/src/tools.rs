//! Housekeeping tools: duplicate files, leftovers of unfinished downloads,
//! settings backups and a diagnostics report.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::{Config, Error, HashAlgo, hash_file};

/// Files that are byte-for-byte the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupGroup {
    pub size: u64,
    pub files: Vec<PathBuf>,
}

impl DupGroup {
    /// Space freed by keeping one copy.
    pub fn wasted(&self) -> u64 {
        self.size * (self.files.len() as u64).saturating_sub(1)
    }
}

const MAX_FILES: usize = 400_000;
const MAX_DEPTH: usize = 12;

fn is_partial(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("vxpart") || e.eq_ignore_ascii_case("vxmeta"))
}

/// Every regular file below `dirs` (symbolic links are not followed).
fn walk(dirs: &[PathBuf], cancel: &AtomicBool, mut visit: impl FnMut(&Path, &fs::Metadata)) {
    let mut count = 0;
    let mut stack: Vec<(PathBuf, usize)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    let mut seen = HashSet::new();
    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || count >= MAX_FILES || !seen.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() && depth < MAX_DEPTH {
                stack.push((entry.path(), depth + 1));
            } else if meta.is_file() {
                count += 1;
                visit(&entry.path(), &meta);
            }
        }
    }
}

fn prefix_hash(path: &Path) -> io::Result<[u8; 32]> {
    let mut buf = vec![0u8; 64 * 1024];
    let n = fs::File::open(path)?.read(&mut buf)?;
    Ok(Sha256::digest(&buf[..n]).into())
}

/// Groups identical files of at least `min_size` bytes. Sizes are compared
/// first, then the first 64 KB, then the whole file, so only real candidates
/// are read in full. Blocking; stops early when `cancel` is set.
pub fn find_duplicates(dirs: &[PathBuf], min_size: u64, cancel: &AtomicBool) -> Vec<DupGroup> {
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    walk(dirs, cancel, |path, meta| {
        if meta.len() >= min_size.max(1) && !is_partial(path) {
            by_size.entry(meta.len()).or_default().push(path.to_path_buf());
        }
    });
    let mut groups = Vec::new();
    for (size, files) in by_size.into_iter().filter(|(_, f)| f.len() > 1) {
        let mut by_prefix: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
        for f in files {
            if cancel.load(Ordering::Relaxed) {
                return groups;
            }
            if let Ok(h) = prefix_hash(&f) {
                by_prefix.entry(h).or_default().push(f);
            }
        }
        for candidates in by_prefix.into_values().filter(|c| c.len() > 1) {
            let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
            for f in candidates {
                if cancel.load(Ordering::Relaxed) {
                    return groups;
                }
                if let Ok(h) = hash_file(HashAlgo::Sha256, &f) {
                    by_hash.entry(h).or_default().push(f);
                }
            }
            for mut same in by_hash.into_values().filter(|s| s.len() > 1) {
                same.sort();
                groups.push(DupGroup { size, files: same });
            }
        }
    }
    groups.sort_by(|a, b| b.wasted().cmp(&a.wasted()).then_with(|| a.files.cmp(&b.files)));
    groups
}

/// A `.vxpart` / `.vxmeta` file no download in the list owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub path: PathBuf,
    pub size: u64,
}

/// Partial-download files below `dirs` that are not in `owned`.
pub fn find_leftovers(dirs: &[PathBuf], owned: &HashSet<PathBuf>) -> Vec<Leftover> {
    let never = AtomicBool::new(false);
    let mut found = Vec::new();
    walk(dirs, &never, |path, meta| {
        if is_partial(path) && !owned.contains(path) {
            found.push(Leftover { path: path.to_path_buf(), size: meta.len() });
        }
    });
    found.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    found
}

/// Deletes leftovers; only partial-download files are ever removed.
pub fn delete_leftovers(paths: &[PathBuf]) -> usize {
    paths.iter().filter(|p| is_partial(p) && fs::remove_file(p).is_ok()).count()
}

/// Settings without the secrets (access tokens), as JSON.
pub fn shareable_settings(config: &Config) -> Result<String, Error> {
    let mut c = config.clone();
    c.api_token = None;
    c.mcp_token = None;
    c.control_token = None;
    serde_json::to_string_pretty(&c).map_err(|e| Error::Io(e.to_string()))
}

/// Writes a backup zip: settings (no tokens), history, statistics and the download list.
pub fn export_backup(data_dir: &Path, config: &Config, dest: &Path) -> Result<(), Error> {
    let file = fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let io = |e: zip::result::ZipError| Error::Io(e.to_string());
    zip.start_file("settings.json", opts).map_err(io)?;
    zip.write_all(shareable_settings(config)?.as_bytes())?;
    for name in ["history.json", "stats.json", "downloads.json"] {
        if let Ok(bytes) = fs::read(data_dir.join(name)) {
            zip.start_file(name, opts).map_err(io)?;
            zip.write_all(&bytes)?;
        }
    }
    zip.finish().map_err(io)?;
    Ok(())
}

/// The settings inside a backup zip. Tokens and the local-only fields stay as
/// they are on this computer when the caller applies them.
pub fn read_backup_settings(src: &Path) -> Result<Config, Error> {
    let mut zip = zip::ZipArchive::new(io::BufReader::new(fs::File::open(src)?))
        .map_err(|_| Error::Unsupported("that file is not a StewardGet backup".into()))?;
    let mut entry =
        zip.by_name("settings.json").map_err(|_| Error::Unsupported("that backup has no settings in it".into()))?;
    let mut text = String::new();
    (&mut entry).take(4 << 20).read_to_string(&mut text)?;
    serde_json::from_str(&text)
        .map_err(|e| Error::Unsupported(format!("the settings in the backup are unreadable: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stewardget-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn identical_files_are_grouped_and_lookalikes_are_not() {
        let d = temp("dup");
        fs::create_dir_all(d.join("sub")).unwrap();
        let big = vec![7u8; 200_000];
        fs::write(d.join("a.bin"), &big).unwrap();
        fs::write(d.join("sub/copy of a.bin"), &big).unwrap();
        // Same size and same first 64 KB, different ending.
        let mut almost = big.clone();
        *almost.last_mut().unwrap() = 8;
        fs::write(d.join("almost.bin"), &almost).unwrap();
        fs::write(d.join("tiny1"), b"x").unwrap();
        fs::write(d.join("tiny2"), b"x").unwrap();
        let groups = find_duplicates(std::slice::from_ref(&d), 1024, &AtomicBool::new(false));
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].files.len(), 2);
        assert_eq!(groups[0].wasted(), 200_000);
        assert!(find_duplicates(&[d], 1, &AtomicBool::new(false)).len() == 2, "small files count when asked");
    }

    #[test]
    fn only_unowned_partial_files_are_leftovers_and_only_those_are_deleted() {
        let d = temp("left");
        for n in ["keep.zip.vxpart", "keep.zip.vxmeta", "old.iso.vxpart", "real.txt"] {
            fs::write(d.join(n), b"data").unwrap();
        }
        let owned: HashSet<PathBuf> = [d.join("keep.zip.vxpart"), d.join("keep.zip.vxmeta")].into();
        let found = find_leftovers(std::slice::from_ref(&d), &owned);
        assert_eq!(
            found.iter().map(|l| l.path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(),
            ["old.iso.vxpart"]
        );
        assert_eq!(delete_leftovers(&[d.join("real.txt"), d.join("old.iso.vxpart")]), 1);
        assert!(d.join("real.txt").exists() && !d.join("old.iso.vxpart").exists());
    }

    #[test]
    fn backups_round_trip_without_secrets() {
        let d = temp("backup");
        fs::write(d.join("history.json"), b"{}").unwrap();
        let config = Config { api_token: Some("secret-token-value".into()), connections: 12, ..Config::default() };
        let zip_path = d.join("b.zip");
        export_backup(&d, &config, &zip_path).unwrap();
        let back = read_backup_settings(&zip_path).unwrap();
        assert_eq!(back.connections, 12);
        assert_eq!(back.api_token, None);
        assert!(read_backup_settings(&d.join("history.json")).is_err());
    }
}
