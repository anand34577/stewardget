//! Site grabber: finds the downloadable files linked from a web page, and
//! optionally from the pages it links to.
//!
//! It is polite by design: it reads `robots.txt`, fetches one page at a time
//! with a short pause, stays on the starting site unless told otherwise, and
//! stops at fixed page and link budgets. It reads static HTML only (no
//! scripts are run), so pages that build their links in the browser need the
//! extension's "Download all links" instead.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use reqwest::Client;
use url::Url;

use crate::{Config, Error};

/// Most pages read in one run.
pub const MAX_PAGES: usize = 100;
/// Most files reported in one run.
pub const MAX_FILES: usize = 5000;
/// Largest page body read.
const MAX_BODY: usize = 4 * 1024 * 1024;
const PAUSE: Duration = Duration::from_millis(120);

/// What to look for.
#[derive(Debug, Clone, Default)]
pub struct GrabOptions {
    /// Link levels followed from the first page: 0 reads only that page.
    pub depth: u8,
    /// Follow pages on other sites too.
    pub other_sites: bool,
    /// Extensions that count as files (`zip`, `tar.gz`…). Empty: every link to
    /// a known kind of file.
    pub extensions: Vec<String>,
}

/// Files found, and how far the search went.
#[derive(Debug, Clone, Default)]
pub struct GrabResult {
    pub pages: usize,
    /// `(file url, page it was linked from)`, in page order, without duplicates.
    pub files: Vec<(String, String)>,
    /// Pages left alone because the site's `robots.txt` asks crawlers to.
    pub blocked_by_robots: usize,
    /// A budget ended the search before every page was read.
    pub truncated: bool,
}

pub(crate) async fn grab(
    client: &Client,
    config: &Config,
    start: &str,
    options: &GrabOptions,
) -> Result<GrabResult, Error> {
    let start = Url::parse(start.trim()).map_err(|e| Error::InvalidUrl(e.to_string()))?;
    if !matches!(start.scheme(), "http" | "https") {
        return Err(Error::InvalidUrl("only http and https pages can be read".into()));
    }
    let mut result = GrabResult::default();
    let mut seen_pages: HashSet<String> = HashSet::from([page_key(&start)]);
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut robots: Vec<(String, Vec<String>)> = Vec::new();
    let mut queue: VecDeque<(Url, u8)> = VecDeque::from([(start.clone(), 0)]);

    while let Some((page, level)) = queue.pop_front() {
        if result.pages >= MAX_PAGES {
            result.truncated = true;
            break;
        }
        let host = page.host_str().unwrap_or("").to_owned();
        let rules = match robots.iter().find(|(h, _)| *h == host) {
            Some((_, rules)) => rules.clone(),
            None => {
                let rules = fetch_robots(client, &page).await;
                robots.push((host.clone(), rules.clone()));
                rules
            }
        };
        let target = match page.query() {
            Some(q) => format!("{}?{q}", page.path()),
            None => page.path().to_owned(),
        };
        if rules.iter().any(|rule| robots_match(rule, &target)) {
            result.blocked_by_robots += 1;
            continue;
        }
        let Some(html) = fetch_page(client, &page).await else {
            if result.pages == 0 && level == 0 {
                return Err(Error::Network(format!("could not read {page}")));
            }
            continue;
        };
        result.pages += 1;
        for link in links(&html, &page) {
            if is_file(&link, options, config) {
                if result.files.len() >= MAX_FILES {
                    result.truncated = true;
                    break;
                }
                if seen_files.insert(link.as_str().to_owned()) {
                    result.files.push((link.to_string(), page.to_string()));
                }
            } else if level < options.depth
                && looks_like_page(&link)
                && (options.other_sites || link.host_str() == start.host_str())
                && seen_pages.insert(page_key(&link))
            {
                queue.push_back((link, level + 1));
            }
        }
        tokio::time::sleep(PAUSE).await;
    }
    Ok(result)
}

fn page_key(url: &Url) -> String {
    let mut u = url.clone();
    u.set_fragment(None);
    u.to_string()
}

async fn fetch_page(client: &Client, url: &Url) -> Option<String> {
    let resp = client.get(url.clone()).timeout(Duration::from_secs(20)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if let Some(kind) = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok())
        && !(kind.contains("html") || kind.contains("xml") || kind.starts_with("text/"))
    {
        return None;
    }
    let mut body = Vec::new();
    let mut resp = resp;
    while let Ok(Some(chunk)) = resp.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_BODY {
            break;
        }
    }
    Some(String::from_utf8_lossy(&body).into_owned())
}

/// The `Disallow` prefixes that apply to everyone (`User-agent: *`).
async fn fetch_robots(client: &Client, page: &Url) -> Vec<String> {
    let Ok(robots) = page.join("/robots.txt") else { return Vec::new() };
    let Ok(resp) = client.get(robots).timeout(Duration::from_secs(10)).send().await else { return Vec::new() };
    if !resp.status().is_success() {
        return Vec::new();
    }
    parse_robots(&resp.text().await.unwrap_or_default())
}

fn parse_robots(text: &str) -> Vec<String> {
    let mut applies = false;
    let mut previous_was_agent = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match key.as_str() {
            "user-agent" => {
                // Consecutive agent lines share one rule group.
                applies = if previous_was_agent { applies || value == "*" } else { value == "*" };
                previous_was_agent = true;
                continue;
            }
            "disallow" if applies && !value.is_empty() => out.push(value.to_owned()),
            _ => {}
        }
        previous_was_agent = false;
    }
    out
}

/// Whether a `Disallow` rule covers a path: a prefix match, where `*` stands
/// for any text and a trailing `$` pins the end (`/*.pdf$`).
fn robots_match(rule: &str, path: &str) -> bool {
    let (rule, anchored) = rule.strip_suffix('$').map_or((rule, false), |r| (r, true));
    let mut parts = rule.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = path.strip_prefix(first) else { return false };
    let tail: Vec<&str> = parts.collect();
    for (i, part) in tail.iter().enumerate() {
        let last = i + 1 == tail.len();
        if last && anchored {
            // The final piece must end the path.
            return rest.ends_with(part) && (part.len() <= rest.len());
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    !anchored || (tail.is_empty() && rest.is_empty())
}

/// Every `href` and `src` in the page, as absolute http(s) URLs.
fn links(html: &str, base: &Url) -> Vec<Url> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    for attr in ["href", "src"] {
        let mut from = 0;
        while let Some(at) = lower[from..].find(attr) {
            let start = from + at + attr.len();
            from = start;
            // Must be a whole attribute name: `... href=`, not `data-href2=`.
            let before = lower[..start - attr.len()].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':') {
                continue;
            }
            let rest = html[start..].trim_start();
            let Some(rest) = rest.strip_prefix('=') else { continue };
            let rest = rest.trim_start();
            let value = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
                _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
            };
            let value = value.trim();
            if value.is_empty() || value.starts_with('#') || value.starts_with("javascript:") {
                continue;
            }
            if let Ok(mut url) = base.join(&decode_entities(value))
                && matches!(url.scheme(), "http" | "https")
            {
                url.set_fragment(None);
                out.push(url);
            }
        }
    }
    out
}

fn decode_entities(value: &str) -> String {
    value.replace("&amp;", "&").replace("&#38;", "&").replace("&#x26;", "&")
}

fn extension_of(url: &Url) -> String {
    let name = url.path().rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    name.rsplit_once('.').map(|(_, e)| e.to_owned()).unwrap_or_default()
}

fn is_file(url: &Url, options: &GrabOptions, config: &Config) -> bool {
    let name = url.path().rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    if !options.extensions.is_empty() {
        return options.extensions.iter().any(|e| name.len() > e.len() + 1 && name.ends_with(&format!(".{e}")));
    }
    config.categorize(&name) != crate::Category::Other
}

fn looks_like_page(url: &Url) -> bool {
    matches!(extension_of(url).as_str(), "" | "html" | "htm" | "xhtml" | "shtml" | "php" | "asp" | "aspx" | "jsp")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn finds_links_in_messy_html() {
        let html = r##"<a HREF="/files/a.zip">a</a> <a href='b.iso'>b</a> <img src=pic.png>
            <a href="#top">x</a> <a href="javascript:void(0)">y</a> <a href="mailto:me@x.y">m</a>
            <a data-href="no.zip">d</a> <a href="/q?a=1&amp;b=2">q</a>"##;
        let found: Vec<String> =
            links(html, &url("https://example.com/dir/page.html")).iter().map(Url::to_string).collect();
        assert!(found.contains(&"https://example.com/files/a.zip".to_owned()));
        assert!(found.contains(&"https://example.com/dir/b.iso".to_owned()));
        assert!(found.contains(&"https://example.com/dir/pic.png".to_owned()));
        assert!(found.contains(&"https://example.com/q?a=1&b=2".to_owned()));
        assert!(!found.iter().any(|u| u.contains("no.zip") || u.contains("mailto") || u.contains("top")));
    }

    #[test]
    fn files_and_pages() {
        let config = Config::default();
        let any = GrabOptions::default();
        assert!(is_file(&url("https://x.com/a/setup.exe"), &any, &config));
        assert!(!is_file(&url("https://x.com/a/"), &any, &config));
        assert!(!is_file(&url("https://x.com/a/readme.unknownext"), &any, &config));
        let only = GrabOptions { extensions: vec!["tar.gz".into()], ..Default::default() };
        assert!(is_file(&url("https://x.com/src.tar.gz"), &only, &config));
        assert!(!is_file(&url("https://x.com/setup.exe"), &only, &config));
        assert!(looks_like_page(&url("https://x.com/docs/")));
        assert!(looks_like_page(&url("https://x.com/a.php?x=1")));
        assert!(!looks_like_page(&url("https://x.com/a.zip")));
    }

    #[test]
    fn robots_patterns() {
        assert!(robots_match("/private/", "/private/a.zip"));
        assert!(!robots_match("/private/", "/public/a.zip"));
        assert!(robots_match("/*.pdf$", "/docs/a.pdf"));
        assert!(!robots_match("/*.pdf$", "/docs/a.pdf?x=1"));
        assert!(!robots_match("/*.pdf$", "/docs/a.pdfx"));
        assert!(robots_match("/a*/c", "/abc/c"));
        assert!(!robots_match("/a*/c", "/abc/d"));
        assert!(robots_match("/exact$", "/exact"));
        assert!(!robots_match("/exact$", "/exact/more"));
    }

    #[test]
    fn robots_rules() {
        let text = "User-agent: Googlebot\nDisallow: /g\n\nUser-agent: *\nDisallow: /private/\nDisallow:\n# c\nDisallow: /tmp # note\n";
        assert_eq!(parse_robots(text), ["/private/", "/tmp"]);
        assert!(parse_robots("User-agent: bot\nDisallow: /").is_empty());
    }
}
