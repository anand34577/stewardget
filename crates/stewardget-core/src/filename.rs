//! File-name discovery and sanitisation.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use url::Url;

/// Extracts a file name from a `Content-Disposition` header (RFC 6266).
/// `filename*` (RFC 5987, UTF-8) wins over plain `filename`.
pub fn from_content_disposition(header: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for part in split_params(header) {
        let Some((key, value)) = part.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if key == "filename*" {
            // charset'lang'percent-encoded
            let mut it = value.splitn(3, '\'');
            if let (Some(_charset), Some(_lang), Some(encoded)) = (it.next(), it.next(), it.next()) {
                extended = Some(percent_decode_str(encoded.trim_matches('"')).decode_utf8_lossy().into_owned());
            }
        } else if key == "filename" {
            let v = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
            plain = Some(v.replace("\\\"", "\""));
        }
    }
    extended.or(plain).filter(|n| !n.trim().is_empty())
}

/// Splits on `;` while respecting quoted strings.
fn split_params(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// The last non-empty path segment of a URL, percent-decoded.
pub fn from_url(url: &Url) -> Option<String> {
    url.path_segments()?
        .rev()
        .find(|s| !s.is_empty())
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
}

/// A sensible extension for common MIME types, used when a name has none.
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let essence = mime.split(';').next()?.trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "application/zip" => "zip",
        "application/pdf" => "pdf",
        "application/x-7z-compressed" => "7z",
        "application/x-rar-compressed" | "application/vnd.rar" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-xz" => "xz",
        "application/json" => "json",
        "application/x-msdownload" | "application/vnd.microsoft.portable-executable" => "exe",
        "application/x-msi" => "msi",
        "application/x-apple-diskimage" => "dmg",
        "application/vnd.debian.binary-package" => "deb",
        "application/x-iso9660-image" => "iso",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/csv" => "csv",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/flac" => "flac",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        _ => return None,
    })
}

/// Makes a name safe on every major file system.
pub fn sanitize(name: &str) -> String {
    // Only keep the final component if a server tries a path.
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        // Bidirectional overrides make `invoice\u{202e}fdp.exe` display as
        // `invoiceexe.pdf`; they have no business in a file name.
        .filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect();
    let trimmed = out.trim().trim_end_matches(['.', ' ']).trim_start_matches('.');
    out = trimmed.to_owned();
    if out.is_empty() {
        out = "download".to_owned();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let reserved_numbered =
        (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit();
    if RESERVED.contains(&stem.as_str()) || reserved_numbered {
        out.insert(0, '_');
    }
    truncate_preserving_extension(&out, 200)
}

fn truncate_preserving_extension(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_owned();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if e.len() <= 16 && !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    let budget = max_bytes - ext.map_or(0, |e| e.len() + 1);
    let mut cut = budget.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    match ext {
        Some(e) => format!("{}.{e}", &stem[..cut]),
        None => stem[..cut].to_owned(),
    }
}

/// Returns `dir/name`, or `dir/name (n).ext` when `taken` reports a clash.
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let candidate = dir.join(name);
    if !taken(&candidate) {
        return candidate;
    }
    let (stem, ext) = split_extension(name);
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("an unbounded range always yields a free name")
}

fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for double in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if lower.ends_with(double) && name.len() > double.len() {
            return name.split_at(name.len() - double.len());
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bidi_overrides_are_removed() {
        assert_eq!(sanitize("invoice\u{202e}fdp.exe"), "invoicefdp.exe");
        assert_eq!(sanitize("\u{2067}a\u{2069}.txt"), "a.txt");
    }

    #[test]
    fn content_disposition() {
        assert_eq!(from_content_disposition(r#"attachment; filename="a b.zip""#).as_deref(), Some("a b.zip"));
        assert_eq!(
            from_content_disposition(r#"attachment; filename="x.zip"; filename*=UTF-8''na%C3%AFve%20file.zip"#)
                .as_deref(),
            Some("naïve file.zip")
        );
        assert_eq!(from_content_disposition("attachment; filename=plain.txt").as_deref(), Some("plain.txt"));
        assert_eq!(
            from_content_disposition(r#"attachment; filename="semi;colon.txt""#).as_deref(),
            Some("semi;colon.txt")
        );
        assert_eq!(from_content_disposition("inline"), None);
    }

    #[test]
    fn url_names() {
        let u = Url::parse("https://example.com/files/My%20File.tar.gz?x=1").unwrap();
        assert_eq!(from_url(&u).as_deref(), Some("My File.tar.gz"));
        let u = Url::parse("https://example.com/dir/").unwrap();
        assert_eq!(from_url(&u).as_deref(), Some("dir"));
        let u = Url::parse("https://example.com/").unwrap();
        assert_eq!(from_url(&u), None);
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("a<b>c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("  .hidden. "), "hidden");
        assert_eq!(sanitize(""), "download");
        let long = format!("{}.iso", "x".repeat(400));
        let s = sanitize(&long);
        assert!(s.len() <= 200 && s.ends_with(".iso"));
    }

    #[test]
    fn unique() {
        let dir = Path::new("d");
        let taken = |p: &Path| p == Path::new("d/a.tar.gz") || p == Path::new("d/a (1).tar.gz");
        assert_eq!(unique_path(dir, "a.tar.gz", taken), PathBuf::from("d/a (2).tar.gz"));
        assert_eq!(unique_path(dir, "b.zip", taken), PathBuf::from("d/b.zip"));
    }
}
