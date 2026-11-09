//! The access state machine: what is on, who is paired, and the one-shot
//! invitations. This module is the SINGLE source of the state vocabulary
//! (`off`/`awaiting`/`connected`); every other module and the frontend
//! mirror consume it through [`LanState`] instead of spelling the token
//! (quality return R1).
//!
//! Two facts are kept apart on purpose:
//!   * `on` — the listener is bound and serving (the access is available);
//!   * the live session set — a phone has actually shown itself recently.
//! `state` is DERIVED from both, so the window can never say «подключено»
//! while no device is reachable and can never say «выключен» while the
//! listener serves (REQ-LAN-01 AC4, REQ-LAN-09 AC5).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::commands::UiError;

/// A session that has been silent longer than this is no longer shown as
/// «подключено» (ratified Q5: not later than a minute). The session itself
/// stays valid — a sleeping phone must not need a new code (ratified Q3).
pub const LIVE_WINDOW: chrono::Duration = chrono::Duration::seconds(60);

const INVITE_TOKEN_BYTES: usize = 32;
const SESSION_SECRET_BYTES: usize = 32;

/// The QR-render failure text: when the invitation exists but its QR did not
/// render, the window says so instead of showing an empty slot that reads as
/// «приглашение использовано» (quality return R8).
const QR_RENDER_FAILED: &str = "QR не построился — откройте адрес с телефона вручную";

/// The FACT the window renders. `Off` = no listener; `Awaiting` = listening,
/// no live phone; `Connected` = listening and at least one phone is live.
///
/// The variant is serialized lower-case, which is exactly the wire token a
/// second consumer reads — and the token `LanState::token` pins for tests.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LanState {
    Off,
    Awaiting,
    Connected,
}

impl LanState {
    /// Every state the surface can publish. The parity test asserts that
    /// this list is declared in the frontend mirror too, so a state added
    /// here cannot silently stay unhandled there.
    #[allow(dead_code)] // consumed by the parity test, not at runtime
    pub const ALL: &'static [LanState] = &[LanState::Off, LanState::Awaiting, LanState::Connected];

    /// The wire token of the variant. This is the only place in the crate
    /// where the literals live; every comparison in the product code goes
    /// through the enum (the parity test mechanically enforces that).
    #[allow(dead_code)] // consumed by the parity test, not at runtime
    pub fn token(self) -> &'static str {
        match self {
            LanState::Off => "off",
            LanState::Awaiting => "awaiting",
            LanState::Connected => "connected",
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct LanDevice {
    /// Stable session id, so the window can disconnect ONE device.
    pub id: String,
    /// Client-declared name (its User-Agent platform block) — describes the
    /// source, it does not authenticate anybody (REQ-LAN-06 constraint).
    pub name: String,
    pub addr: String,
    pub connected_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub live: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct LanStatus {
    /// Derived fact, never an intention flag (REQ-LAN-01 AC4).
    pub state: LanState,
    /// `ip:port` of the bound listener; `None` while off.
    pub address: Option<String>,
    /// Whether an UNUSED invitation exists. This is what separates «приглашение
    /// использовано» (false, no QR) from «QR не отрендерился» (true + qr_error).
    pub invite_active: bool,
    /// One-time pairing URL for a browser (carries the token); only while
    /// awaiting. Kept visible as text next to the QR, so a phone WITHOUT the
    /// app still opens the same interface (REQ-QRAPP-01 AC3, REQ-QRAPP-03).
    pub url: Option<String>,
    /// The SAME invitation as a `devai://pair` link — the address the QR
    /// encodes, so Android hands the scan to our app instead of a browser
    /// (REQ-QRAPP-01 AC2). Built from the same token and address as `url`.
    pub app_url: Option<String>,
    /// The invitation token itself, shown until a phone pairs; `None` after.
    pub token: Option<String>,
    /// Rendered QR of `app_url` (SVG markup). The phone scans it with its
    /// camera and our application opens (REQ-QRAPP-02).
    pub qr_svg: Option<String>,
    /// Why the QR could not be rendered — distinct from a used invitation.
    pub qr_error: Option<String>,
    /// Machine reason when the access is off after a refusal (e.g.
    /// `no_local_address`, `network_changed`, `bind_failed`).
    pub reason_code: Option<String>,
    /// Human wording of `reason_code`.
    pub reason: Option<String>,
    /// The «was on» flag could not be written: the operator must SEE that the
    /// next launch will not restore the access (quality return R3). Never
    /// carries a token or a session secret.
    pub persist_error: Option<String>,
    pub devices: Vec<LanDevice>,
    pub started_at: Option<DateTime<Utc>>,
}

impl LanStatus {
    fn off(reason: Option<(&'static str, String)>, persist_error: Option<String>) -> LanStatus {
        LanStatus {
            state: LanState::Off,
            address: None,
            invite_active: false,
            url: None,
            app_url: None,
            token: None,
            qr_svg: None,
            qr_error: None,
            reason_code: reason.as_ref().map(|(c, _)| c.to_string()),
            reason: reason.map(|(_, m)| m),
            persist_error,
            devices: Vec::new(),
            started_at: None,
        }
    }
}

/// What the HTTP boundary claims about the client. Nothing here is proof of
/// identity: the name is what the client says, the address is what the socket
/// says (REQ-LAN-06 constraint).
#[derive(Clone, Debug)]
pub struct DeviceClaim {
    pub name: String,
    pub addr: String,
}

impl DeviceClaim {
    pub fn new(user_agent: &str, peer: SocketAddr) -> DeviceClaim {
        DeviceClaim {
            name: device_name(user_agent),
            addr: peer.ip().to_string(),
        }
    }
}

/// The device part of the actor string a LAN write is recorded with. The base
/// actor stays the person; the suffix only names the source (REQ-LAN-06 AC1-3,
/// AC5). `actor_marker` is the single producer of this shape.
pub fn actor_marker(base: &str, device: &DeviceClaim) -> String {
    format!("{base} · телефон: {} ({})", device.name, device.addr)
}

/// The client-claimed platform block of the User-Agent («заявленное клиентом
/// имя»), truncated so a hostile header cannot bloat the stored history.
fn device_name(user_agent: &str) -> String {
    let ua = user_agent.trim();
    if ua.is_empty() {
        return "неизвестное устройство".into();
    }
    let block = match (ua.find('('), ua.find(')')) {
        (Some(start), Some(end)) if end > start => &ua[start + 1..end],
        _ => ua,
    };
    truncate_chars(block, 80)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

struct Invite {
    token: String,
}

struct Session {
    id: String,
    secret: String,
    name: String,
    addr: String,
    connected_at: DateTime<Utc>,
    last_seen: DateTime<Utc>,
}

impl Session {
    fn view(&self, now: DateTime<Utc>) -> LanDevice {
        LanDevice {
            id: self.id.clone(),
            name: self.name.clone(),
            addr: self.addr.clone(),
            connected_at: self.connected_at,
            last_seen: self.last_seen,
            live: now - self.last_seen < LIVE_WINDOW,
        }
    }
}

/// A port on the machine that is actually listening. The listener is bound by
/// `start_access` BEFORE the state claims «включён», so the window can never
/// show a state the phone cannot reach (REQ-LAN-01 AC2/AC4).
struct Bound {
    shutdown: tokio::sync::watch::Sender<bool>,
}

struct Inner {
    /// `false` once the access is off; the invite/sessions only live while on.
    on: bool,
    address: Option<SocketAddr>,
    invite: Option<Invite>,
    sessions: Vec<Session>,
    started_at: Option<DateTime<Utc>>,
    reason: Option<(&'static str, String)>,
    bound: Option<Bound>,
    /// The id of the current run; a watchdog from an older run must not
    /// touch a newer state.
    generation: u64,
    /// Visible persistence failure of the «was on» flag (R3).
    persist_error: Option<String>,
}

/// The access state machine + the persistence of the «was on» flag (Q1).
/// Everything the HTTP boundary decides about authentication goes through
/// this type, so the rules are unit-testable without a Tauri app or sockets.
pub struct LanAccess {
    inner: Mutex<Inner>,
    /// `<control home>/lan-access.json`; holds the ENABLED flag only — never
    /// a token and never a session secret.
    state_file: PathBuf,
    counter: AtomicU64,
}

impl LanAccess {
    pub fn new(state_file: PathBuf) -> LanAccess {
        LanAccess {
            inner: Mutex::new(Inner {
                on: false,
                address: None,
                invite: None,
                sessions: Vec::new(),
                started_at: None,
                reason: None,
                bound: None,
                generation: 0,
                persist_error: None,
            }),
            state_file,
            counter: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The persisted «was on» fact (ratified Q1). A missing/garbled file is
    /// simply «off»: the platform does not invent an enabled access.
    pub fn persisted_enabled(&self) -> bool {
        std::fs::read_to_string(&self.state_file)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|doc| doc.get("enabled").and_then(|v| v.as_bool()))
            .unwrap_or(false)
    }

    /// Write the «was on» flag. EVERY step is a typed failure the caller must
    /// surface (R3): a swallowed `create_dir_all`/`write`/`rename` left the
    /// operator believing the access would come back after a restart.
    fn persist(&self, enabled: bool) -> Result<(), String> {
        let dir = self
            .state_file
            .parent()
            .ok_or_else(|| "у файла состояния нет каталога".to_string())?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("не удалось создать каталог {}: {e}", dir.display()))?;
        let body = serde_json::json!({ "enabled": enabled });
        let tmp = self.state_file.with_extension("json.tmp");
        std::fs::write(&tmp, body.to_string())
            .map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.state_file)
            .map_err(|e| format!("не удалось сохранить {}: {e}", self.state_file.display()))
    }

    /// Persist and keep the outcome on the visible state. The flag never
    /// carries a token, so the failure text is safe to show and to log.
    fn record_persist(&self, inner: &mut Inner, enabled: bool) {
        match self.persist(enabled) {
            Ok(()) => inner.persist_error = None,
            Err(message) => {
                eprintln!("devai-ui: настройка доступа не сохранена: {message}");
                inner.persist_error = Some(message);
            }
        }
    }

    pub fn is_on(&self) -> bool {
        self.lock().on
    }

    #[allow(dead_code)] // read by the network-change test
    pub fn address(&self) -> Option<SocketAddr> {
        self.lock().address
    }

    /// The fact the window renders. `state` is derived (R11 / REQ-LAN-01 AC4,
    /// REQ-LAN-09 AC5): «подключено» only while a phone is actually live;
    /// a silent session keeps the access «ждёт подключения» and its row says
    /// «не отвечает» — the two directions of the window/availability match.
    pub fn status(&self) -> LanStatus {
        let inner = self.lock();
        if !inner.on {
            return LanStatus::off(inner.reason.clone(), inner.persist_error.clone());
        }
        let now = Utc::now();
        let devices: Vec<LanDevice> = inner.sessions.iter().map(|s| s.view(now)).collect();
        let state = if devices.iter().any(|d| d.live) {
            LanState::Connected
        } else {
            LanState::Awaiting
        };
        let token = inner.invite.as_ref().map(|i| i.token.clone());
        // Both addresses of ONE invitation (REQ-QRAPP-01 constraint): the
        // browser URL and the app link are derived from the same address and
        // the same one-time code, so they cannot disagree. The QR encodes the
        // app link — Android hands `devai://pair` to our APK, not to a browser
        // (REQ-QRAPP-02).
        let (url, app_url) = invite_addresses(token.as_deref(), inner.address);
        let (qr_svg, qr_error) = qr_presentation(app_url.as_deref());
        LanStatus {
            state,
            address: inner.address.map(|a| a.to_string()),
            invite_active: token.is_some(),
            url,
            app_url,
            token,
            qr_svg,
            qr_error,
            reason_code: None,
            reason: None,
            persist_error: inner.persist_error.clone(),
            devices,
            started_at: inner.started_at,
        }
    }

    /// Install a live listener: the state becomes «включён» only here, after
    /// the bind succeeded (REQ-LAN-01 AC2). A fresh invitation is issued
    /// (one-time per pairing, REQ-LAN-04).
    pub fn activate(
        &self,
        address: SocketAddr,
        shutdown: tokio::sync::watch::Sender<bool>,
    ) -> String {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.on = true;
        inner.address = Some(address);
        inner.reason = None;
        inner.started_at = Some(Utc::now());
        inner.sessions.clear();
        inner.bound = Some(Bound { shutdown });
        let token = fresh_secret(&self.counter, INVITE_TOKEN_BYTES);
        inner.invite = Some(Invite {
            token: token.clone(),
        });
        self.record_persist(&mut inner, true);
        token
    }

    /// Refuse to turn the access on, naming the cause; the window stays
    /// «выключен» and does not pretend (REQ-LAN-01 AC5, REQ-LAN-03 AC4).
    pub fn refuse(&self, code: &'static str, message: impl Into<String>) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.on = false;
        inner.address = None;
        inner.invite = None;
        inner.sessions.clear();
        inner.bound = None;
        inner.reason = Some((code, message.into()));
        self.record_persist(&mut inner, false);
    }

    /// Turn the access off (the «отключить» button). The state is already off
    /// when the listener is told to stop, so the next request gets a refusal
    /// even if the socket drains slowly (REQ-LAN-04 AC6, REQ-LAN-09 AC4).
    pub fn deactivate(&self) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.on = false;
        inner.address = None;
        inner.invite = None;
        inner.sessions.clear();
        inner.reason = None;
        let bound = inner.bound.take();
        self.record_persist(&mut inner, false);
        // The listener is told to stop AFTER the state is off: a request that
        // races the shutdown already sees a closed door.
        if let Some(bound) = bound {
            let _ = bound.shutdown.send(true);
        }
    }

    /// Ratified Q2: a new invitation retires the PREVIOUS invitation only.
    /// Already paired phones keep their sessions.
    pub fn new_invite(&self) -> Result<String, UiError> {
        let mut inner = self.lock();
        if !inner.on {
            return Err(UiError {
                code: "lan_off".into(),
                message: "доступ с телефона выключен".into(),
            });
        }
        let token = fresh_secret(&self.counter, INVITE_TOKEN_BYTES);
        inner.invite = Some(Invite {
            token: token.clone(),
        });
        Ok(token)
    }

    /// Disconnect ONE paired device by its session id.
    pub fn disconnect(&self, id: &str) -> bool {
        let mut inner = self.lock();
        let before = inner.sessions.len();
        inner.sessions.retain(|s| s.id != id);
        before != inner.sessions.len()
    }

    /// Page load from a browser. A valid session cookie wins (a reload does
    /// not need a new code — ratified Q3); otherwise a matching invitation is
    /// CONSUMED and becomes exactly one session (REQ-LAN-04 AC4).
    pub fn page_load(
        &self,
        token: Option<&str>,
        cookie: Option<&str>,
        device: &DeviceClaim,
    ) -> PageAuth {
        let mut inner = self.lock();
        if !inner.on {
            return PageAuth::Deny {
                code: "lan_off",
                message: "доступ с телефона выключен".into(),
            };
        }
        if let Some(secret) = cookie.filter(|s| !s.is_empty()) {
            if let Some(session) = inner
                .sessions
                .iter_mut()
                .find(|s| secret_eq(&s.secret, secret))
            {
                session.last_seen = Utc::now();
                return PageAuth::Allow { set_cookie: None };
            }
        }
        self.pair(&mut inner, token, device)
    }

    /// Consume the invitation into exactly one session. The code is removed
    /// BEFORE the session is recorded, so a racing second load cannot use it.
    fn pair(&self, inner: &mut Inner, token: Option<&str>, device: &DeviceClaim) -> PageAuth {
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return PageAuth::Deny {
                code: "lan_token_required",
                message: "нужен код приглашения".into(),
            };
        };
        let valid = inner
            .invite
            .as_ref()
            .is_some_and(|i| secret_eq(&i.token, token));
        if !valid {
            return PageAuth::Deny {
                code: "lan_token_invalid",
                message: "код приглашения недействителен или уже использован".into(),
            };
        }
        inner.invite = None;
        let now = Utc::now();
        let secret = fresh_secret(&self.counter, SESSION_SECRET_BYTES);
        let id = fresh_secret(&self.counter, 8);
        inner.sessions.push(Session {
            id,
            secret: secret.clone(),
            name: device.name.clone(),
            addr: device.addr.clone(),
            connected_at: now,
            last_seen: now,
        });
        PageAuth::Allow {
            set_cookie: Some(secret),
        }
    }

    /// Authenticate an RPC call by its session cookie. A missing/unknown
    /// secret is a refusal and nothing is served (REQ-LAN-04 AC1-3).
    pub fn touch_session(&self, cookie: Option<&str>, device: &DeviceClaim) -> Option<DeviceClaim> {
        let mut inner = self.lock();
        if !inner.on {
            return None;
        }
        let secret = cookie?;
        let session = inner
            .sessions
            .iter_mut()
            .find(|s| secret_eq(&s.secret, secret))?;
        session.last_seen = Utc::now();
        Some(DeviceClaim {
            name: session.name.clone(),
            addr: device.addr.clone(),
        })
    }

    /// The phone's health question (REQ-QRAPP-05/06). It never pairs and never
    /// spends the code: it reports one of four facts, and a live session is
    /// kept live by the question itself.
    pub fn pair_state(&self, token: Option<&str>, cookie: Option<&str>) -> PairState {
        let mut inner = self.lock();
        if !inner.on {
            return PairState::Off;
        }
        if let Some(secret) = cookie.filter(|s| !s.is_empty()) {
            if let Some(session) = inner
                .sessions
                .iter_mut()
                .find(|s| secret_eq(&s.secret, secret))
            {
                session.last_seen = Utc::now();
                return PairState::Paired;
            }
        }
        match token.filter(|t| !t.is_empty()) {
            Some(token) => {
                if inner
                    .invite
                    .as_ref()
                    .is_some_and(|i| secret_eq(&i.token, token))
                {
                    PairState::Invite
                } else {
                    PairState::Refused("lan_token_invalid")
                }
            }
            None => PairState::Refused("lan_token_required"),
        }
    }

    /// The network watchdog (REQ-LAN-08): if the bound address is no longer
    /// one of the machine's addresses, the access stops HONESTLY — off, with
    /// the reason — and is never re-bound to the new network by itself.
    /// Returns true when it stopped.
    pub fn stop_if_network_changed(&self, current: &[std::net::Ipv4Addr]) -> bool {
        let (changed, shutdown) = {
            let mut inner = self.lock();
            let Some(addr) = inner.address else {
                return false;
            };
            if current
                .iter()
                .any(|ip| std::net::IpAddr::V4(*ip) == addr.ip())
            {
                return false;
            }
            inner.generation += 1;
            inner.on = false;
            inner.address = None;
            inner.invite = None;
            inner.sessions.clear();
            inner.reason = Some((
                "network_changed",
                "сеть изменилась: адрес машины пропал, доступ выключен".into(),
            ));
            let bound = inner.bound.take();
            self.record_persist(&mut inner, false);
            (true, bound.map(|b| b.shutdown))
        };
        if let Some(tx) = shutdown {
            let _ = tx.send(true);
        }
        changed
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }
}

/// The two addresses of ONE invitation: the browser URL (shown as text) and
/// the `devai://pair` link (encoded in the QR). Both come from the SAME
/// producers in [`crate::lan::pair`], so they cannot carry a different token
/// or a different machine — the constraint of REQ-QRAPP-01 is this function's
/// shape, not a convention (quality return R4).
fn invite_addresses(
    token: Option<&str>,
    addr: Option<SocketAddr>,
) -> (Option<String>, Option<String>) {
    match (token, addr) {
        (Some(token), Some(addr)) => (
            Some(crate::lan::pair::home_url(addr, Some(token))),
            Some(crate::lan::pair::app_link(addr, token)),
        ),
        _ => (None, None),
    }
}

/// The QR presentation of an invitation URL, kept apart from the state so the
/// two failure shapes are testable: a USED invitation has neither a QR nor an
/// error; an invitation whose QR failed has the error (R8).
fn qr_presentation(url: Option<&str>) -> (Option<String>, Option<String>) {
    match url {
        Some(url) => match qr_svg(url) {
            Some(svg) => (Some(svg), None),
            None => (None, Some(QR_RENDER_FAILED.to_string())),
        },
        None => (None, None),
    }
}

/// The fact `GET /pair` publishes: the phone's health question, answered
/// without any data. Kept apart from [`PageAuth`] because it REFUSES nothing
/// but a well-formed ask: «paired», «this code is valid and unused», «refused»
/// or «the door is off» (REQ-QRAPP-05, REQ-QRAPP-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairState {
    Paired,
    Invite,
    Refused(&'static str),
    Off,
}

pub enum PageAuth {
    Allow { set_cookie: Option<String> },
    Deny { code: &'static str, message: String },
}

/// Session/invite secrets are compared without an early exit, so a wrong
/// guess cannot be refined by timing.
fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// 32 random bytes from the OS, hex-encoded. The fallback keeps the process
/// working where `/dev/urandom` is absent; it is a last resort, not the rule.
fn fresh_secret(counter: &AtomicU64, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if read_urandom(&mut buf) {
        return hex(&buf);
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    let n = counter.fetch_add(1, Ordering::Relaxed);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    n.hash(&mut hasher);
    let seed = hasher.finish();
    for (i, slot) in buf.iter_mut().enumerate() {
        *slot = ((seed >> ((i % 8) * 8)) as u8) ^ (i as u8).wrapping_mul(31);
    }
    hex(&buf)
}

fn read_urandom(buf: &mut [u8]) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open("/dev/urandom") else {
        return false;
    };
    f.read_exact(buf).is_ok()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The invitation renders as an SVG so the frontend carries no QR library and
/// the code is crisp on a phone screen.
fn qr_svg(url: &str) -> Option<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(260, 260)
            .quiet_zone(true)
            .dark_color(svg::Color("#0f1115"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(name: &str) -> DeviceClaim {
        DeviceClaim {
            name: name.into(),
            addr: "192.168.1.42".into(),
        }
    }

    fn access() -> (LanAccess, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let lan = LanAccess::new(dir.path().join("lan-access.json"));
        (lan, dir)
    }

    fn activate(lan: &LanAccess) -> (String, tokio::sync::watch::Receiver<bool>) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let addr: SocketAddr = "192.168.1.20:4567".parse().unwrap();
        let token = lan.activate(addr, tx);
        (token, rx)
    }

    fn frontend(path: &str) -> String {
        std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// The quoted tokens inside `LAN_STATES = [ … ]`.
    fn declared_states(source: &str) -> Vec<&str> {
        let start = source.find("LAN_STATES = [").expect("LAN_STATES");
        let end = start + source[start..].find(']').expect("closing bracket");
        source[start..end].split('"').skip(1).step_by(2).collect()
    }

    /// R1: the state vocabulary is declared in the frontend mirror, and the
    /// product code outside this module never spells a state token. A state
    /// added here without the TS mirror (or a literal creeping back into a
    /// comparison) fails HERE instead of at runtime.
    #[test]
    fn every_state_is_declared_across_the_wire_and_named_only_in_this_module() {
        let lan_state_ts = frontend("frontend/src/lanState.ts");
        let expected: Vec<&str> = LanState::ALL.iter().map(|s| s.token()).collect();
        assert_eq!(
            declared_states(&lan_state_ts),
            expected,
            "the TS mirror drifted from LanState"
        );
        for state in LanState::ALL {
            assert!(
                lan_state_ts.contains(&format!("{}:", state.token())),
                "`{}` is not handled in lanState.ts",
                state.token()
            );
        }
        // No product module spells a state token outside this declaration
        // site. (Tests are cut off: they legitimately assert the wire names.)
        for file in [
            "src/lan/mod.rs",
            "src/lan/net.rs",
            "src/lan/server.rs",
            "src/lan/http.rs",
            "src/lan/instance.rs",
            "src/lan/commands.rs",
            "src/commands.rs",
            "src/lib.rs",
        ] {
            let source = frontend(file);
            let production = source.split("#[cfg(test)]").next().unwrap_or("");
            for state in LanState::ALL {
                let quoted = format!("\"{}\"", state.token());
                assert!(
                    !production.contains(&quoted),
                    "{file} names the state token {quoted} outside lan::state"
                );
            }
        }
    }

    #[test]
    fn the_off_state_serves_nothing_and_names_no_address() {
        let (lan, _dir) = access();
        let status = lan.status();
        assert_eq!(status.state, LanState::Off);
        assert!(status.address.is_none());
        assert!(status.url.is_none());
        assert!(status.devices.is_empty());
        assert!(status.persist_error.is_none());
        // Without an invitation the page is refused, not served.
        assert!(matches!(
            lan.page_load(None, None, &claims("Pixel")),
            PageAuth::Deny {
                code: "lan_off",
                ..
            }
        ));
        assert!(lan.touch_session(None, &claims("Pixel")).is_none());
    }

    #[test]
    fn an_invitation_pairs_exactly_one_device_and_then_disappears() {
        let (lan, _dir) = access();
        let (token, _rx) = activate(&lan);
        let first = lan.page_load(Some(&token), None, &claims("Pixel 7"));
        let PageAuth::Allow { set_cookie } = first else {
            panic!("the freshly issued invitation must pair");
        };
        let cookie = set_cookie.expect("pairing issues a session cookie");
        let status = lan.status();
        assert_eq!(status.state, LanState::Connected);
        assert!(
            status.token.is_none()
                && status.url.is_none()
                && status.app_url.is_none()
                && !status.invite_active,
            "a used invitation is not shown as active any more"
        );
        assert_eq!(status.devices.len(), 1);
        assert_eq!(status.devices[0].name, "Pixel 7");

        // The SAME code on a second device: refused (one-time per pairing).
        assert!(matches!(
            lan.page_load(Some(&token), None, &claims("другой телефон")),
            PageAuth::Deny {
                code: "lan_token_invalid",
                ..
            }
        ));
        // A reload with the session cookie needs no new code (ratified Q3).
        assert!(matches!(
            lan.page_load(None, Some(&cookie), &claims("Pixel 7")),
            PageAuth::Allow { set_cookie: None }
        ));
    }

    #[test]
    fn a_new_invitation_retires_the_previous_one_but_keeps_paired_phones() {
        let (lan, _dir) = access();
        let (first, _rx) = activate(&lan);
        let PageAuth::Allow {
            set_cookie: Some(cookie),
        } = lan.page_load(Some(&first), None, &claims("Pixel"))
        else {
            panic!("first pairing");
        };
        let second = lan.new_invite().expect("access is on");
        assert_ne!(first, second);
        assert!(
            matches!(
                lan.page_load(Some(&first), None, &claims("old code")),
                PageAuth::Deny { .. }
            ),
            "the retired invitation must not pair"
        );
        // Ratified Q2 «может»: the paired session survives a new invitation.
        assert!(lan.touch_session(Some(&cookie), &claims("Pixel")).is_some());
        assert_eq!(lan.status().devices.len(), 1);
        assert_eq!(lan.status().token.as_deref(), Some(second.as_str()));
        assert!(lan.status().invite_active);
    }

    #[test]
    fn several_phones_can_be_paired_one_invitation_each() {
        let (lan, _dir) = access();
        let (_token, _rx) = activate(&lan);
        let mut cookies = Vec::new();
        for name in ["Pixel", "Tablet"] {
            let token = lan.new_invite().unwrap();
            let PageAuth::Allow {
                set_cookie: Some(cookie),
            } = lan.page_load(Some(&token), None, &claims(name))
            else {
                panic!("pairing {name}");
            };
            cookies.push(cookie);
        }
        let status = lan.status();
        assert_eq!(status.devices.len(), 2);
        assert_eq!(status.state, LanState::Connected);
        for cookie in cookies {
            assert!(lan.touch_session(Some(&cookie), &claims("x")).is_some());
        }
    }

    #[test]
    fn disable_revokes_every_session_immediately() {
        let (lan, _dir) = access();
        let (token, _rx) = activate(&lan);
        let PageAuth::Allow {
            set_cookie: Some(cookie),
        } = lan.page_load(Some(&token), None, &claims("Pixel"))
        else {
            panic!("pairing");
        };
        lan.deactivate();
        assert!(!lan.is_on());
        assert_eq!(lan.status().state, LanState::Off);
        // Even the still-open page's cookie is now worthless.
        assert!(lan.touch_session(Some(&cookie), &claims("Pixel")).is_none());
        assert!(matches!(
            lan.page_load(Some(&cookie), Some(&cookie), &claims("Pixel")),
            PageAuth::Deny {
                code: "lan_off",
                ..
            }
        ));
    }

    #[test]
    fn a_wrong_session_secret_is_refused() {
        let (lan, _dir) = access();
        let (token, _rx) = activate(&lan);
        let _ = lan.page_load(Some(&token), None, &claims("Pixel"));
        assert!(
            lan.touch_session(Some("deadbeef"), &claims("Pixel"))
                .is_none()
        );
    }

    /// REQ-LAN-01 AC4 / REQ-LAN-09 AC5 both directions: a quiet phone is NOT
    /// «подключено» (the window must not claim a live session), and the same
    /// quiet phone keeps its cookie (a sleeping phone is not unpaired).
    #[test]
    fn a_quiet_device_is_not_connected_but_keeps_its_session() {
        let (lan, _dir) = access();
        let (token, _rx) = activate(&lan);
        let PageAuth::Allow {
            set_cookie: Some(cookie),
        } = lan.page_load(Some(&token), None, &claims("Pixel"))
        else {
            panic!("pairing");
        };
        {
            let mut inner = lan.lock();
            inner.sessions[0].last_seen = Utc::now() - chrono::Duration::seconds(120);
        }
        let status = lan.status();
        assert_eq!(
            status.state,
            LanState::Awaiting,
            "a silent phone is not a live session"
        );
        assert!(!status.devices[0].live);
        // Sleeping is not unpairing (ratified Q3).
        assert!(lan.touch_session(Some(&cookie), &claims("Pixel")).is_some());
        assert!(lan.status().devices[0].live);
        assert_eq!(lan.status().state, LanState::Connected);
    }

    #[test]
    fn a_network_change_stops_the_access_and_never_rebinds() {
        let (lan, dir) = access();
        let (token, _rx) = activate(&lan);
        let _ = lan.page_load(Some(&token), None, &claims("Pixel"));
        assert!(lan.persisted_enabled());
        // Same address: nothing happens.
        assert!(!lan.stop_if_network_changed(&["192.168.1.20".parse().unwrap()]));
        assert!(lan.is_on());
        // Address gone: honest stop with the typed reason.
        assert!(lan.stop_if_network_changed(&["10.0.0.5".parse().unwrap()]));
        let status = lan.status();
        assert_eq!(status.state, LanState::Off);
        assert_eq!(status.reason_code.as_deref(), Some("network_changed"));
        assert!(status.address.is_none() && status.devices.is_empty());
        assert!(
            !lan.persisted_enabled(),
            "the flag is cleared: a return to the old network must not restore it"
        );
        // No hidden recovery.
        assert!(lan.address().is_none());
        let _ = dir;
    }

    #[test]
    fn a_refusal_is_reported_and_never_claims_connected() {
        let (lan, _dir) = access();
        lan.refuse("no_local_address", "нет адреса в локальной сети");
        let status = lan.status();
        assert_eq!(status.state, LanState::Off);
        assert_eq!(status.reason_code.as_deref(), Some("no_local_address"));
        assert!(!lan.persisted_enabled());
    }

    /// R3: a refused write of the «was on» flag is a VISIBLE fact, not a
    /// swallowed `let _ =`. The access still works for this run; the window
    /// says the next launch will not restore it.
    #[test]
    fn a_failed_flag_write_is_visible_in_the_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, "not a directory").unwrap();
        let lan = LanAccess::new(blocker.join("lan-access.json"));
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let addr: SocketAddr = "192.168.1.20:4567".parse().unwrap();
        lan.activate(addr, tx);
        let status = lan.status();
        assert_eq!(status.state, LanState::Awaiting);
        let error = status.persist_error.expect("the write failure is visible");
        assert!(
            error.contains("blocked"),
            "the reason names the target: {error}"
        );
        // The access itself still serves this run.
        assert!(status.address.is_some());
    }

    #[test]
    fn an_enabled_access_is_restored_from_the_persisted_flag_only() {
        let (lan, dir) = access();
        assert!(!lan.persisted_enabled());
        let (token, _rx) = activate(&lan);
        drop(token);
        // A fresh process on the same control home sees «was on».
        let rebooted = LanAccess::new(dir.path().join("lan-access.json"));
        assert!(rebooted.persisted_enabled());
        let _ = lan.deactivate();
        assert!(!LanAccess::new(dir.path().join("lan-access.json")).persisted_enabled());
    }

    /// REQ-QRAPP-01: the invitation carries BOTH addresses, built from one
    /// code and one machine address; the QR encodes the app link, so the scan
    /// goes to the APK (REQ-QRAPP-02) while the browser link stays readable
    /// text for a phone without the app (REQ-QRAPP-03).
    #[test]
    fn the_invitation_carries_one_code_as_an_app_link_and_a_browser_link() {
        let (lan, _dir) = access();
        let (token, _rx) = activate(&lan);
        let status = lan.status();
        let address = "192.168.1.20:4567";
        assert_eq!(status.address.as_deref(), Some(address));
        let browser = status.url.as_deref().expect("browser link");
        let app = status.app_url.as_deref().expect("app link");

        // The same code and the same machine in both, and the code is the
        // published one.
        assert_eq!(status.token.as_deref(), Some(token.as_str()));
        assert!(browser.contains(&token));
        assert!(browser.contains(address));
        assert!(app.contains(&token));
        let parsed = crate::lan::pair::parse_app_link(app).expect("the app link is valid");
        assert_eq!(parsed.addr.to_string(), address);
        assert_eq!(parsed.token, token);

        // The QR is built from the app link, and it exists exactly while the
        // invitation is unused.
        assert!(
            status
                .qr_svg
                .as_deref()
                .is_some_and(|svg| svg.contains("<svg"))
        );
        assert!(status.invite_active);
    }

    /// R8: «QR не отрендерился» and «приглашение использовано» are two
    /// distinct facts — an empty QR slot alone cannot tell them apart.
    #[test]
    fn a_failed_qr_render_is_not_the_same_as_a_used_invitation() {
        let (lan, _dir) = access();
        let (_token, _rx) = activate(&lan);
        // A pathological token makes the QR encoder refuse the payload.
        let huge = "x".repeat(4000);
        {
            let mut inner = lan.lock();
            inner.invite = Some(Invite {
                token: huge.clone(),
            });
        }
        let failed = lan.status();
        assert!(failed.invite_active, "the invitation still exists");
        assert!(failed.qr_svg.is_none());
        assert!(
            failed.qr_error.is_some(),
            "a render failure has its own reason"
        );
        // A used invitation: neither QR nor error.
        let _ = lan.page_load(Some(&huge), None, &claims("Pixel"));
        let paired = lan.status();
        assert!(!paired.invite_active);
        assert!(paired.qr_svg.is_none() && paired.qr_error.is_none());
    }

    #[test]
    fn the_device_marker_describes_the_source_without_becoming_the_author() {
        let marker = actor_marker("guerrero", &claims("Pixel 7"));
        assert!(
            marker.starts_with("guerrero"),
            "the person stays the author"
        );
        assert!(marker.contains("телефон"));
        assert!(marker.contains("Pixel 7"));
        assert!(marker.contains("192.168.1.42"));
    }

    #[test]
    fn the_client_name_is_taken_from_the_ua_platform_block() {
        let ua = "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36";
        assert_eq!(device_name(ua), "Linux; Android 14; Pixel 7");
        assert_eq!(device_name(""), "неизвестное устройство");
        assert!(device_name(&"x".repeat(500)).chars().count() <= 80);
    }

    #[test]
    fn secrets_are_random_and_never_empty_hex() {
        let counter = AtomicU64::new(1);
        let a = fresh_secret(&counter, 32);
        let b = fresh_secret(&counter, 32);
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
