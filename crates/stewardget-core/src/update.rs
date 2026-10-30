//! The optional update check: one request to the project's public release page,
//! made only when the user turned it on (or pressed "Check now"). It sends no
//! identifiers, only a generic `User-Agent`.

use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/anand34577/stewardget/releases/latest";

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Version number without the leading `v`.
    pub version: String,
    /// Page to read about and download it.
    pub url: String,
}

/// Fetches the newest published release.
pub async fn latest() -> Result<Release, String> {
    let client = reqwest::Client::builder()
        .user_agent("StewardGet-update-check")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let response =
        client.get(LATEST).header("Accept", "application/vnd.github+json").send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("the release page answered HTTP {}", response.status().as_u16()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    parse(&bytes)
}

fn parse(body: &[u8]) -> Result<Release, String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().ok_or("the release has no version")?;
    let url = json["html_url"].as_str().unwrap_or("https://github.com/anand34577/stewardget/releases/latest");
    Ok(Release { version: tag.trim_start_matches('v').to_owned(), url: url.to_owned() })
}

/// Whether `candidate` is a higher version than `current` (`1.2.3` style; anything
/// after a `-` or `+` is ignored).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(candidate), parts(current));
    (0..a.len().max(b.len()))
        .map(|i| (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0)))
        .find(|(x, y)| x != y)
        .is_some_and(|(x, y)| x > y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.9.0", "0.8.0"));
        assert!(is_newer("0.10.0", "0.9.5"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.8.0", "0.8.0"));
        assert!(!is_newer("0.7.4", "0.8.0"));
        assert!(!is_newer("garbage", "0.8.0"));
    }

    #[test]
    fn reads_a_release() {
        let r = parse(br#"{"tag_name":"v0.9.1","html_url":"https://example.org/r"}"#).unwrap();
        assert_eq!(r, Release { version: "0.9.1".into(), url: "https://example.org/r".into() });
        assert!(parse(b"{}").is_err());
    }
}
