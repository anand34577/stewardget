//! Small, dependency-free helpers shared by every front-end.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/// Formats a byte count, e.g. `1.46 GiB`.
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let precision = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.precision$} {}", UNITS[unit])
}

/// Formats a transfer rate, e.g. `12.4 MiB/s`.
pub fn format_speed(bytes_per_sec: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec.max(0.0) as u64))
}

/// Formats a duration compactly, e.g. `1h 04m`, `3m 07s`, `12s`.
pub fn format_duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match (d, h, m) {
        (d, _, _) if d > 0 => format!("{d}d {h:02}h"),
        (_, h, _) if h > 0 => format!("{h}h {m:02}m"),
        (_, _, m) if m > 0 => format!("{m}m {s:02}s"),
        _ => format!("{s}s"),
    }
}

/// Parses human sizes such as `500k`, `2M`, `1.5MiB`, `10mb/s` or `0`.
/// Units are binary (1k = 1024). Returns `None` on malformed input.
pub fn parse_size(input: &str) -> Option<u64> {
    let s = input.trim().to_ascii_lowercase();
    let s = s.strip_suffix("/s").unwrap_or(&s).trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num.parse().ok()?;
    let unit = unit.trim().trim_end_matches("ib").trim_end_matches('b');
    let mult = match unit {
        "" => 1u64,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        "t" => 1 << 40,
        _ => return None,
    };
    (value.is_finite() && value >= 0.0).then(|| (value * mult as f64).round() as u64)
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The local calendar day of a Unix timestamp, as days since 1970-01-01.
pub fn day_index(unix_secs: u64) -> i64 {
    (unix_secs as i64 + local_offset_secs(unix_secs)).div_euclid(86_400)
}

/// The local time zone's offset from UTC at that instant, in seconds.
pub fn local_offset_secs(unix_secs: u64) -> i64 {
    use chrono::{Offset, TimeZone};
    chrono::Local
        .timestamp_opt(unix_secs as i64, 0)
        .single()
        .map_or(0, |t| i64::from(t.offset().fix().local_minus_utc()))
}

/// Converts days since the Unix epoch into a `(year, month, day)` civil date.
/// Howard Hinnant's algorithm; valid for the whole proleptic Gregorian range.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Formats a Unix timestamp as `YYYY-MM-DD HH:MM` in local time.
pub fn format_timestamp(unix_secs: u64) -> String {
    let local = unix_secs as i64 + local_offset_secs(unix_secs);
    let (y, mo, d) = civil_from_days(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    format!("{y:04}-{mo:02}-{d:02} {:02}:{:02}", secs / 3600, secs / 60 % 60)
}

/// How long ago `unix_secs` was, for lists: `just now`, `5 min ago`,
/// `3 h ago`, `Yesterday`, `4 days ago`, then the date.
pub fn format_relative(unix_secs: u64, now: u64) -> String {
    let ago = now.saturating_sub(unix_secs);
    let days = day_index(now) - day_index(unix_secs);
    match ago {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", ago / 60),
        _ if ago < 6 * 3600 || days == 0 => format!("{} h ago", ago / 3600),
        _ if days == 1 => "Yesterday".into(),
        _ if days < 7 => format!("{days} days ago"),
        _ => format_timestamp(unix_secs).split(' ').next().unwrap_or_default().to_owned(),
    }
}

/// Opens the system file manager with `path` selected (or its folder opened).
pub fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let exists = path.exists();
    // A file that is gone: open the closest folder that is still there.
    let folder = path.ancestors().skip(1).find(|p| p.is_dir()).unwrap_or(&path).to_path_buf();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        if exists {
            // Explorer wants `/select,"C: bile"` with the quotes around the path only; a
            // normal argument would be quoted as a whole, and Explorer would open Documents.
            Command::new("explorer").raw_arg(explorer_select_arg(&path)).spawn()?;
        } else {
            Command::new("explorer").arg(windows_path(&folder)).spawn()?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if exists {
            Command::new("open").arg("-R").arg(&path).spawn()?;
        } else {
            Command::new("open").arg(&folder).spawn()?;
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() { path.clone() } else { folder };
        let _ = exists;
        Command::new("xdg-open").arg(dir).spawn()?;
    }
    Ok(())
}

/// `C: bile` with backslashes only, which is all Explorer understands.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_path(path: &Path) -> String {
    path.display().to_string().replace('/', "\\")
}

/// The command line Explorer needs to open a folder with `path` selected.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn explorer_select_arg(path: &Path) -> String {
    format!("/select,\"{}\"", windows_path(path))
}

/// Opens a file with the system's default application.
pub fn open_with_system(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    shell_open(path.as_os_str())?;
    #[cfg(not(target_os = "windows"))]
    use std::process::Command;
    #[cfg(target_os = "macos")]
    Command::new("open").arg(path).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

/// Opens a URL in the default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    shell_open(std::ffi::OsStr::new(url))?;
    #[cfg(not(target_os = "windows"))]
    use std::process::Command;
    #[cfg(target_os = "macos")]
    Command::new("open").arg(url).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}

/// `ShellExecuteW(open)`: no console window, and no shell parsing of `&` or
/// `%` in file names.
#[cfg(target_os = "windows")]
fn shell_open(target: &std::ffi::OsStr) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target: Vec<u16> = target.encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    // Values above 32 mean success.
    if code > 32 { Ok(()) } else { Err(std::io::Error::other(format!("Windows could not open it (code {code})"))) }
}

/// Cheap, non-cryptographic jitter in `0..max` (used for retry back-off).
/// Whether a file name hides a program behind a harmless-looking extension,
/// like `invoice.pdf.exe` or `photo.jpg.scr`. Real documents don't have two.
pub fn disguised_program(name: &str) -> bool {
    const PROGRAMS: [&str; 17] = [
        "exe", "scr", "com", "pif", "bat", "cmd", "msi", "ps1", "vbs", "vbe", "js", "jse", "wsf", "hta", "jar", "lnk",
        "apk",
    ];
    const HARMLESS: [&str; 22] = [
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "jpg", "jpeg", "png", "gif", "bmp", "mp3",
        "mp4", "avi", "mkv", "zip", "rar", "csv", "odt",
    ];
    let lower = name.trim().to_lowercase();
    let mut parts = lower.rsplit('.');
    let (Some(last), Some(before)) = (parts.next(), parts.next()) else { return false };
    // Spaces before the real extension are another favourite: `report.pdf      .exe`.
    PROGRAMS.contains(&last) && HARMLESS.contains(&before.trim())
}

/// 32 hex characters from the operating system's random number generator,
/// for access tokens.
pub fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No system randomness (practically never): fall back to clock and hasher seeds.
        use std::hash::{BuildHasher, Hasher, RandomState};
        for chunk in bytes.chunks_mut(8) {
            let mut h = RandomState::new().build_hasher();
            h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares secrets without stopping at the first difference.
pub fn secrets_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

pub(crate) fn jitter_ms(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    // xorshift to spread out the low-entropy clock bits
    let mut x = u64::from(nanos) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x % max
}

/// Free space on the drive that holds `dir` (or its nearest existing parent).
pub fn free_space(dir: &Path) -> Option<u64> {
    dir.ancestors().find(|d| d.exists()).and_then(crate::fsio::free_space)
}

/// Splits pasted text into links: one per line, or several on a line when
/// each starts with `http://`, `https://`, `ftp://` or `magnet:`. Words after a link on the same
/// line belong to it, so `https://x/Firefox Setup.exe` stays one link (with
/// its spaces encoded) instead of becoming three.
pub fn split_links(text: &str) -> Vec<String> {
    let has_scheme = |t: &str| {
        let lower = t.get(..8).unwrap_or(t).to_ascii_lowercase();
        lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("ftp://")
            || lower.starts_with("magnet:")
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if !tokens.iter().any(|t| has_scheme(t)) {
            out.extend(tokens.into_iter().map(str::to_owned));
            continue;
        }
        let mut current: Option<String> = None;
        for token in tokens {
            match &mut current {
                Some(link) if !has_scheme(token) => {
                    link.push_str("%20");
                    link.push_str(token);
                }
                _ => {
                    out.extend(current.take());
                    if has_scheme(token) {
                        current = Some(token.to_owned());
                    } else {
                        out.push(token.to_owned());
                    }
                }
            }
        }
        out.extend(current);
    }
    out
}

/// Expands batch patterns in a link, IDM-style: `img[001-120].jpg` gives
/// 120 links with zero-padded numbers, `part[a-f].zip` gives letters, and
/// several patterns combine. Text without a valid pattern is returned as is.
/// At most `limit` links are produced.
pub fn expand_pattern(input: &str, limit: usize) -> Vec<String> {
    fn parse(body: &str, limit: usize) -> Option<Vec<String>> {
        let (a, b) = body.split_once('-')?;
        let (a, b) = (a.trim(), b.trim());
        if let (Ok(x), Ok(y)) = (a.parse::<u64>(), b.parse::<u64>()) {
            let width = if a.len() > 1 && a.starts_with('0') { a.len() } else { 0 };
            let (lo, hi) = (x.min(y), x.max(y));
            if hi - lo > 1_000_000 {
                return None;
            }
            // Only as many as can be used: `[1-999999]` must not build a million strings.
            let take = limit.max(1);
            let v: Vec<String> = if x > y {
                (lo..=hi).rev().take(take).map(|n| format!("{n:0width$}")).collect()
            } else {
                (lo..=hi).take(take).map(|n| format!("{n:0width$}")).collect()
            };
            return Some(v);
        }
        let (mut ca, mut cb) = (a.chars(), b.chars());
        match (ca.next(), ca.next(), cb.next(), cb.next()) {
            (Some(x), None, Some(y), None)
                if x.is_ascii_alphabetic()
                    && y.is_ascii_alphabetic()
                    && x.is_ascii_lowercase() == y.is_ascii_lowercase() =>
            {
                let (lo, hi) = (x.min(y), x.max(y));
                Some((lo..=hi).map(String::from).collect())
            }
            _ => None,
        }
    }

    let Some(open) = input.find('[') else { return vec![input.to_owned()] };
    let Some(close) = input[open..].find(']').map(|i| open + i) else { return vec![input.to_owned()] };
    let Some(values) = parse(&input[open + 1..close], limit) else {
        // Not a pattern (e.g. an IPv6 host); keep it and look further.
        let rest = expand_pattern(&input[close + 1..], limit);
        return rest.into_iter().map(|r| format!("{}{r}", &input[..=close])).collect();
    };
    // Expanded once: doing it per value makes the time double with every pattern.
    let tails = expand_pattern(&input[close + 1..], limit);
    let mut out = Vec::new();
    for v in values {
        for tail in &tails {
            if out.len() >= limit {
                return out;
            }
            out.push(format!("{}{v}{tail}", &input[..open]));
        }
    }
    out
}

/// What to do with the computer once all downloads are finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Shutdown,
}

/// Puts the computer to sleep or shuts it down.
pub fn power(action: PowerAction) -> std::io::Result<()> {
    use std::process::Command;
    let mut cmd = match action {
        #[cfg(windows)]
        PowerAction::Sleep => {
            // Called directly: `rundll32 powrprof.dll,SetSuspendState` passes
            // garbage arguments and hibernates when hibernation is enabled.
            // SAFETY: plain Win32 call without pointers.
            let ok = unsafe { windows_sys::Win32::System::Power::SetSuspendState(false, false, false) };
            return if ok { Ok(()) } else { Err(std::io::Error::last_os_error()) };
        }
        #[cfg(windows)]
        PowerAction::Shutdown => {
            let mut c = Command::new("shutdown");
            c.args(["/s", "/t", "10"]);
            c
        }
        #[cfg(target_os = "macos")]
        PowerAction::Sleep => {
            let mut c = Command::new("pmset");
            c.arg("sleepnow");
            c
        }
        #[cfg(target_os = "macos")]
        PowerAction::Shutdown => {
            let mut c = Command::new("osascript");
            c.args(["-e", "tell app \"System Events\" to shut down"]);
            c
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        PowerAction::Sleep => {
            let mut c = Command::new("systemctl");
            c.arg("suspend");
            c
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        PowerAction::Shutdown => {
            let mut c = Command::new("systemctl");
            c.arg("poweroff");
            c
        }
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map(drop)
}

/// Splits a command line into arguments: whitespace separates them, double
/// quotes group them (`"C:\Program Files\x.exe" -f`). Backslashes are kept as
/// typed, so Windows paths work.
pub fn split_command(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut started) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

/// Runs the "after each download" command for a finished file. `{path}`,
/// `{dir}`, `{name}` and `{url}` are replaced inside each argument after
/// splitting, and the program is started directly, without a shell, so a
/// file name such as `a & del *.*` stays one harmless argument.
pub fn run_after_download(command: &str, path: &Path, url: &str) -> std::io::Result<()> {
    let args = command_args(command, path, url);
    let Some((program, rest)) = args.split_first() else { return Ok(()) };
    let mut cmd = std::process::Command::new(program);
    cmd.args(rest);
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Runs on a background thread; a program that hangs is stopped after a while.
    let mut child = cmd.spawn()?;
    let deadline = std::time::Instant::now() + AFTER_DOWNLOAD_LIMIT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the after-download program took too long"));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// How long the "after each download" program may run.
const AFTER_DOWNLOAD_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

fn command_args(command: &str, path: &Path, url: &str) -> Vec<String> {
    let dir = path.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let mut name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // A server-chosen name must not read as an option to the program.
    if name.starts_with('-') {
        name.insert_str(0, "./");
    }
    let full = path.display().to_string();
    let values = [("{path}", full.as_str()), ("{dir}", dir.as_str()), ("{name}", name.as_str()), ("{url}", url)];
    split_command(command).into_iter().map(|a| substitute(&a, &values)).collect()
}

/// Replaces placeholders in one pass: a value that itself contains `{url}` (a
/// server can name a file that) is not expanded again.
fn substitute(text: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while !rest.is_empty() {
        for (key, value) in values {
            if let Some(after) = rest.strip_prefix(key) {
                out.push_str(value);
                rest = after;
                continue 'outer;
            }
        }
        let c = rest.chars().next().expect("not empty");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn explorer_gets_the_path_in_quotes_with_backslashes() {
        use super::explorer_select_arg;
        let arg = explorer_select_arg(std::path::Path::new("C:/Users/me/My Files/report v2.pdf"));
        assert_eq!(arg, r#"/select,"C:\Users\me\My Files\report v2.pdf""#);
    }

    #[test]
    fn tokens_are_random_and_compared_fully() {
        use super::{random_token, secrets_equal};
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(secrets_equal(&a, &a.clone()));
        assert!(!secrets_equal(&a, &b));
        assert!(!secrets_equal("abc", "abcd"));
        assert!(!secrets_equal("", "x"));
    }

    #[test]
    fn disguised_programs() {
        use super::disguised_program;
        assert!(disguised_program("Invoice.PDF.exe"));
        assert!(disguised_program("photo.jpg.scr"));
        assert!(disguised_program("report.pdf   .exe"));
        assert!(!disguised_program("setup.exe"));
        assert!(!disguised_program("archive.tar.gz"));
        assert!(!disguised_program("notes.txt"));
        assert!(!disguised_program("my.app.exe"));
    }

    use super::*;

    #[test]
    fn commands_are_split_without_a_shell() {
        assert_eq!(
            split_command(r#""C:\Program Files\Scan\scan.exe" -File {path}  "" x"#),
            [r"C:\Program Files\Scan\scan.exe", "-File", "{path}", "", "x"]
        );
        let path = Path::new("/dl/a & rm -rf ~.zip");
        let args = command_args("7z x {path} -o{dir}/out", path, "https://x/a");
        assert_eq!(args, ["7z", "x", "/dl/a & rm -rf ~.zip", "-o/dl/out"], "a file name stays one argument");
        assert_eq!(command_args("tool {name}", Path::new("/dl/-rf"), "u"), ["tool", "./-rf"]);
        // A name containing a placeholder is not expanded again.
        assert_eq!(command_args("tool {name} {url}", Path::new("/dl/{url}.zip"), "u"), ["tool", "{url}.zip", "u"]);
    }

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.50 KiB");
        assert_eq!(format_bytes(150 * 1024 * 1024), "150 MiB");
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("0"), Some(0));
        assert_eq!(parse_size("500k"), Some(500 * 1024));
        assert_eq!(parse_size("2M"), Some(2 << 20));
        assert_eq!(parse_size("1.5 MiB"), Some(1_572_864));
        assert_eq!(parse_size("10mb/s"), Some(10 << 20));
        assert_eq!(parse_size("abc"), None);
        assert_eq!(parse_size("5x"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(5), "5s");
        assert_eq!(format_duration(187), "3m 07s");
        assert_eq!(format_duration(3840), "1h 04m");
        assert_eq!(format_duration(90_000), "1d 01h");
    }

    #[test]
    fn relative_times() {
        let now = 1_800_000_000;
        assert_eq!(format_relative(now - 5, now), "just now");
        assert_eq!(format_relative(now - 600, now), "10 min ago");
        assert_eq!(format_relative(now - 2 * 3600, now), "2 h ago");
        assert_eq!(format_relative(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(format_relative(now - 40 * 86_400, now).len(), 10, "a date");
    }

    #[test]
    fn civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn pasted_links() {
        assert_eq!(
            split_links("https://x/Firefox Setup 131.0.exe?a=1"),
            ["https://x/Firefox%20Setup%20131.0.exe?a=1"],
            "spaces in a link don't split it"
        );
        assert_eq!(
            split_links("https://x/a.zip https://x/b.zip\n  http://y/c.iso "),
            ["https://x/a.zip", "https://x/b.zip", "http://y/c.iso"]
        );
        assert_eq!(split_links("example.com/a.zip example.com/b.zip"), ["example.com/a.zip", "example.com/b.zip"]);
        assert_eq!(split_links("see: https://x/a.zip"), ["see:", "https://x/a.zip"]);
        assert!(split_links(" \n ").is_empty());
    }

    #[test]
    fn batch_patterns() {
        assert_eq!(expand_pattern("https://x/a.zip", 10), ["https://x/a.zip"]);
        assert_eq!(
            expand_pattern("https://x/img[08-11].jpg", 10),
            ["https://x/img08.jpg", "https://x/img09.jpg", "https://x/img10.jpg", "https://x/img11.jpg"]
        );
        assert_eq!(expand_pattern("https://x/[a-c]/[1-2]", 10).len(), 6);
        assert_eq!(expand_pattern("https://x/[1-100]", 5).len(), 5);
        assert_eq!(expand_pattern("https://x/[9-1]", 2), ["https://x/9", "https://x/8"]);
        assert_eq!(expand_pattern("https://x/[1-999999]", 3).len(), 3);
        // Many patterns must not take exponential time.
        let many = format!("https://x/{}", "[a-b]".repeat(60));
        assert_eq!(expand_pattern(&many, 100).len(), 100);
        assert_eq!(expand_pattern("https://x/[A-z]", 100), ["https://x/[A-z]"]);
        assert_eq!(expand_pattern("http://[::1]/f[1-2]", 10), ["http://[::1]/f1", "http://[::1]/f2"]);
    }
}
