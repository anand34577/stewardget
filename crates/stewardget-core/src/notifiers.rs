//! Messages to other devices: Gotify, ntfy and email (SMTP). Any number of
//! destinations, each with its own service, the events it wants and, if
//! wanted, only some queues. Sent by the engine, so they also work without the
//! window (`stewardget serve`). A failed send never affects a download.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::QueueId;

/// Where messages go. Empty until the user adds a destination.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteNotify {
    pub targets: Vec<Target>,
}

/// Something that happened that a destination can ask to hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A download of `queue` finished.
    Finished { queue: QueueId },
    /// A download of `queue` failed for good (automatic retries used up).
    Failed { queue: QueueId },
    /// The last waiting or running download of `queue` came to an end.
    QueueFinished { queue: QueueId },
    /// Nothing is waiting or running any more, in any queue.
    AllFinished,
}

impl RemoteNotify {
    /// A free id for a new destination.
    pub fn next_id(&self) -> u32 {
        self.targets.iter().map(|t| t.id).max().map_or(1, |m| m + 1)
    }

    /// Who gets what for these simultaneous events. A destination that wants
    /// "all finished" doesn't also get "queue finished" for the same moment.
    pub fn plan(&self, events: &[(Event, Message)]) -> Vec<(Target, Message)> {
        let everything_done = events.iter().any(|(e, _)| *e == Event::AllFinished);
        let mut out = Vec::new();
        for t in self.targets.iter().filter(|t| t.ready()) {
            let gets_all = everything_done && t.wants(&Event::AllFinished);
            for (event, message) in events {
                if t.wants(event) && !(gets_all && matches!(event, Event::QueueFinished { .. })) {
                    out.push((t.clone(), message.clone()));
                }
            }
        }
        out
    }

    /// The same settings without passwords and tokens, for backups and reports.
    pub fn without_secrets(&self) -> Self {
        let mut c = self.clone();
        for t in &mut c.targets {
            t.service.clear_secret();
        }
        c
    }

    /// Takes the passwords and tokens of matching destinations from `other`
    /// (this computer's settings), keeping everything else.
    pub fn keep_secrets_of(&mut self, other: &Self) {
        for t in &mut self.targets {
            if let Some(mine) = other.targets.iter().find(|o| o.id == t.id && o.service.kind() == t.service.kind()) {
                t.service.set_secret(mine.service.secret().to_owned());
            }
        }
    }
}

/// One destination: a service, what to send there and for which queues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    pub id: u32,
    /// The user's name for it, e.g. "My phone" or "Team mailbox".
    pub name: String,
    pub enabled: bool,
    pub service: Service,
    pub events: Events,
    /// Only downloads of these queues; empty means every queue.
    pub queues: Vec<QueueId>,
    /// Events that differ from `events` for one queue (set in that queue's settings).
    pub queue_events: Vec<QueueEvents>,
}

/// What a destination is told about one queue, in place of its usual events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEvents {
    pub queue: QueueId,
    pub events: Events,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            enabled: true,
            service: Service::Ntfy(Ntfy::default()),
            events: Events::default(),
            queues: Vec::new(),
            queue_events: Vec::new(),
        }
    }
}

impl Target {
    pub fn new(id: u32, service: Service) -> Self {
        Self { id, name: service.kind().name().to_owned(), service, ..Self::default() }
    }

    /// Switched on and filled in well enough to try.
    pub fn ready(&self) -> bool {
        self.enabled && self.service.filled_in()
    }

    /// The events this destination hears about for `queue`.
    pub fn events_for(&self, queue: QueueId) -> Events {
        self.queue_events.iter().find(|q| q.queue == queue).map_or(self.events, |q| q.events)
    }

    pub fn wants(&self, event: &Event) -> bool {
        let queue_ok = |q: &QueueId| self.queues.is_empty() || self.queues.contains(q);
        match event {
            Event::Finished { queue } => self.events_for(*queue).finished && queue_ok(queue),
            Event::Failed { queue } => self.events_for(*queue).failed && queue_ok(queue),
            Event::QueueFinished { queue } => self.events_for(*queue).queue_finished && queue_ok(queue),
            Event::AllFinished => self.events.all_finished,
        }
    }
}

/// Which events a destination is told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Events {
    pub finished: bool,
    pub failed: bool,
    pub queue_finished: bool,
    pub all_finished: bool,
}

impl Default for Events {
    fn default() -> Self {
        Self { finished: true, failed: true, queue_finished: false, all_finished: false }
    }
}

/// The service a destination uses, with its settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Service {
    Gotify(Gotify),
    Ntfy(Ntfy),
    Email(Email),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Gotify,
    Ntfy,
    Email,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Gotify, Kind::Ntfy, Kind::Email];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Gotify => "Gotify",
            Kind::Ntfy => "ntfy",
            Kind::Email => "Email",
        }
    }

    /// A new, empty service of this kind.
    pub fn service(self) -> Service {
        match self {
            Kind::Gotify => Service::Gotify(Gotify::default()),
            Kind::Ntfy => Service::Ntfy(Ntfy::default()),
            Kind::Email => Service::Email(Email::default()),
        }
    }
}

impl Service {
    pub fn kind(&self) -> Kind {
        match self {
            Service::Gotify(_) => Kind::Gotify,
            Service::Ntfy(_) => Kind::Ntfy,
            Service::Email(_) => Kind::Email,
        }
    }

    fn filled_in(&self) -> bool {
        match self {
            Service::Gotify(g) => !g.url.trim().is_empty() && !g.token.trim().is_empty(),
            Service::Ntfy(n) => !n.topic.trim().is_empty(),
            Service::Email(e) => !e.host.trim().is_empty() && !e.to.trim().is_empty(),
        }
    }

    fn secret(&self) -> &str {
        match self {
            Service::Gotify(g) => &g.token,
            Service::Ntfy(n) => &n.token,
            Service::Email(e) => &e.password,
        }
    }

    fn set_secret(&mut self, secret: String) {
        match self {
            Service::Gotify(g) => g.token = secret,
            Service::Ntfy(n) => n.token = secret,
            Service::Email(e) => e.password = secret,
        }
    }

    fn clear_secret(&mut self) {
        self.set_secret(String::new());
    }
}

/// A Gotify server (self-hosted push notifications).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gotify {
    pub enabled: bool,
    /// Server address, e.g. `https://gotify.example.com`.
    pub url: String,
    /// Application token created in Gotify.
    pub token: String,
    /// 0–10; 5 and up usually make a sound on the phone.
    pub priority: u8,
}

impl Default for Gotify {
    fn default() -> Self {
        Self { enabled: false, url: String::new(), token: String::new(), priority: 5 }
    }
}

/// An ntfy topic (ntfy.sh or a self-hosted server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ntfy {
    pub enabled: bool,
    /// Server address; `https://ntfy.sh` unless self-hosted.
    pub server: String,
    pub topic: String,
    /// Access token for protected topics; empty for public ones.
    pub token: String,
}

impl Default for Ntfy {
    fn default() -> Self {
        Self { enabled: false, server: "https://ntfy.sh".to_owned(), topic: String::new(), token: String::new() }
    }
}

/// How the connection to the mail server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Plain connection upgraded with STARTTLS (usually port 587).
    #[default]
    StartTls,
    /// TLS from the start (usually port 465).
    Tls,
    /// No encryption: only for a server on this computer or the local network.
    None,
}

/// Email through an SMTP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Email {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// Login; empty when the server doesn't need one.
    pub username: String,
    pub password: String,
    /// Sender address; the login is used when empty.
    pub from: String,
    /// One or more recipients, separated by commas.
    pub to: String,
}

impl Default for Email {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 587,
            security: Security::StartTls,
            username: String::new(),
            password: String::new(),
            from: String::new(),
            to: String::new(),
        }
    }
}

/// What a message says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// A failure, sent with a higher priority where the service has one.
    pub problem: bool,
}

const TIMEOUT: Duration = Duration::from_secs(20);

/// Sends `message` to one destination.
pub async fn send(target: &Target, message: &Message) -> Result<(), String> {
    match &target.service {
        Service::Gotify(g) => send_gotify(g, message).await,
        Service::Ntfy(n) => send_ntfy(n, message).await,
        Service::Email(e) => send_email(e, message).await,
    }
}

/// Sends each planned message; returns the destinations that failed.
pub async fn send_planned(plan: Vec<(Target, Message)>) -> Vec<(String, String)> {
    let mut failed = Vec::new();
    for (target, message) in plan {
        if let Err(e) = send(&target, &message).await {
            failed.push((target.name.clone(), e));
        }
    }
    failed
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("StewardGet/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Turns a non-success response into a short error with the server's own words.
async fn check(response: reqwest::Response) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let text = response.text().await.unwrap_or_default();
    let detail: String = text.trim().chars().take(200).collect();
    Err(if detail.is_empty() { format!("HTTP {status}") } else { format!("HTTP {status}: {detail}") })
}

fn base_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("the server address must start with http:// or https://".to_owned());
    }
    Ok(url.to_owned())
}

async fn send_gotify(g: &Gotify, m: &Message) -> Result<(), String> {
    let url = format!("{}/message", base_url(&g.url)?);
    let priority = if m.problem { g.priority.saturating_add(2).min(10) } else { g.priority.min(10) };
    let body = serde_json::json!({ "title": m.title, "message": m.body, "priority": priority });
    let response = client()?
        .post(url)
        .header("X-Gotify-Key", g.token.trim())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    check(response).await
}

async fn send_ntfy(n: &Ntfy, m: &Message) -> Result<(), String> {
    let server = if n.server.trim().is_empty() { "https://ntfy.sh" } else { n.server.as_str() };
    let topic = n.topic.trim().trim_matches('/');
    if topic.is_empty() || topic.contains('/') {
        return Err("the topic must be a single name, like stewardget-alerts".to_owned());
    }
    let mut request = client()?
        .post(format!("{}/{topic}", base_url(server)?))
        // Header values must be plain ASCII.
        .header("Title", ascii_header(&m.title))
        .header("Tags", if m.problem { "warning" } else { "white_check_mark" })
        .header("Priority", if m.problem { "high" } else { "default" })
        .body(m.body.clone());
    if !n.token.trim().is_empty() {
        request = request.bearer_auth(n.token.trim());
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    check(response).await
}

/// `text` with anything a header can't carry replaced, so a file name with
/// accents or emoji can't make the request fail.
fn ascii_header(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '?' }).collect()
}

async fn send_email(e: &Email, m: &Message) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let host = e.host.trim();
    if host.is_empty() {
        return Err("no mail server is set".to_owned());
    }
    let from_text = if e.from.trim().is_empty() { e.username.trim() } else { e.from.trim() };
    let from: lettre::message::Mailbox =
        from_text.parse().map_err(|_| format!("\"{from_text}\" is not an email address (sender)"))?;
    let mut builder = lettre::Message::builder().from(from).subject(&m.title);
    let mut any = false;
    for to in e.to.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()) {
        builder = builder.to(to.parse().map_err(|_| format!("\"{to}\" is not an email address (recipient)"))?);
        any = true;
    }
    if !any {
        return Err("no recipient is set".to_owned());
    }
    let mail = builder.header(ContentType::TEXT_PLAIN).body(m.body.clone()).map_err(|e| e.to_string())?;

    let transport = match e.security {
        Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
        Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
        Security::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)),
    }
    .map_err(|e| e.to_string())?
    .port(e.port)
    .timeout(Some(TIMEOUT));
    let transport = if e.username.trim().is_empty() {
        transport
    } else {
        transport.credentials(Credentials::new(e.username.trim().to_owned(), e.password.clone()))
    };
    transport.build().send(mail).await.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ntfy(id: u32, queues: Vec<QueueId>, events: Events) -> Target {
        let service = Service::Ntfy(Ntfy { topic: format!("t{id}"), ..Ntfy::default() });
        Target { id, queues, events, ..Target::new(id, service) }
    }

    fn msg(title: &str) -> Message {
        Message { title: title.into(), body: String::new(), problem: false }
    }

    #[test]
    fn a_queue_can_change_a_destinations_events() {
        let mut t = ntfy(1, vec![], Events::default());
        t.queue_events.push(QueueEvents { queue: 2, events: Events { finished: false, ..Events::default() } });
        assert!(t.wants(&Event::Finished { queue: 1 }));
        assert!(!t.wants(&Event::Finished { queue: 2 }));
        assert!(t.wants(&Event::Failed { queue: 2 }));
    }

    #[test]
    fn destinations_get_only_their_events_and_queues() {
        let all_events = Events { finished: true, failed: true, queue_finished: true, all_finished: true };
        let config = RemoteNotify {
            targets: vec![
                ntfy(1, vec![], Events::default()),
                ntfy(2, vec![7], all_events),
                Target { enabled: false, ..ntfy(3, vec![], all_events) },
                Target::new(4, Kind::Gotify.service()), // not filled in
            ],
        };
        let plan = config.plan(&[(Event::Finished { queue: 0 }, msg("done"))]);
        assert_eq!(plan.iter().map(|(t, _)| t.id).collect::<Vec<_>>(), vec![1]);
        let plan = config.plan(&[(Event::Failed { queue: 7 }, msg("failed"))]);
        assert_eq!(plan.iter().map(|(t, _)| t.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn all_finished_replaces_queue_finished() {
        let events = Events { finished: false, failed: false, queue_finished: true, all_finished: true };
        let only_queue = Events { all_finished: false, ..events };
        let config = RemoteNotify { targets: vec![ntfy(1, vec![], events), ntfy(2, vec![], only_queue)] };
        let plan = config.plan(&[(Event::QueueFinished { queue: 0 }, msg("queue")), (Event::AllFinished, msg("all"))]);
        let got: Vec<(u32, &str)> = plan.iter().map(|(t, m)| (t.id, m.title.as_str())).collect();
        assert_eq!(got, vec![(1, "all"), (2, "queue")]);
    }

    #[test]
    fn secrets_stay_out_of_copies() {
        let g = Gotify { token: "g".into(), ..Gotify::default() };
        let e = Email { password: "p".into(), ..Email::default() };
        let config =
            RemoteNotify { targets: vec![Target::new(1, Service::Gotify(g)), Target::new(2, Service::Email(e))] };
        let shared = config.without_secrets();
        assert!(shared.targets.iter().all(|t| t.service.secret().is_empty()));
        let mut restored = shared.clone();
        restored.keep_secrets_of(&config);
        assert_eq!(restored, config);
    }

    #[test]
    fn settings_round_trip() {
        let config = RemoteNotify { targets: vec![ntfy(1, vec![2, 3], Events::default())] };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"kind\":\"ntfy\""));
        assert_eq!(serde_json::from_str::<RemoteNotify>(&json).unwrap(), config);
    }

    #[test]
    fn header_text_is_ascii() {
        assert_eq!(ascii_header("Fin\u{00e9} \u{2713}"), "Fin? ?");
    }

    #[tokio::test]
    async fn bad_addresses_are_explained() {
        let n = Ntfy { enabled: true, server: "ntfy.sh".into(), topic: "x".into(), token: String::new() };
        let m = Message { title: "t".into(), body: "b".into(), problem: false };
        assert!(send_ntfy(&n, &m).await.unwrap_err().contains("http"));
        let e = Email {
            enabled: true,
            host: "smtp.example.com".into(),
            to: "not an address".into(),
            from: "a@b.c".into(),
            ..Email::default()
        };
        assert!(send_email(&e, &m).await.unwrap_err().contains("recipient"));
    }
}
