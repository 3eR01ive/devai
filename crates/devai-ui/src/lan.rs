//! LAN access from the phone (task 01a1be98, REQ-LAN-01…10).
//!
//! The window is a Tauri app whose data door is the command plane in
//! `commands.rs`; there is no other way to the store. The phone therefore does
//! NOT get a second data path: this module puts an embeddable HTTP server in
//! front of the SAME `#[tauri::command]` functions, and a browser page marks
//! itself as the phone transport (the frontend calls `rpc` instead of IPC when
//! it is not inside the Tauri webview). One command plane, two transports.
//!
//! Boundaries that are deliberate, not incidental:
//!   * the listener binds to THIS MACHINE'S LAN address, never `0.0.0.0` and
//!     never a loopback bridge, so the surface does not exist outside the home
//!     segment (REQ-LAN-03);
//!   * one-time invitation token → one device session; the token exists only
//!     while the access is on, is shown once, and is never written to logs or
//!     to disk (REQ-LAN-04);
//!   * the phone's writes call the same `Api::open` write handle the window and
//!     the CLI use, and carry a device marker in the actor string
//!     (`<operator> · телефон: <name> (<addr>)`) so history says where a
//!     decision came from without changing WHO signed it (REQ-LAN-06);
//!   * a watchdog re-reads the machine's interface list: when the bound address
//!     disappears the access is turned OFF with a typed reason and is NOT
//!     silently rebound (REQ-LAN-08).
//!
//! Ratified decisions of this task that shape the lifecycle:
//!   * Q1 «должен» — an access that was on before the last shutdown comes back
//!     on the next launch (the flag is persisted, never the token);
//!   * Q2 «может» — several phones may be connected; a new invitation retires
//!     the PREVIOUS invitation, not the sessions already paired;
//!   * Q4 «должен жить» — closing the window does not kill the access: the
//!     close is intercepted while the access is on (the process keeps serving,
//!     a second launch re-shows the window through the instance socket).

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::commands::{UiError, dispatch_rpc};

/// How often the watchdog re-reads the interface list (REQ-LAN-08). The
/// acceptance wording allows «не позже минуты» for a vanished phone; the
/// network itself is checked far quicker because an honest «сеть изменилась»
/// beats a silently dead server.
pub const NETWORK_POLL: std::time::Duration = std::time::Duration::from_secs(3);
/// A session that has been silent longer than this is no longer shown as
/// «подключено» (ratified Q5: not later than a minute). The session itself
/// stays valid — a sleeping phone must not need a new code (ratified Q3).
pub const LIVE_WINDOW: chrono::Duration = chrono::Duration::seconds(60);

const INVITE_TOKEN_BYTES: usize = 32;
const SESSION_SECRET_BYTES: usize = 32;
const COOKIE_NAME: &str = "devai_lan";

// ---------------------------------------------------------------------------
// status / view types (the wire shape mirrored by frontend/src/types.ts)
// ---------------------------------------------------------------------------

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
    /// `off` | `awaiting` | `connected` — the FACT, derived from the live
    /// server state, never from an intention flag (REQ-LAN-01 AC4).
    pub state: String,
    /// `ip:port` of the bound listener; `None` while off.
    pub address: Option<String>,
    /// One-time pairing URL (carries the token); only while awaiting.
    pub url: Option<String>,
    /// The invitation token itself, shown until a phone pairs; `None` after.
    pub token: Option<String>,
    /// Rendered QR of `url` (SVG markup). The phone scans it with its camera.
    pub qr_svg: Option<String>,
    /// Machine reason when the access is off after a refusal (e.g.
    /// `no_local_address`, `network_changed`, `bind_failed`).
    pub reason_code: Option<String>,
    /// Human wording of `reason_code`.
    pub reason: Option<String>,
    pub devices: Vec<LanDevice>,
    pub started_at: Option<DateTime<Utc>>,
}

impl LanStatus {
    fn off(reason: Option<(&'static str, String)>) -> LanStatus {
        LanStatus {
            state: "off".into(),
            address: None,
            url: None,
            token: None,
            qr_svg: None,
            reason_code: reason.as_ref().map(|(c, _)| c.to_string()),
            reason: reason.map(|(_, m)| m),
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

// ---------------------------------------------------------------------------
// sessions / invitations
// ---------------------------------------------------------------------------

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

    fn persist(&self, enabled: bool) {
        if let Some(dir) = self.state_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = serde_json::json!({ "enabled": enabled });
        let tmp = self.state_file.with_extension("json.tmp");
        if std::fs::write(&tmp, body.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.state_file);
        }
    }

    pub fn is_on(&self) -> bool {
        self.lock().on
    }

    #[allow(dead_code)] // read by the network-change test
    pub fn address(&self) -> Option<SocketAddr> {
        self.lock().address
    }

    /// The fact the window renders. `state` is derived: an access with a
    /// bound listener but no session is «ждёт подключения», one with a live
    /// session is «подключено» (REQ-LAN-09).
    pub fn status(&self) -> LanStatus {
        let inner = self.lock();
        if !inner.on {
            return LanStatus::off(inner.reason.clone());
        }
        let now = Utc::now();
        let address = inner.address;
        let devices: Vec<LanDevice> = inner.sessions.iter().map(|s| s.view(now)).collect();
        let state = if inner.sessions.is_empty() {
            "awaiting"
        } else {
            "connected"
        };
        let url = inner
            .invite
            .as_ref()
            .zip(address)
            .map(|(invite, addr)| format!("http://{addr}/?token={}", invite.token));
        LanStatus {
            state: state.into(),
            address: address.map(|a| a.to_string()),
            url: url.clone(),
            token: inner.invite.as_ref().map(|i| i.token.clone()),
            qr_svg: url.as_deref().and_then(qr_svg),
            reason_code: None,
            reason: None,
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
        self.persist(true);
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
        self.persist(false);
    }

    /// Turn the access off (the «отключить» button). Returning the shutdown
    /// sender lets the caller stop the listener; the state is already off, so
    /// the next request gets a refusal even if the socket drains slowly.
    pub fn deactivate(&self) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.on = false;
        inner.address = None;
        inner.invite = None;
        inner.sessions.clear();
        inner.reason = None;
        let bound = inner.bound.take();
        self.persist(false);
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
        if let Some(secret) = cookie {
            if let Some(session) = inner
                .sessions
                .iter_mut()
                .find(|s| secret_eq(&s.secret, secret))
            {
                session.last_seen = Utc::now();
                return PageAuth::Allow { set_cookie: None };
            }
        }
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return PageAuth::Deny {
                code: "lan_token_required",
                message: "нужен код приглашения".into(),
            };
        };
        let invite = match inner.invite.as_ref() {
            Some(i) if secret_eq(&i.token, token) => i.token.clone(),
            _ => {
                return PageAuth::Deny {
                    code: "lan_token_invalid",
                    message: "код приглашения недействителен или уже использован".into(),
                };
            }
        };
        // Consume the invitation: the same code cannot pair a second device.
        inner.invite = None;
        let _ = invite;
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

    /// The network watchdog (REQ-LAN-08): if the bound address is no longer
    /// one of the machine's addresses, the access stops HONESTLY — off, with
    /// the reason — and is never re-bound to the new network by itself.
    /// Returns true when it stopped.
    pub fn stop_if_network_changed(&self, current: &[Ipv4Addr]) -> bool {
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
            (true, bound.map(|b| b.shutdown))
        };
        self.persist(false);
        if let Some(tx) = shutdown {
            let _ = tx.send(true);
        }
        changed
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }
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

// ---------------------------------------------------------------------------
// network interfaces
// ---------------------------------------------------------------------------

/// Every IPv4 address of this machine that a phone in the home segment could
/// reach. Loopback and link-local are excluded: they are not «домашняя сеть».
#[cfg(unix)]
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() {
                let sa = &*ifa.ifa_addr;
                if sa.sa_family as i32 == libc::AF_INET {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                    let up = ifa.ifa_flags & libc::IFF_UP as u32 != 0;
                    let loopback = ifa.ifa_flags & libc::IFF_LOOPBACK as u32 != 0;
                    if up && !loopback && !ip.is_link_local() {
                        out.push(ip);
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

#[cfg(not(unix))]
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    // Route lookup without sending a packet; the only portable fallback.
    let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    if sock.connect("8.8.8.8:80").is_err() {
        return Vec::new();
    }
    match sock.local_addr() {
        Ok(SocketAddr::V4(a)) if !a.ip().is_loopback() => vec![*a.ip()],
        _ => Vec::new(),
    }
}

/// The address the access binds to: the home-segment address of the machine.
/// Private ranges first — a phone on the same Wi-Fi reaches them; the rest is
/// an honest attempt on unusual home networks. `None` = refuse to enable
/// (REQ-LAN-03 AC4), never a made-up address.
pub fn pick_lan_address(ips: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    // Home routers overwhelmingly hand out 192.168/16; 10/8 is the next
    // common home/VPN plan. 172.16/12 is where container bridges (docker0 =
    // 172.17.0.1) live, so it goes AFTER the home-looking plans — the phone
    // on the Wi-Fi cannot reach a bridge address.
    let rank = |ip: &Ipv4Addr| -> u8 {
        if ip.is_loopback() || ip.is_link_local() {
            return u8::MAX;
        }
        let o = ip.octets();
        match o {
            [192, 168, _, _] => 0,
            [10, _, _, _] => 1,
            [172, 16..=31, _, _] => 3,
            _ => 2,
        }
    };
    ips.iter()
        .filter(|ip| rank(ip) != u8::MAX)
        .min_by_key(|ip| rank(ip))
        .copied()
}

// ---------------------------------------------------------------------------
// server lifecycle
// ---------------------------------------------------------------------------

/// Bind, spawn the server thread and mark the access on. Returns the status
/// the window renders. Called by the `lan_enable` command and at startup when
/// the persisted flag says the access was on (ratified Q1).
pub fn start_access(app: tauri::AppHandle, lan: Arc<LanAccess>) -> Result<LanStatus, UiError> {
    // Already serving: the window asks again, the fact is the same.
    if lan.is_on() {
        return Ok(lan.status());
    }
    let Some(ip) = pick_lan_address(&local_ipv4s()) else {
        lan.refuse(
            "no_local_address",
            "у машины нет адреса в локальной сети — включить доступ нельзя",
        );
        return Err(UiError {
            code: "no_local_address".into(),
            message: "у машины нет адреса в локальной сети".into(),
        });
    };
    // Port 0: the OS picks a free port; the QR carries it, so there is nothing
    // to configure and nothing to collide with.
    let std_listener = match std::net::TcpListener::bind(SocketAddr::from((ip, 0))) {
        Ok(l) => l,
        Err(e) => {
            lan.refuse(
                "bind_failed",
                format!("не удалось открыть порт на {ip}: {e}"),
            );
            return Err(UiError {
                code: "bind_failed".into(),
                message: format!("не удалось открыть порт на {ip}: {e}"),
            });
        }
    };
    let _ = std_listener.set_nonblocking(true);
    let address = match std_listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            lan.refuse(
                "bind_failed",
                format!("не удалось определить порт на {ip}: {e}"),
            );
            return Err(UiError {
                code: "bind_failed".into(),
                message: format!("не удалось определить порт на {ip}: {e}"),
            });
        }
    };
    let (tx, rx) = tokio::sync::watch::channel(false);
    lan.activate(address, tx.clone());

    // The server thread owns its runtime: the Tauri process is not a tokio
    // app, and the access must stop cleanly without touching the window loop.
    let app_for_server = app.clone();
    let lan_for_server = lan.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("devai-lan".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            else {
                lan_for_server.refuse("runtime_failed", "не удалось запустить сервер доступа");
                return;
            };
            rt.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(std_listener) else {
                    lan_for_server.refuse("bind_failed", "не удалось открыть порт доступа");
                    return;
                };
                let surface = Arc::new(AppSurface {
                    app: app_for_server,
                });
                let router = routes(lan_for_server, surface);
                let served = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(wait_shutdown(rx));
                if let Err(e) = served.await {
                    eprintln!("devai-ui: LAN-сервер остановлен: {e}");
                }
            });
        })
    {
        // A listener without a thread is not an access: do not claim «on».
        lan.refuse(
            "runtime_failed",
            format!("не удалось запустить поток доступа: {e}"),
        );
        return Err(UiError {
            code: "runtime_failed".into(),
            message: format!("не удалось запустить поток доступа: {e}"),
        });
    }

    if let std::net::IpAddr::V4(bound_ip) = address.ip() {
        spawn_watchdog(lan.clone(), bound_ip);
    }
    // NOT the token: the address is the only fact that may reach the log.
    eprintln!("devai-ui: доступ с телефона включён на {address}");
    Ok(lan.status())
}

/// The «отключить» act: the state is off immediately, the listener is asked
/// to stop, and the watchdog of this run exits by itself (generation moved).
pub fn stop_access(lan: &LanAccess) -> LanStatus {
    lan.deactivate();
    lan.status()
}

/// Startup restore of the persisted flag (ratified Q1). A failure to bind is
/// not fatal: the window opens, the access stays off with its reason.
pub fn restore_at_startup(app: tauri::AppHandle, lan: Arc<LanAccess>) {
    if !lan.persisted_enabled() {
        return;
    }
    match start_access(app, lan) {
        Ok(status) => eprintln!(
            "devai-ui: доступ с телефона восстановлен на {}",
            status.address.unwrap_or_else(|| "—".into())
        ),
        Err(e) => eprintln!(
            "devai-ui: доступ не восстановлен: [{}] {}",
            e.code, e.message
        ),
    }
}

fn spawn_watchdog(lan: Arc<LanAccess>, bound_ip: Ipv4Addr) {
    let generation = lan.generation();
    std::thread::Builder::new()
        .name("devai-lan-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(NETWORK_POLL);
                if lan.generation() != generation {
                    return; // the access was disabled or restarted: this run is over
                }
                if lan.stop_if_network_changed(&local_ipv4s()) {
                    eprintln!("devai-ui: LAN-адрес {bound_ip} пропал — доступ выключен");
                    return;
                }
                if !lan.is_on() {
                    return;
                }
            }
        })
        .ok();
}

async fn wait_shutdown(mut rx: tokio::sync::watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    while rx.changed().await.is_ok() {
        if *rx.borrow() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP surface: assets + the RPC door
// ---------------------------------------------------------------------------

/// One embedded asset of the frontend bundle.
pub struct Asset {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Everything the HTTP boundary needs from the process it serves. The window
/// supplies the Tauri implementation (embedded assets + the command plane);
/// tests supply a fake, so the WHOLE request path — token → cookie → RPC —
/// runs over a real socket without a display.
pub trait Surface: Send + Sync + 'static {
    fn asset(&self, path: &str) -> Option<Asset>;
    fn dispatch(
        &self,
        device: &DeviceClaim,
        command: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, UiError>;
}

struct AppSurface {
    app: tauri::AppHandle,
}

impl Surface for AppSurface {
    fn asset(&self, path: &str) -> Option<Asset> {
        let asset = self.app.asset_resolver().get(path.to_string())?;
        Some(Asset {
            bytes: asset.bytes,
            mime: asset.mime_type,
        })
    }

    fn dispatch(
        &self,
        device: &DeviceClaim,
        command: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, UiError> {
        dispatch_rpc(&self.app, device, command, params)
    }
}

#[derive(Clone)]
struct ServerCtx {
    lan: Arc<LanAccess>,
    surface: Arc<dyn Surface>,
}

#[derive(Deserialize)]
struct RpcRequest {
    command: String,
    #[serde(default)]
    params: serde_json::Value,
}

fn routes(lan: Arc<LanAccess>, surface: Arc<dyn Surface>) -> axum::Router {
    let ctx = ServerCtx { lan, surface };
    axum::Router::new()
        .route("/", axum::routing::get(page))
        .route("/rpc", axum::routing::post(rpc))
        .route("/*path", axum::routing::get(asset))
        .with_state(ctx)
}

/// `GET /?token=…` — the QR target. A valid invitation pairs the device
/// (cookie set), a valid session cookie reloads without a new code.
async fn page(
    axum::extract::State(ctx): axum::extract::State<ServerCtx>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let device = DeviceClaim::new(header_str(&headers, "user-agent").unwrap_or(""), peer);
    let token = query.get("token").map(String::as_str);
    let cookie = cookie_value(&headers, COOKIE_NAME);
    match ctx.lan.page_load(token, cookie.as_deref(), &device) {
        PageAuth::Deny { code, message } => {
            if code == "lan_off" {
                plain(axum::http::StatusCode::FORBIDDEN, &message)
            } else {
                plain(axum::http::StatusCode::UNAUTHORIZED, &message)
            }
        }
        PageAuth::Allow { set_cookie } => {
            let mut response = index_response(&ctx);
            if let Some(secret) = set_cookie {
                let value = format!("{COOKIE_NAME}={secret}; Path=/; HttpOnly; SameSite=Lax");
                if let Ok(value) = axum::http::HeaderValue::from_str(&value) {
                    response
                        .headers_mut()
                        .append(axum::http::header::SET_COOKIE, value);
                }
            }
            response
        }
    }
}

/// `POST /rpc` — the phone's door to the SAME command plane. The session
/// cookie is the only key; without it nothing is answered (REQ-LAN-04).
async fn rpc(
    axum::extract::State(ctx): axum::extract::State<ServerCtx>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<RpcRequest>,
) -> axum::response::Response {
    let device = DeviceClaim::new(header_str(&headers, "user-agent").unwrap_or(""), peer);
    let cookie = cookie_value(&headers, COOKIE_NAME);
    let Some(device) = ctx.lan.touch_session(cookie.as_deref(), &device) else {
        return json(
            axum::http::StatusCode::UNAUTHORIZED,
            serde_json::json!({
                "ok": false,
                "error": {
                    "code": "lan_unauthorized",
                    "message": "доступ не подтверждён: откройте страницу по коду приглашения",
                }
            }),
        );
    };
    // The dispatch is deliberately exhaustive over the command plane: the
    // phone gets exactly the window's rights, no narrower and no wider
    // (REQ-LAN-05). An unknown name is refused, never guessed.
    match ctx
        .surface
        .dispatch(&device, &request.command, &request.params)
    {
        Ok(data) => json(
            axum::http::StatusCode::OK,
            serde_json::json!({ "ok": true, "data": data }),
        ),
        Err(e) => json(
            axum::http::StatusCode::OK,
            serde_json::json!({ "ok": false, "error": { "code": e.code, "message": e.message } }),
        ),
    }
}

/// `GET /assets/…` — the SAME embedded frontend the window renders. Requires
/// a paired session: the application shell itself is part of what a token
/// unlocks, so an unpaired client gets nothing at all.
async fn asset(
    axum::extract::State(ctx): axum::extract::State<ServerCtx>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> axum::response::Response {
    let device = DeviceClaim::new(header_str(&headers, "user-agent").unwrap_or(""), peer);
    let cookie = cookie_value(&headers, COOKIE_NAME);
    if ctx.lan.touch_session(cookie.as_deref(), &device).is_none() {
        return plain(
            axum::http::StatusCode::UNAUTHORIZED,
            "нужен код приглашения",
        );
    }
    let path = uri.path().trim_start_matches('/');
    match ctx.surface.asset(path) {
        Some(asset) => {
            let mut response = axum::response::Response::new(axum::body::Body::from(asset.bytes));
            if let Ok(value) = axum::http::HeaderValue::from_str(&asset.mime) {
                response
                    .headers_mut()
                    .insert(axum::http::header::CONTENT_TYPE, value);
            }
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response
        }
        None => plain(axum::http::StatusCode::NOT_FOUND, "не найдено"),
    }
}

fn index_response(ctx: &ServerCtx) -> axum::response::Response {
    match ctx.surface.asset("index.html") {
        Some(asset) => {
            let mut response = axum::response::Response::new(axum::body::Body::from(asset.bytes));
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
            );
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response
        }
        None => plain(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "интерфейс не собран в этом бинаре",
        ),
    }
}

fn header_str<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_string())
    })
}

fn plain(status: axum::http::StatusCode, message: &str) -> axum::response::Response {
    let mut response = axum::response::Response::new(axum::body::Body::from(message.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn json(status: axum::http::StatusCode, body: serde_json::Value) -> axum::response::Response {
    let mut response = axum::response::Response::new(axum::body::Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

// ---------------------------------------------------------------------------
// single-instance handoff (ratified Q4: the access outlives the window)
// ---------------------------------------------------------------------------

/// While the access is on, a window close only HIDES the window (lib.rs); the
/// process keeps serving the phone. Without a handoff the hidden window would
/// be unreachable — a second launch connects to this socket and asks the
/// running instance to show itself again.
#[cfg(all(unix, not(mobile)))]
pub struct InstanceGuard {
    path: PathBuf,
    listener: std::os::unix::net::UnixListener,
}

#[cfg(all(unix, not(mobile)))]
impl InstanceGuard {
    /// `None` when another instance is already running: this call has asked
    /// it to show its window and the caller must exit.
    pub fn acquire(path: PathBuf) -> Option<InstanceGuard> {
        use std::os::unix::net::{UnixListener, UnixStream};
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut stream) = UnixStream::connect(&path) {
            use std::io::Write;
            let _ = stream.write_all(b"show\n");
            return None;
        }
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).ok()?;
        Some(InstanceGuard { path, listener })
    }

    /// Accept handoffs on a background thread and re-show the window.
    pub fn spawn(self, app: tauri::AppHandle) {
        std::thread::Builder::new()
            .name("devai-ui-instance".into())
            .spawn(move || {
                use tauri::Manager;
                for stream in self.listener.incoming() {
                    if stream.is_err() {
                        continue;
                    }
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
                let _ = std::fs::remove_file(&self.path);
            })
            .ok();
    }
}

#[cfg(not(all(unix, not(mobile))))]
pub struct InstanceGuard;

#[cfg(not(all(unix, not(mobile))))]
impl InstanceGuard {
    pub fn acquire(_path: PathBuf) -> Option<InstanceGuard> {
        Some(InstanceGuard)
    }

    pub fn spawn(self, _app: tauri::AppHandle) {}
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

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

    #[test]
    fn the_off_state_serves_nothing_and_names_no_address() {
        let (lan, _dir) = access();
        let status = lan.status();
        assert_eq!(status.state, "off");
        assert!(status.address.is_none());
        assert!(status.url.is_none());
        assert!(status.devices.is_empty());
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
        assert_eq!(status.state, "connected");
        assert!(
            status.token.is_none() && status.url.is_none(),
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
        assert_eq!(status.state, "connected");
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
        assert_eq!(lan.status().state, "off");
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

    #[test]
    fn a_quiet_device_is_no_longer_live_but_keeps_its_session() {
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
        assert_eq!(status.state, "connected");
        assert!(!status.devices[0].live, "a silent phone is not 'connected'");
        // Sleeping is not unpairing (ratified Q3).
        assert!(lan.touch_session(Some(&cookie), &claims("Pixel")).is_some());
        assert!(lan.status().devices[0].live);
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
        assert_eq!(status.state, "off");
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
        assert_eq!(status.state, "off");
        assert_eq!(status.reason_code.as_deref(), Some("no_local_address"));
        assert!(!lan.persisted_enabled());
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
    fn the_lan_address_prefers_the_home_segment() {
        let ips: Vec<Ipv4Addr> = ["100.64.0.1", "192.168.1.20", "10.1.2.3"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            pick_lan_address(&ips),
            Some("192.168.1.20".parse().unwrap())
        );
        // A container bridge (docker0 = 172.17.0.1) is not a home address:
        // the Wi-Fi plan wins even when the bridge is enumerated first.
        let with_docker: Vec<Ipv4Addr> = ["172.17.0.1", "10.1.2.3", "127.0.0.1"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            pick_lan_address(&with_docker),
            Some("10.1.2.3".parse().unwrap())
        );
        assert_eq!(pick_lan_address(&[]), None);
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

    struct FakeSurface;

    impl Surface for FakeSurface {
        fn asset(&self, path: &str) -> Option<Asset> {
            match path {
                "index.html" => Some(Asset {
                    bytes: b"<html>ok</html>".to_vec(),
                    mime: "text/html".into(),
                }),
                p if p.starts_with("assets/") => Some(Asset {
                    bytes: p.as_bytes().to_vec(),
                    mime: "text/plain".into(),
                }),
                _ => None,
            }
        }

        fn dispatch(
            &self,
            device: &DeviceClaim,
            command: &str,
            params: &serde_json::Value,
        ) -> Result<serde_json::Value, UiError> {
            Ok(serde_json::json!({
                "command": command,
                "device": device.name,
                "params": params,
            }))
        }
    }

    fn http(addr: SocketAddr, request: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        out
    }

    fn get(addr: SocketAddr, path: &str, cookie: Option<&str>) -> String {
        let cookie = cookie
            .map(|c| format!("Cookie: devai_lan={c}\r\n"))
            .unwrap_or_default();
        http(
            addr,
            &format!("GET {path} HTTP/1.1\r\nHost: lan\r\nConnection: close\r\n{cookie}\r\n"),
        )
    }

    fn post_rpc(addr: SocketAddr, cookie: Option<&str>, body: &str) -> String {
        let cookie = cookie
            .map(|c| format!("Cookie: devai_lan={c}\r\n"))
            .unwrap_or_default();
        http(
            addr,
            &format!(
                "POST /rpc HTTP/1.1\r\nHost: lan\r\nConnection: close\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\n{cookie}\r\n{body}",
                body.len()
            ),
        )
    }

    fn start_http_server(lan: Arc<LanAccess>) -> SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let _ = listener.set_nonblocking(true);
        let addr = listener.local_addr().unwrap();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        lan.activate(addr, tx);
        let lan_for_server = lan;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(
                    listener,
                    routes(lan_for_server, Arc::new(FakeSurface))
                        .into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                .unwrap();
            });
        });
        addr
    }

    fn cookie_of(response: &str) -> Option<String> {
        let lower = response.to_ascii_lowercase();
        let at = lower.find("set-cookie: devai_lan=")?;
        let rest = &response[at + "set-cookie: devai_lan=".len()..];
        let end = rest.find(';')?;
        Some(rest[..end].to_string())
    }

    /// The WHOLE LAN boundary over a real socket: nothing is served before the
    /// invitation is presented, the invitation becomes an HttpOnly cookie, the
    /// RPC door accepts only that session, and «отключить» closes it at once
    /// (REQ-LAN-04 AC1-6). The window's own command plane is replaced by a
    /// fake surface — the transport, not the domain, is under test here.
    #[test]
    fn the_http_boundary_serves_nothing_without_a_valid_pairing() {
        let (lan, _dir) = access();
        let lan = Arc::new(lan);
        let addr = start_http_server(lan.clone());
        let invite = lan.status().token.expect("a fresh invitation");

        // 1. No token: neither the shell nor the RPC door answers.
        let unpaired = get(addr, "/", None);
        assert!(
            unpaired.starts_with("HTTP/1.1 401"),
            "an unpaired client must get a refusal, got: {}",
            &unpaired[..unpaired.len().min(40)]
        );
        let rpc_unpaired = post_rpc(addr, None, "{\"command\":\"tasks_list\",\"params\":{}}");
        assert!(
            rpc_unpaired.starts_with("HTTP/1.1 401"),
            "RPC without a session must be refused"
        );

        // 2. The invitation opens the shell and turns into an HttpOnly cookie.
        let paired = get(addr, &format!("/?token={invite}"), None);
        assert!(paired.starts_with("HTTP/1.1 200"), "pairing: {paired}");
        let cookie = cookie_of(&paired).expect("pairing sets the session cookie");
        assert!(
            paired.to_ascii_lowercase().contains("httponly"),
            "the session cookie must not be readable from JS"
        );

        // 3. The cookie carries every request from then on — and the SAME
        //    invitation cannot pair a second client.
        assert!(get(addr, "/", Some(&cookie)).starts_with("HTTP/1.1 200"));
        assert!(
            get(addr, &format!("/?token={invite}"), None).starts_with("HTTP/1.1 401"),
            "a used invitation is dead"
        );
        let rpc = post_rpc(
            addr,
            Some(&cookie),
            "{\"command\":\"tasks_list\",\"params\":{}}",
        );
        assert!(rpc.starts_with("HTTP/1.1 200"), "paired RPC: {rpc}");
        assert!(rpc.contains("\"ok\":true"));
        assert!(rpc.contains("tasks_list"));

        // 4. «Отключить» revokes the session: the very next request is out,
        //    even though the page was already open in the phone's browser.
        lan.deactivate();
        let after = post_rpc(
            addr,
            Some(&cookie),
            "{\"command\":\"tasks_list\",\"params\":{}}",
        );
        assert!(
            after.starts_with("HTTP/1.1 401"),
            "a revoked session must not be served"
        );
        assert!(get(addr, "/", Some(&cookie)).starts_with("HTTP/1.1 403"));
    }

    #[test]
    fn the_rpc_route_and_the_window_share_one_command_list() {
        // The phone may reach exactly the window's command plane: the route
        // table is a subset of the registered commands, and every domain
        // command is routed (REQ-LAN-05 AC1-2).
        let routed: Vec<&str> = crate::commands::commands_for_rpc().to_vec();
        let lib = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("lib.rs");
        for name in &routed {
            assert!(
                lib.contains(&format!("commands::{name},")),
                "`{name}` is routed over the LAN but not registered in the invoke handler"
            );
        }
        for required in ["task_approve", "task_reject", "tasks_list", "homes_list"] {
            assert!(routed.contains(&required), "`{required}` must be reachable");
        }
    }
}
