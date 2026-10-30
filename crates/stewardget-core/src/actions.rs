//! Actions that run on a file once it has downloaded: unpack archives,
//! scan for malware, move it somewhere, run a program, or tag it.
//!
//! Each [`Rule`] says which files it applies to and what to do with them. The
//! rules run in order on the finished file, in the background; one that moves
//! the file hands the new place to the rules after it. A scan that finds
//! something stops the chain, so a bad file isn't unpacked or opened.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Category;

/// What a rule does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Unpack a `.zip`, `.tar`, `.tar.gz` or `.tgz` next to the archive.
    Extract { delete_archive: bool },
    /// Move the file into a folder (created when missing). The folder may use
    /// `{category}`, `{year}`, `{month}`, `{day}` and `{host}`, so one rule can
    /// sort everything: `D:\Library\{category}\{year}-{month}`.
    MoveTo { dir: PathBuf },
    /// Rename the file. The template may use `{name}` (without extension),
    /// `{ext}`, `{host}`, `{year}`, `{month}` and `{day}`; the extension is kept
    /// when the template doesn't end in one.
    Rename { template: String },
    /// Scan with the system's antivirus (Microsoft Defender, or ClamAV).
    Scan,
    /// Run a program; `{path}`, `{dir}`, `{name}` and `{url}` are replaced.
    Command { command: String },
    /// Attach a tag to the file in the history.
    Tag { tag: String },
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Extract { .. } => "Unpack",
            Action::Rename { .. } => "Rename",
            Action::MoveTo { .. } => "Move",
            Action::Scan => "Scan for malware",
            Action::Command { .. } => "Run program",
            Action::Tag { .. } => "Tag",
        }
    }
}

/// One "when this file arrives, do that".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub name: String,
    pub enabled: bool,
    /// Only these kinds of file; empty = any.
    pub categories: Vec<Category>,
    /// Only these extensions (without the dot, lower case); empty = any.
    pub extensions: Vec<String>,
    /// Only links from these hosts (a leading `*.` matches subdomains); empty = any.
    pub hosts: Vec<String>,
    /// Only files at least this big (bytes); `0` = any.
    pub min_size: u64,
    pub action: Action,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            categories: Vec::new(),
            extensions: Vec::new(),
            hosts: Vec::new(),
            min_size: 0,
            action: Action::Scan,
        }
    }
}

impl Rule {
    /// Whether the rule applies to a finished file.
    pub fn applies(&self, file_name: &str, category: Category, host: &str, size: u64) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.categories.is_empty() && !self.categories.contains(&category) {
            return false;
        }
        if !self.extensions.is_empty() {
            let lower = file_name.to_lowercase();
            let hit = self.extensions.iter().any(|e| {
                let e = e.trim().trim_start_matches('.').to_lowercase();
                !e.is_empty() && lower.ends_with(&format!(".{e}"))
            });
            if !hit {
                return false;
            }
        }
        if !self.hosts.is_empty() && !self.hosts.iter().any(|h| host_matches(h, host)) {
            return false;
        }
        size >= self.min_size
    }
}

fn host_matches(pattern: &str, host: &str) -> bool {
    let (pattern, host) = (pattern.trim().to_lowercase(), host.to_lowercase());
    match pattern.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => host == pattern,
    }
}

/// What one rule did.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub rule: String,
    pub ok: bool,
    /// A short sentence for the history.
    pub message: String,
}

/// What a whole chain did to a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub outcomes: Vec<Outcome>,
    /// Where the file is now, if it moved or was replaced by its contents.
    pub new_path: Option<PathBuf>,
    pub tags: Vec<String>,
    /// A scan found something.
    pub threat: bool,
}

/// Runs the applicable rules on `path`. Blocking; call from a background thread.
pub fn run_rules(rules: &[Rule], path: &Path, url: &str, category: Category, size: u64) -> Report {
    let host = crate::host_of(url);
    let mut report = Report::default();
    let mut current = path.to_path_buf();
    for rule in rules {
        let name = current.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !rule.applies(&name, category, &host, size) {
            continue;
        }
        let label = if rule.name.trim().is_empty() { rule.action.label().to_owned() } else { rule.name.clone() };
        let result = match &rule.action {
            Action::Extract { delete_archive } => extract(&current, *delete_archive).map(|(dir, n)| {
                if *delete_archive {
                    report.new_path = Some(dir.clone());
                    current = dir.clone();
                }
                format!("Unpacked {n} file{} into {}", if n == 1 { "" } else { "s" }, dir.display())
            }),
            Action::Rename { template } => rename(&current, template, &host).map(|renamed| {
                let msg =
                    format!("Renamed to {}", renamed.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
                report.new_path = Some(renamed.clone());
                current = renamed;
                msg
            }),
            Action::MoveTo { dir } => move_to(&current, &expand_dir(dir, category, &host)).map(|moved| {
                let msg = format!("Moved to {}", moved.display());
                report.new_path = Some(moved.clone());
                current = moved;
                msg
            }),
            Action::Scan => match scan(&current) {
                Ok(Scan::Clean(by)) => Ok(format!("Scanned by {by}: no threats")),
                Ok(Scan::Threat(by)) => {
                    report.threat = true;
                    report.tags.push("threat".into());
                    report.outcomes.push(Outcome {
                        rule: label,
                        ok: false,
                        message: format!("{by} reported a threat. The file was left alone; later steps were skipped."),
                    });
                    return report;
                }
                Err(e) => Err(e),
            },
            Action::Command { command } => {
                crate::util::run_after_download(command, &current, url).map(|()| "Program finished".to_owned())
            }
            Action::Tag { tag } => {
                let tag = tag.trim().to_owned();
                if !tag.is_empty() && !report.tags.contains(&tag) {
                    report.tags.push(tag.clone());
                }
                Ok(format!("Tagged “{tag}”"))
            }
        };
        report.outcomes.push(match result {
            Ok(message) => Outcome { rule: label, ok: true, message },
            Err(e) => Outcome { rule: label, ok: false, message: e.to_string() },
        });
    }
    report
}

/// Most files and bytes one archive may unpack to; a guard against
/// "zip bombs" that fill the disk.
const MAX_FILES: usize = 200_000;
const MAX_BYTES: u64 = 64 << 30;

/// Unpacks `archive` into a folder named after it. Returns the folder and the
/// number of files written.
fn extract(archive: &Path, delete_archive: bool) -> io::Result<(PathBuf, usize)> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ext = ["tar.gz", "tgz", "tar", "zip"]
        .into_iter()
        .find(|e| name.ends_with(&format!(".{e}")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "not a .zip, .tar, .tar.gz or .tgz file"))?;
    let original = archive.file_name().unwrap().to_string_lossy().into_owned();
    // The extension is ASCII, so its length is the same in the original spelling.
    let folder_name = original[..original.len() - ext.len() - 1].to_owned();
    let parent = archive.parent().unwrap_or(Path::new("."));
    let mut dest = parent.join(&folder_name);
    let mut n = 1;
    while dest.exists() {
        n += 1;
        dest = parent.join(format!("{folder_name} ({n})"));
    }
    fs::create_dir_all(&dest)?;
    let count =
        if name.ends_with(".zip") { unzip(archive, &dest) } else { untar(archive, &dest, name.ends_with(".tar")) };
    match count {
        Ok(count) => {
            if delete_archive {
                fs::remove_file(archive)?;
            }
            Ok((dest, count))
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&dest); // never leave a half-unpacked folder
            Err(e)
        }
    }
}

fn unzip(archive: &Path, dest: &Path) -> io::Result<usize> {
    let mut zip = zip::ZipArchive::new(io::BufReader::new(fs::File::open(archive)?)).map_err(io::Error::other)?;
    if zip.len() > MAX_FILES {
        return Err(io::Error::other("the archive holds too many files"));
    }
    let mut total = 0u64;
    let mut files = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(io::Error::other)?;
        // Names with `..` or absolute paths would write outside the folder.
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(dir) = out.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = fs::File::create(&out)?;
        let budget = MAX_BYTES.saturating_sub(total);
        let written = io::copy(&mut (&mut entry).take(budget + 1), &mut file)?;
        total += written;
        if total > MAX_BYTES {
            return Err(io::Error::other("the archive unpacks to more than 64 GB"));
        }
        files += 1;
    }
    Ok(files)
}

fn untar(archive: &Path, dest: &Path, plain: bool) -> io::Result<usize> {
    let file = io::BufReader::new(fs::File::open(archive)?);
    let reader: Box<dyn Read> = if plain { Box::new(file) } else { Box::new(flate2::read::GzDecoder::new(file)) };
    let mut tar = tar::Archive::new(reader);
    let mut files = 0;
    let mut total = 0u64;
    for entry in tar.entries()? {
        let mut entry = entry?;
        total += entry.size();
        files += 1;
        if files > MAX_FILES || total > MAX_BYTES {
            return Err(io::Error::other("the archive is too large to unpack safely"));
        }
        // `unpack_in` refuses paths that would escape `dest`.
        entry.unpack_in(dest)?;
    }
    Ok(files)
}

/// Fills in `{category}`, `{year}`, `{month}`, `{day}` and `{host}` in a folder.
fn expand_dir(dir: &Path, category: Category, host: &str) -> PathBuf {
    let text = dir.to_string_lossy();
    if !text.contains('{') {
        return dir.to_path_buf();
    }
    PathBuf::from(fill(&text, &[("category", category.label()), ("host", host)]))
}

/// Replaces `{key}` placeholders; the date ones come from the clock. Values
/// are made safe as a single path part so a host or name can't climb out.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    let (y, m, d) = (now.year().to_string(), format!("{:02}", now.month()), format!("{:02}", now.day()));
    let mut out = template.to_owned();
    let all = values.iter().copied().chain([("year", y.as_str()), ("month", m.as_str()), ("day", d.as_str())]);
    for (key, value) in all {
        let safe: String =
            value.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
        out = out.replace(&format!("{{{key}}}"), safe.trim_matches('.'));
    }
    out
}

fn rename(path: &Path, template: &str, host: &str) -> io::Result<PathBuf> {
    let template = template.trim();
    if template.is_empty() {
        return Err(io::Error::other("the new name is empty"));
    }
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy().into_owned();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), e.to_owned()),
        _ => (name.clone(), String::new()),
    };
    let mut new = fill(template, &[("name", &stem), ("ext", &ext), ("host", host)]);
    new = new.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    // Keep the extension unless the template brings its own.
    if !ext.is_empty()
        && !template.contains("{ext}")
        && !new.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase()))
    {
        new = format!("{new}.{ext}");
    }
    let new = new.trim().to_owned();
    if new.is_empty() || new == name {
        return Ok(path.to_path_buf());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let target = unique(dir, &new);
    fs::rename(path, &target)?;
    Ok(target)
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let mut target = dir.join(name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = dir.join(format!("{stem} ({n}){ext}"));
    }
    target
}

fn move_to(path: &Path, dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy().into_owned();
    let mut target = dir.join(&name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = dir.join(format!("{stem} ({n}){ext}"));
    }
    // Copies across drives, but leaves the file alone when it is merely in use.
    crate::fsio::move_file(path, &target)?;
    Ok(target)
}

enum Scan {
    Clean(&'static str),
    Threat(&'static str),
}

fn scan(path: &Path) -> io::Result<Scan> {
    #[cfg(windows)]
    {
        if let Some(exe) = defender() {
            let status = run_quiet(
                std::process::Command::new(exe)
                    .args(["-Scan", "-ScanType", "3", "-DisableRemediation", "-File"])
                    .arg(path),
            )?;
            // 0 = clean, 2 = something found.
            return match status {
                0 => Ok(Scan::Clean("Microsoft Defender")),
                2 => Ok(Scan::Threat("Microsoft Defender")),
                code => Err(io::Error::other(format!("Microsoft Defender could not scan the file (code {code})"))),
            };
        }
    }
    for (program, name) in [("clamdscan", "ClamAV"), ("clamscan", "ClamAV")] {
        match run_quiet(std::process::Command::new(program).arg("--no-summary").arg(path)) {
            Ok(0) => return Ok(Scan::Clean(name)),
            Ok(1) => return Ok(Scan::Threat(name)),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no antivirus scanner found (Microsoft Defender or ClamAV)"))
}

/// Microsoft Defender's command-line scanner: the newest platform folder first.
#[cfg(windows)]
fn defender() -> Option<PathBuf> {
    let data = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let platform = data.join(r"Microsoft\Windows Defender\Platform");
    let mut versions: Vec<PathBuf> = fs::read_dir(platform).ok()?.flatten().map(|e| e.path()).collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .map(|v| v.join("MpCmdRun.exe"))
        .chain(std::iter::once(
            PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into()))
                .join(r"Windows Defender\MpCmdRun.exe"),
        ))
        .find(|p| p.exists())
}

fn run_quiet(cmd: &mut std::process::Command) -> io::Result<i32> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code().unwrap_or(-1));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the scan took too long"));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stewardget-actions-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_zip(path: &Path) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("a/hello.txt", opts).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.start_file("../escape.txt", opts).unwrap();
        zip.write_all(b"nope").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn rules_match_on_kind_extension_host_and_size() {
        let rule = Rule {
            categories: vec![Category::Archive],
            extensions: vec![".ZIP".into()],
            hosts: vec!["*.example.com".into()],
            min_size: 10,
            ..Default::default()
        };
        assert!(rule.applies("a.zip", Category::Archive, "cdn.example.com", 10));
        assert!(rule.applies("a.zip", Category::Archive, "example.com", 99));
        assert!(!rule.applies("a.zip", Category::Video, "example.com", 99));
        assert!(!rule.applies("a.rar", Category::Archive, "example.com", 99));
        assert!(!rule.applies("a.zip", Category::Archive, "evil-example.com", 99));
        assert!(!rule.applies("a.zip", Category::Archive, "example.com", 9));
        assert!(!Rule { enabled: false, ..Default::default() }.applies("a", Category::Other, "", 0));
    }

    #[test]
    fn zip_unpacks_safely_and_can_remove_the_archive() {
        let dir = temp("zip");
        let archive = dir.join("pack.zip");
        make_zip(&archive);
        let rule = Rule { action: Action::Extract { delete_archive: true }, ..Default::default() };
        let report = run_rules(&[rule], &archive, "https://x.test/pack.zip", Category::Archive, 1);
        assert!(report.outcomes[0].ok, "{:?}", report.outcomes);
        assert_eq!(fs::read_to_string(dir.join("pack/a/hello.txt")).unwrap(), "hello");
        assert!(!dir.join("escape.txt").exists(), "paths with .. must not escape");
        assert!(!archive.exists());
        assert_eq!(report.new_path, Some(dir.join("pack")));
    }

    #[test]
    fn moving_keeps_names_unique_and_later_rules_follow_the_file() {
        let dir = temp("move");
        let file = dir.join("song.mp3");
        fs::write(&file, b"x").unwrap();
        let dest = dir.join("music");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("song.mp3"), b"old").unwrap();
        let rules = vec![
            Rule { action: Action::MoveTo { dir: dest.clone() }, ..Default::default() },
            Rule { action: Action::Tag { tag: "music".into() }, extensions: vec!["mp3".into()], ..Default::default() },
        ];
        let report = run_rules(&rules, &file, "https://x.test/song.mp3", Category::Audio, 1);
        assert_eq!(report.new_path, Some(dest.join("song (2).mp3")));
        assert!(!file.exists());
        assert_eq!(fs::read(dest.join("song.mp3")).unwrap(), b"old");
        assert_eq!(report.tags, vec!["music".to_owned()]);
        assert!(report.outcomes.iter().all(|o| o.ok));
    }

    #[test]
    fn unsupported_archives_report_an_error_without_leaving_folders() {
        let dir = temp("rar");
        let file = dir.join("x.rar");
        fs::write(&file, b"x").unwrap();
        let rule = Rule { action: Action::Extract { delete_archive: false }, ..Default::default() };
        let report = run_rules(&[rule], &file, "https://x.test/x.rar", Category::Archive, 1);
        assert!(!report.outcomes[0].ok);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn folders_and_names_can_use_placeholders_safely() {
        let dir = temp("tpl");
        let file = dir.join("Report.PDF");
        fs::write(&file, b"x").unwrap();
        let rules = vec![
            Rule { action: Action::Rename { template: "{host}-{name}".into() }, ..Default::default() },
            Rule { action: Action::MoveTo { dir: dir.join("{category}").join("{year}") }, ..Default::default() },
        ];
        let report = run_rules(&rules, &file, "https://Files.Example.com/Report.PDF", Category::Document, 1);
        assert!(report.outcomes.iter().all(|o| o.ok), "{:?}", report.outcomes);
        let year = chrono::Local::now().format("%Y").to_string();
        assert_eq!(
            report.new_path,
            Some(dir.join(Category::Document.label()).join(year).join("files.example.com-Report.PDF"))
        );
        assert!(report.new_path.unwrap().exists());
    }

    #[test]
    fn placeholder_values_cannot_climb_out_of_the_folder() {
        assert_eq!(fill("{host}", &[("host", "../../etc")]), "etc");
        assert_eq!(fill("{name}", &[("name", "a/b\\c")]), "abc");
    }
}
