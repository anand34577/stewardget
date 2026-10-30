//! "Why is this download running at this speed?"
//!
//! A pure function from what the engine already knows about a download to a
//! plain-language explanation and, where one exists, a fix the user can apply
//! with a click. No network access, no state: the front-ends call it every frame.

use crate::util::format_speed;
use crate::{DownloadView, Status};

/// Settings and measurements around a download that explain its speed.
#[derive(Debug, Clone, Default)]
pub struct InsightContext {
    /// Global limit in bytes/second; `0` = none.
    pub global_limit: u64,
    /// The time-of-day limit in force instead of `global_limit`.
    pub scheduled_limit: Option<u64>,
    /// Limit of the download's queue; `0` = none.
    pub queue_limit: u64,
    /// Total speed of all downloads, bytes/second.
    pub aggregate_speed: f64,
    /// Downloads currently transferring.
    pub active_downloads: usize,
    /// Connections the settings allow for this download.
    pub configured_connections: usize,
    /// Mean speed earlier downloads from the same host reached, if any.
    pub host_average: Option<f64>,
    /// The limits are shown on their own ([`limit_notices`]), so [`explain`]
    /// leaves them out and says what else is going on.
    pub limits_separate: bool,
}

/// What kind of situation the download is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Starting,
    Healthy,
    /// Held back by its own speed limit.
    DownloadLimit,
    /// Held back by the global (or time-of-day) limit.
    GlobalLimit,
    /// Held back by its queue's limit.
    QueueLimit,
    /// Other downloads use the same line.
    SharedLine,
    /// The server does not allow byte ranges, so one connection carries the file.
    SingleStream,
    /// The server accepts fewer connections than were asked for.
    ConnectionCap,
    /// Most connections keep dropping.
    Unstable,
    /// Nothing has arrived for a while.
    Stalled,
    SlowerThanUsual,
    FasterThanUsual,
    Finishing,
}

/// A one-click remedy attached to an insight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Remove this download's own speed limit.
    RemoveDownloadLimit,
    /// Remove the global speed limit.
    RemoveGlobalLimit,
    /// Open the properties of the download (connections, headers…).
    OpenProperties,
    /// Pause, then resume: a fresh set of connections.
    Reconnect,
}

impl Fix {
    pub fn label(self) -> &'static str {
        match self {
            Fix::RemoveDownloadLimit => "Remove this limit",
            Fix::RemoveGlobalLimit => "Remove the global limit",
            Fix::OpenProperties => "Open properties",
            Fix::Reconnect => "Reconnect",
        }
    }
}

/// How much attention an insight deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tone {
    Good,
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Insight {
    pub kind: Kind,
    pub tone: Tone,
    pub headline: String,
    pub detail: String,
    pub fix: Option<Fix>,
}

/// Fraction of a limit above which the limit is what holds the speed.
const AT_LIMIT: f64 = 0.85;
/// Samples (500 ms each) of silence before a download counts as stalled.
const STALL_SAMPLES: usize = 10;

fn insight(
    kind: Kind,
    tone: Tone,
    headline: impl Into<String>,
    detail: impl Into<String>,
    fix: Option<Fix>,
) -> Insight {
    Insight { kind, tone, headline: headline.into(), detail: detail.into(), fix }
}

/// Explains the current speed of `d`, or `None` when there is nothing to say
/// (not transferring, or finished).
pub fn explain(d: &DownloadView, cx: &InsightContext) -> Option<Insight> {
    match d.status {
        Status::Connecting => {
            return Some(insight(
                Kind::Starting,
                Tone::Info,
                "Contacting the server",
                "Checking the file's size and whether it can be downloaded in parts.",
                None,
            ));
        }
        Status::Downloading => {}
        _ => return None,
    }

    let silent = d.history.len() >= STALL_SAMPLES && d.history.iter().rev().take(STALL_SAMPLES).all(|s| *s < 1.0);
    if d.speed < 1.0 && (silent || (d.history.is_empty() && d.downloaded > 0)) {
        return Some(if d.reconnecting > 0 {
            insight(
                Kind::Unstable,
                Tone::Warn,
                "Connection lost — retrying",
                "The server or your network stopped answering. StewardGet keeps retrying with growing pauses.",
                Some(Fix::Reconnect),
            )
        } else {
            insight(
                Kind::Stalled,
                Tone::Warn,
                "No data is arriving",
                "The server is connected but silent. If this lasts, reconnecting often helps.",
                Some(Fix::Reconnect),
            )
        });
    }

    // Limits come first: they are the one cause the user can lift immediately.
    if !cx.limits_separate {
        let own = d.options.speed_limit;
        if own > 0 && d.speed >= own as f64 * AT_LIMIT {
            return Some(insight(
                Kind::DownloadLimit,
                Tone::Info,
                format!("Held at this download's limit ({})", format_speed(own as f64)),
                "You set a speed limit on this download. It would go faster without it.",
                Some(Fix::RemoveDownloadLimit),
            ));
        }
        let global = cx.scheduled_limit.unwrap_or(cx.global_limit);
        if global > 0 && cx.aggregate_speed >= global as f64 * AT_LIMIT {
            let scheduled = cx.scheduled_limit.is_some();
            return Some(insight(
                Kind::GlobalLimit,
                Tone::Info,
                format!("Held at the overall limit ({})", format_speed(global as f64)),
                if scheduled {
                    "A time-of-day speed limit is active. It ends at the hour set in Settings."
                } else {
                    "The speed limit in the status bar applies to all downloads together."
                },
                (!scheduled).then_some(Fix::RemoveGlobalLimit),
            ));
        }
        if cx.queue_limit > 0 && cx.aggregate_speed >= cx.queue_limit as f64 * AT_LIMIT {
            return Some(insight(
                Kind::QueueLimit,
                Tone::Info,
                format!("Held at the queue's limit ({})", format_speed(cx.queue_limit as f64)),
                "This download's queue shares a speed cap between its downloads.",
                None,
            ));
        }
    }

    if d.connections > 0 && d.reconnecting * 2 >= d.connections {
        return Some(insight(
            Kind::Unstable,
            Tone::Warn,
            "Many connections keep dropping",
            "The server is refusing or resetting most connections. StewardGet backs off automatically; \
             fewer connections often work better for this server.",
            Some(Fix::OpenProperties),
        ));
    }

    if !d.resumable && d.downloaded > 0 || (d.connections == 1 && !d.resumable && d.total.is_some()) {
        return Some(insight(
            Kind::SingleStream,
            Tone::Info,
            "This server sends the file in one piece",
            "It doesn't support downloading in parts, so one connection carries the whole file and \
             adding connections would not help.",
            None,
        ));
    }

    let wanted = d.options.connections.unwrap_or(cx.configured_connections);
    if d.resumable && wanted > 1 && d.connections > 0 && d.connections + 1 < wanted && d.fraction().unwrap_or(0.0) < 0.9
    {
        return Some(insight(
            Kind::ConnectionCap,
            Tone::Info,
            format!("The server allows {} of {wanted} connections", d.connections),
            "Servers often cap connections per visitor. The extra ones back off so they don't slow the others down.",
            None,
        ));
    }

    if cx.active_downloads > 1 && cx.aggregate_speed > d.speed * 1.3 {
        return Some(insight(
            Kind::SharedLine,
            Tone::Info,
            format!(
                "Sharing your connection with {} other download{}",
                cx.active_downloads - 1,
                plural(cx.active_downloads - 1)
            ),
            format!(
                "Together they use {}. This one gets {} of it.",
                format_speed(cx.aggregate_speed),
                format_speed(d.speed)
            ),
            None,
        ));
    }

    if let Some(avg) = cx.host_average.filter(|a| *a > 0.0) {
        if d.speed < avg * 0.4 && d.active_secs >= 10 {
            return Some(insight(
                Kind::SlowerThanUsual,
                Tone::Warn,
                "Slower than usual from this server",
                format!(
                    "Earlier downloads from it averaged {}. Its load, or your network, may be the cause.",
                    format_speed(avg)
                ),
                Some(Fix::Reconnect),
            ));
        }
        if d.speed > avg * 1.3 && d.active_secs >= 10 {
            return Some(insight(
                Kind::FasterThanUsual,
                Tone::Good,
                "Faster than usual from this server",
                format!("Earlier downloads from it averaged {}.", format_speed(avg)),
                None,
            ));
        }
    }

    if d.fraction().is_some_and(|f| f >= 0.97) {
        return Some(insight(Kind::Finishing, Tone::Good, "Almost there", "Finishing the last pieces.", None));
    }

    Some(insight(
        Kind::Healthy,
        Tone::Good,
        "Running at full speed",
        format!(
            "{} connection{} carrying {}. Nothing is holding this download back.",
            d.connections.max(1),
            plural(d.connections.max(1)),
            format_speed(d.speed)
        ),
        None,
    ))
}

/// The speed limits that apply to `d`, one notice each, for as long as they
/// are set: they don't come and go with the speed, which sits near a limit
/// without always reaching it. Empty unless the download is transferring.
pub fn limit_notices(d: &DownloadView, cx: &InsightContext) -> Vec<Insight> {
    let mut out = Vec::new();
    if d.status != Status::Downloading {
        return out;
    }
    let own = d.options.speed_limit;
    if own > 0 {
        out.push(insight(
            Kind::DownloadLimit,
            Tone::Info,
            format!("This download is limited to {}", format_speed(own as f64)),
            "You set a speed limit on this download. It would go faster without it.",
            Some(Fix::RemoveDownloadLimit),
        ));
    }
    let global = cx.scheduled_limit.unwrap_or(cx.global_limit);
    if global > 0 {
        let scheduled = cx.scheduled_limit.is_some();
        out.push(insight(
            Kind::GlobalLimit,
            Tone::Info,
            format!("Overall limit: {}", format_speed(global as f64)),
            if scheduled {
                "A time-of-day speed limit is active. It ends at the hour set in Settings."
            } else {
                "The speed limit in the status bar applies to all downloads together."
            },
            (!scheduled).then_some(Fix::RemoveGlobalLimit),
        ));
    }
    if cx.queue_limit > 0 {
        out.push(insight(
            Kind::QueueLimit,
            Tone::Info,
            format!("Queue limit: {}", format_speed(cx.queue_limit as f64)),
            "This download's queue shares a speed cap between its downloads.",
            None,
        ));
    }
    out
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, DownloadOptions};

    fn view() -> DownloadView {
        DownloadView {
            id: 1,
            url: "https://h.example/f".into(),
            file_name: "f".into(),
            path: Default::default(),
            category: Category::Other,
            queue: 0,
            status: Status::Downloading,
            total: Some(1000),
            downloaded: 100,
            speed: 5_000_000.0,
            eta: None,
            connections: 8,
            reconnecting: 0,
            active_secs: 30,
            resumable: true,
            segments: vec![],
            history: vec![5e6; 20],
            error: None,
            added_at: 0,
            finished_at: None,
            options: DownloadOptions::default(),
            checksum_verified: false,
            awaiting_link: false,
            retry_in: None,
            retries: 0,
            lanes: Vec::new(),
            tuned_connections: None,
        }
    }

    fn cx() -> InsightContext {
        InsightContext { configured_connections: 8, active_downloads: 1, aggregate_speed: 5e6, ..Default::default() }
    }

    fn kind(d: &DownloadView, c: &InsightContext) -> Kind {
        explain(d, c).unwrap().kind
    }

    #[test]
    fn nothing_to_say_unless_transferring() {
        let mut d = view();
        d.status = Status::Paused;
        assert!(explain(&d, &cx()).is_none());
        d.status = Status::Connecting;
        assert_eq!(kind(&d, &cx()), Kind::Starting);
    }

    #[test]
    fn limits_are_named_and_removable() {
        let mut d = view();
        d.options.speed_limit = 5_500_000;
        let i = explain(&d, &cx()).unwrap();
        assert_eq!((i.kind, i.fix), (Kind::DownloadLimit, Some(Fix::RemoveDownloadLimit)));

        d.options.speed_limit = 0;
        let c = InsightContext { global_limit: 5_500_000, ..cx() };
        assert_eq!(explain(&d, &c).unwrap().fix, Some(Fix::RemoveGlobalLimit));

        // A time-of-day limit can't be removed from here.
        let c = InsightContext { scheduled_limit: Some(5_500_000), ..cx() };
        assert_eq!(explain(&d, &c).unwrap().fix, None);

        // Far below the limit: not the limit's doing.
        let c = InsightContext { global_limit: 50_000_000, ..cx() };
        assert_eq!(kind(&d, &c), Kind::Healthy);
    }

    #[test]
    fn server_side_causes() {
        let mut d = view();
        d.resumable = false;
        assert_eq!(kind(&d, &cx()), Kind::SingleStream);

        let mut d = view();
        d.connections = 3;
        assert_eq!(kind(&d, &cx()), Kind::ConnectionCap);

        let mut d = view();
        d.reconnecting = 5;
        assert_eq!(kind(&d, &cx()), Kind::Unstable);
    }

    #[test]
    fn stalls_and_sharing() {
        let mut d = view();
        d.speed = 0.0;
        d.history = vec![0.0; 12];
        assert_eq!(kind(&d, &cx()), Kind::Stalled);
        d.reconnecting = 2;
        assert_eq!(kind(&d, &cx()), Kind::Unstable);

        let d = view();
        let c = InsightContext { active_downloads: 3, aggregate_speed: 20e6, ..cx() };
        assert_eq!(kind(&d, &c), Kind::SharedLine);
    }

    #[test]
    fn compares_with_the_hosts_history() {
        let d = view();
        let slow = InsightContext { host_average: Some(20e6), ..cx() };
        assert_eq!(kind(&d, &slow), Kind::SlowerThanUsual);
        let fast = InsightContext { host_average: Some(2e6), ..cx() };
        assert_eq!(kind(&d, &fast), Kind::FasterThanUsual);
        // Too early to compare.
        let mut young = view();
        young.active_secs = 2;
        assert_eq!(kind(&young, &slow), Kind::Healthy);
    }

    #[test]
    fn finishing_and_healthy() {
        let mut d = view();
        d.downloaded = 980;
        assert_eq!(kind(&d, &cx()), Kind::Finishing);
        assert_eq!(kind(&view(), &cx()), Kind::Healthy);
    }
}
