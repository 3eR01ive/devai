//! The phone side of the invitation (task 01a1bf78, REQ-QRAPP-01…07): the
//! `devai://pair` link the QR carries, the four refusals in plain words, the
//! same-network test, the reading of the machine's cheap health answer and the
//! record of the remembered home.
//!
//! The DECISION half is pure: the question `GET /pair` itself is asked by the
//! page (the app's window), not by this process — the project's `no-network`
//! gate forbids outbound sockets in its own code (local-first sovereignty),
//! and the phone's interface is a page anyway. Rust decides, the page talks —
//! see [`super::deeplink`] — so the whole pairing decision («открыть этот
//! дом» vs «сказать человеку вот такую причину») is unit-testable without a
//! Tauri app, a webview or a real machine on the LAN.
//!
//! The module also has a FILE leg, and it is not pure: [`PairStore`] reads,
//! writes and renames the remembered record under the app's data directory
//! (ratified Q4). It lives here because the record's shape — one address, no
//! code, no session key — belongs with the vocabulary that describes it; its
//! failures are typed ([`PairStoreError`]) and never swallowed.
//!
//! Single source of the refusal and notice vocabulary: the declarative data
//! `data/pair-vocabulary.json` spells every code, title and hint ONCE, and both
//! halves read it — this module through [`refusals`]/[`notices`], the page by
//! generating `frontend/src/generated/pair-vocabulary.ts` from the same file
//! (`frontend/scripts/gen-pair-vocabulary.mjs`). A domain catalogue belongs in
//! data, not in code (quality return R6), and no hand-written mirror can drift
//! into a generic «ошибка связи» (REQ-QRAPP-06 AC4, quality return R5).

// Read by the mobile wiring, by the always-compiled pairing session and by the
// tests. Nothing CONSTRUCTS a pairing session on a desktop build, which is what
// «на десктопе поведение не меняется» means (REQ-QRAPP-08); the code is
// compiled everywhere so one test run guards both halves.
#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The scheme the APK declares (`tauri.conf.json > plugins > deep-link`).
pub const PAIR_SCHEME: &str = "devai";
/// `devai://pair?...` — the host carries the verb, so a stray `devai://file`
/// link can never be mistaken for a pairing offer (REQ-QRAPP-07).
pub const PAIR_HOST: &str = "pair";
/// How long the page may wait for the machine's one-word answer. A check cycle
/// stays under five seconds (REQ-QRAPP-05 AC1); the question must not eat it.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// How often the page asks the paired machine whether the session still lives.
pub const WATCH_POLL: Duration = Duration::from_secs(3);

/// The three HTTP facts `GET /pair` can carry, as constants: the status is a
/// DOMAIN fact of the pairing (a live invitation, a dead code, a closed
/// access), not a magic number of the transport (quality return R6).
pub const STATUS_INVITE: u16 = 200;
pub const STATUS_REFUSED: u16 = 401;
pub const STATUS_CLOSED: u16 = 403;

/// The `state` token of the `/pair` answer — the vocabulary the surface
/// publishes (pinned by `the_pair_answer_states_are_declared_once`).
pub const STATE_PAIRED: &str = "paired";
pub const STATE_INVITE: &str = "invite";
pub const STATE_INVALID: &str = "invalid";
pub const STATE_CLOSED: &str = "closed";

/// The notice codes the app leaves on a page it opened (see [`notices`]).
pub const NOTICE_NOT_REMEMBERED: &str = "pair_not_remembered";
pub const NOTICE_RECORD_BROKEN: &str = "pair_record_broken";

/// The longest one-time code the pairing accepts. A 32-byte code is 64 hex
/// characters, so this leaves room for a longer spelling without accepting an
/// unbounded string from a scanned link (REQ-QRAPP-07).
pub const TOKEN_MAX_LEN: usize = 128;

/// A home network is `/24` in practice and the invitation carries no netmask,
/// so the phone compares this many leading octets (REQ-QRAPP-06 constraint).
pub const HOME_PREFIX_OCTETS: usize = 3;

/// The temporary file a remembered record is written through, so a reader
/// never sees a half-written record.
const TMP_EXTENSION: &str = "json.tmp";

/// Why an invitation was not accepted. Every variant is a DIFFERENT sentence
/// for the person and a different machine code (REQ-QRAPP-06): «просрочено»,
/// «не та сеть», «доступ закрыт» are three facts, not one error.
///
/// The variant name IS the row key in `data/pair-vocabulary.json` (snake
/// case), so adding a reason means adding a variant and a data row — never a
/// third copy of the words in code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairRefusal {
    /// Used, replaced by a newer invitation or closed on the machine. The
    /// ratified decision of 09.11: an invitation never expires by the clock —
    /// «просрочено» means explicitly closed/consumed.
    InviteExpired,
    /// The phone is not in the machine's network right now.
    WrongNetwork,
    /// The machine answered and says this device has no access any more.
    AccessClosed,
    /// The link itself is not a pairing link: wrong scheme, extra parameters,
    /// a path, or an address outside a home network (REQ-QRAPP-07).
    BadLink,
}

/// One row of the refusal vocabulary, parsed from the data file: the machine
/// code that travels on the wire, the words the person reads and the way out.
/// THE single source of all three — the frontend module
/// `src/generated/pair-vocabulary.ts` is rendered from the same rows, so the
/// two halves cannot drift (quality return R5).
#[derive(Deserialize)]
pub struct Refusal {
    pub kind: PairRefusal,
    pub code: String,
    pub title: String,
    pub hint: String,
}

/// A note the app leaves for the person AFTER the access was granted: the
/// access works, but something was not kept. Same data file as the refusals,
/// rendered into the same generated frontend module.
#[derive(Deserialize)]
pub struct Notice {
    pub code: String,
    pub title: String,
    pub hint: String,
}

/// The data file the vocabulary is declared in (quality return R6: a domain
/// catalogue lives in data, not in a typed table in code). Compiled into the
/// binary, so the app reads no file at run time and the data cannot go missing
/// on the phone.
const VOCABULARY_JSON: &str = include_str!("../../data/pair-vocabulary.json");

#[derive(Deserialize)]
struct Vocabulary {
    refusals: Vec<Refusal>,
    notices: Vec<Notice>,
}

fn vocabulary() -> &'static Vocabulary {
    static VOCABULARY: OnceLock<Vocabulary> = OnceLock::new();
    VOCABULARY.get_or_init(|| {
        serde_json::from_str(VOCABULARY_JSON)
            .expect("data/pair-vocabulary.json must parse (pinned by the tests)")
    })
}

/// Every refusal row, in the order the file declares them.
pub fn refusals() -> &'static [Refusal] {
    &vocabulary().refusals
}

/// Every notice row, in the order the file declares them.
pub fn notices() -> &'static [Notice] {
    &vocabulary().notices
}

impl PairRefusal {
    /// Every refusal the surface can publish, DERIVED from the data rows.
    /// There is no hand-kept list to fall out of step with the table (quality
    /// return: the third edit site is gone); a test walks every enum variant
    /// and fails when one has no row.
    pub fn all() -> impl Iterator<Item = PairRefusal> {
        refusals().iter().map(|row| row.kind)
    }

    /// The one row that describes this refusal. The rows are checked against
    /// every enum variant by the tests, so this lookup cannot miss in a
    /// shipped build.
    pub fn row(self) -> &'static Refusal {
        refusals()
            .iter()
            .find(|row| row.kind == self)
            .expect("every PairRefusal has a row in data/pair-vocabulary.json")
    }

    /// The machine code that travels in the URL (`#/pair?reason=<code>`) and
    /// in the refusal page. Three misuse states, three different codes.
    pub fn code(self) -> &'static str {
        self.row().code.as_str()
    }

    /// The words the person reads. Deliberately concrete: no technical codes,
    /// no «ошибка связи» umbrella (REQ-QRAPP-06 AC1-3).
    pub fn text(self) -> &'static str {
        self.row().title.as_str()
    }

    /// What the person can do next — the refusal screen is not a dead end.
    pub fn hint(self) -> &'static str {
        self.row().hint.as_str()
    }
}

/// A pairing offer that passed every check: the machine's address in the home
/// network and the one-time code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairLink {
    pub addr: SocketAddr,
    pub token: String,
}

/// Build the link the QR encodes. The browser link is built from the SAME
/// invitation in [`super::state`], so the two addresses cannot disagree
/// (REQ-QRAPP-01 constraint).
pub fn app_link(addr: SocketAddr, token: &str) -> String {
    format!(
        "{PAIR_SCHEME}://{PAIR_HOST}?addr={}&token={}",
        encode(&addr.to_string()),
        encode(token)
    )
}

/// Strict reader of a scanned link. Anything that is not exactly
/// `devai://pair?addr=<home>[:port]&token=<code>` is refused BEFORE a byte is
/// sent anywhere: no paths, no extra parameters, no other schemes, no
/// public/loopback/link-local addresses (REQ-QRAPP-07 AC1-2).
pub fn parse_app_link(raw: &str) -> Result<PairLink, PairRefusal> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix(&format!("{PAIR_SCHEME}://"))
        .ok_or(PairRefusal::BadLink)?;
    let (host, query) = rest.split_once('?').ok_or(PairRefusal::BadLink)?;
    // The host must be exactly `pair`: no userinfo, no path, no slash.
    if host != PAIR_HOST {
        return Err(PairRefusal::BadLink);
    }
    if query.is_empty() || query.contains('#') {
        return Err(PairRefusal::BadLink);
    }
    let mut addr: Option<String> = None;
    let mut token: Option<String> = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or(PairRefusal::BadLink)?;
        let value = decode(value)?;
        let slot = match key {
            "addr" => &mut addr,
            "token" => &mut token,
            // An unknown parameter is not ignored: a link that carries more
            // than the two known names is not ours.
            _ => return Err(PairRefusal::BadLink),
        };
        if slot.replace(value).is_some() {
            return Err(PairRefusal::BadLink);
        }
    }
    let addr = addr.ok_or(PairRefusal::BadLink)?;
    let token = token.ok_or(PairRefusal::BadLink)?;
    let addr = parse_home_addr(&addr)?;
    if !token_is_plausible(&token) {
        return Err(PairRefusal::BadLink);
    }
    Ok(PairLink { addr, token })
}

/// A machine address a phone in the home segment can reach: IPv4, a private
/// range, a real port. `127.0.0.1`, `169.254/16`, public hosts and port 0 are
/// refused instead of being opened (REQ-QRAPP-07).
pub fn parse_home_addr(raw: &str) -> Result<SocketAddr, PairRefusal> {
    let addr: SocketAddr = raw.parse().map_err(|_| PairRefusal::BadLink)?;
    match addr {
        SocketAddr::V4(v4)
            if v4.port() != 0
                && v4.ip().is_private()
                && !v4.ip().is_loopback()
                && !v4.ip().is_link_local() =>
        {
            Ok(addr)
        }
        _ => Err(PairRefusal::BadLink),
    }
}

fn token_is_plausible(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= TOKEN_MAX_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A fingerprint of a scanned link: «this invitation was already handled by
/// this phone». It is NOT the link and NOT the code — `link_digest` hashes the
/// bytes into a fixed 64-bit FNV-1a value, so the one-time code never lands on
/// disk (REQ-QRAPP-07 AC3), while a replayed LAUNCHING intent is still
/// recognised after a restart (quality return R4). The digest cannot act as an
/// invitation: the code is single-use, and a digest is not a code.
pub fn link_digest(raw: &str) -> String {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// Percent-encoding limited to what a query value can carry. The token is
/// hex and the address is `ip:port`, so this is enough and keeps the link
/// readable in the QR.
fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'~' | b'-' | b'_') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(raw: &str) -> Result<String, PairRefusal> {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = raw.get(i + 1..i + 3).ok_or(PairRefusal::BadLink)?;
                let byte = u8::from_str_radix(hex, 16).map_err(|_| PairRefusal::BadLink)?;
                out.push(byte);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| PairRefusal::BadLink)
}

/// The page the app opens for a paired home: the SAME interface a browser
/// gets, from the machine itself (REQ-QRAPP-02 AC2/AC5). With a one-time code
/// on the first open; without it when the remembered home is re-opened and the
/// session cookie is already there.
pub fn home_url(addr: SocketAddr, token: Option<&str>) -> String {
    match token {
        Some(token) => format!("http://{addr}/?token={}", encode(token)),
        None => format!("http://{addr}/"),
    }
}

/// Where a refusal is shown: `<base>#/pair?reason=<code>`. The base is the
/// machine's page while it works (a dead NEW invitation must not close the
/// working session, ratified Q3) or the bundled app page when no home is open.
/// `can_return` tells the page that the working home is still there, so the
/// refusal screen offers the way back instead of guessing from the reason.
pub fn refusal_url(base: &str, refusal: PairRefusal, can_return: bool) -> String {
    let back = if can_return { "&return=1" } else { "" };
    page_route(base, &format!("reason={}{back}", refusal.code()))
}

/// The screen the app shows while it is answering the invitation: the person
/// sees that the scan was taken, instead of the app's own (unrelated) home
/// flashing before the machine's interface opens.
///
/// `scan` is the link's FINGERPRINT (never the link or the code). It makes the
/// address DIFFERENT for every invitation, so a second scan while the app is
/// already on this screen is a real address change — a fragment-only navigation
/// is a same-document load and would never re-run the handshake (quality
/// return R1, warm scan).
pub fn connect_url(base: &str, scan: &str) -> String {
    page_route(base, &format!("step=connect&scan={scan}"))
}

/// ONE builder of the app's own screens (`#/pair?…`): a route is never spelled
/// twice, so the refusal and the progress screen cannot drift apart.
fn page_route(base: &str, query: &str) -> String {
    let base = base.split('#').next().unwrap_or(base);
    format!("{base}#/pair?{query}")
}

/// The same address plus the one fact the page shows once (a write of the
/// remembered home failed: the app still works now, but the next launch will
/// not know this home — quality return R1). The notice travels in the address
/// the window opens, so nothing is swallowed and no second channel is needed.
pub fn with_notice(url: &str, notice: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}notice={}", encode(notice))
}

/// Whether the machine's address is in the network this phone is on. Home
/// networks are `/24` in practice (`192.168.1.0/24` and friends), and the
/// phone has no netmask from the invitation — so the comparison is on the
/// first three octets (REQ-QRAPP-06 constraint: «сравнивается с текущей
/// сетью»; the silence of an address in this network reads as a closed door).
pub fn same_network(home: Ipv4Addr, local: &[Ipv4Addr]) -> bool {
    let h = home.octets();
    local.iter().any(|ip| {
        let o = ip.octets();
        !ip.is_loopback()
            && !ip.is_link_local()
            && o[..HOME_PREFIX_OCTETS] == h[..HOME_PREFIX_OCTETS]
    })
}

/// [`same_network`] for a socket address. IPv6 has no home-segment notion here
/// — the access is IPv4-only by construction, so anything else is «не та сеть».
pub fn same_network_addr(home: SocketAddr, local: &[Ipv4Addr]) -> bool {
    match home {
        SocketAddr::V4(v4) => same_network(*v4.ip(), local),
        SocketAddr::V6(_) => false,
    }
}

/// What `GET /pair` answered. The page reports the status it read; this module
/// names the fact. Four different sentences for the person come from four
/// different facts (REQ-QRAPP-06 AC4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// 200: the presented one-time code is valid and not consumed yet.
    Invite,
    /// 401: the machine answered and refused — the code is used/replaced.
    Refused,
    /// 403: the access is turned off on the machine.
    Closed,
    /// No answer at all: the address is silent.
    Silent,
}

/// The ONE reading of the machine's status. Anything but the three known
/// answers is silence, never a guess (quality return R6: no status literals in
/// the decision).
pub fn probe_from_status(status: Option<u16>) -> Probe {
    match status {
        Some(STATUS_INVITE) => Probe::Invite,
        Some(STATUS_REFUSED) => Probe::Refused,
        Some(STATUS_CLOSED) => Probe::Closed,
        _ => Probe::Silent,
    }
}

/// The address the page asks for the one-word answer. One producer, so the
/// question and the scanned link can never disagree about the address/code
/// shape (quality return R4).
pub fn probe_url(addr: SocketAddr, token: &str) -> String {
    format!("http://{addr}/pair?token={}", encode(token))
}

/// The refusal a probe result means while ACCEPTING an invitation: a refusal
/// here is a dead code, not a revoked session; a closed door is named as such
/// (REQ-QRAPP-06 AC1/AC3 — «просрочено» and «доступ закрыт» are two facts).
pub fn classify_invite(probe: Probe, home: SocketAddr, local: &[Ipv4Addr]) -> Option<PairRefusal> {
    match probe {
        Probe::Invite => None,
        Probe::Refused => Some(PairRefusal::InviteExpired),
        Probe::Closed => Some(PairRefusal::AccessClosed),
        Probe::Silent => Some(silence(home, local)),
    }
}

/// Silence of the address in the phone's own network is a closed door; silence
/// outside it is the wrong network (REQ-QRAPP-06 constraint).
fn silence(home: SocketAddr, local: &[Ipv4Addr]) -> PairRefusal {
    match home {
        SocketAddr::V4(v4) if same_network(*v4.ip(), local) => PairRefusal::AccessClosed,
        _ => PairRefusal::WrongNetwork,
    }
}

/// The phone's record of the pairing, remembered between launches (ratified
/// Q4). It holds the ADDRESS and a DIGEST of the last handled link — a
/// fingerprint, not the link and not the one-time code, so no code and no
/// session key ever lands on disk (REQ-QRAPP-07 AC3).
#[derive(Debug)]
pub struct PairStore {
    path: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct StoredPairing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handled: Option<String>,
}

/// What the phone remembers right now.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StoredPairingRead {
    /// The home to re-open, if one was remembered and is still a home address.
    pub home: Option<SocketAddr>,
    /// The digest of the last link this phone already handled.
    pub handled: Option<String>,
}

/// What [`PairStore::load`] found. THREE outcomes, not two (quality return
/// R2): «записи нет» и «запись испорчена» are different facts, and the second
/// one is VISIBLE — a phone that silently forgot a home it had just paired
/// with looked exactly like a phone that never paired.
#[derive(Debug, PartialEq, Eq)]
pub enum Remembered {
    Absent,
    Record(StoredPairingRead),
    Broken(PairStoreError),
}

/// A failed read or write of the remembered record. Typed on purpose (quality
/// return R1/R2): the caller must be able to SAY what happened — with a code
/// and the exact path — instead of swallowing a string. Never carries a code
/// or a session secret, so it is safe to show and to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairStoreError {
    NoDirectory,
    CreateDirectory { path: String, reason: String },
    Write { path: String, reason: String },
    Rename { path: String, reason: String },
    Encode { reason: String },
    Read { path: String, reason: String },
    Parse { path: String, reason: String },
}

impl PairStoreError {
    /// The code for a record that could not be WRITTEN: the access works, the
    /// next launch will not know the home.
    pub const NOT_REMEMBERED: &'static str = NOTICE_NOT_REMEMBERED;
    /// The code for a record that exists but cannot be read back: the phone
    /// forgot the home and says so before scanning again.
    pub const RECORD_BROKEN: &'static str = NOTICE_RECORD_BROKEN;

    pub fn code(&self) -> &'static str {
        // Exhaustive on purpose (quality return R7): a new variant has to
        // CHOOSE which fact it is, instead of a catch-all handing it the
        // «not remembered» sentence of a write failure.
        match self {
            PairStoreError::Read { .. } | PairStoreError::Parse { .. } => Self::RECORD_BROKEN,
            PairStoreError::NoDirectory
            | PairStoreError::CreateDirectory { .. }
            | PairStoreError::Write { .. }
            | PairStoreError::Rename { .. }
            | PairStoreError::Encode { .. } => Self::NOT_REMEMBERED,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            PairStoreError::NoDirectory => None,
            PairStoreError::CreateDirectory { path, .. }
            | PairStoreError::Write { path, .. }
            | PairStoreError::Rename { path, .. }
            | PairStoreError::Read { path, .. }
            | PairStoreError::Parse { path, .. } => Some(path),
            PairStoreError::Encode { .. } => None,
        }
    }

    pub fn message(&self) -> String {
        match self {
            PairStoreError::NoDirectory => "у файла пары нет каталога".to_string(),
            PairStoreError::CreateDirectory { path, reason } => {
                format!("не удалось создать каталог {path}: {reason}")
            }
            PairStoreError::Write { path, reason } => {
                format!("не удалось записать {path}: {reason}")
            }
            PairStoreError::Rename { path, reason } => {
                format!("не удалось сохранить {path}: {reason}")
            }
            PairStoreError::Encode { reason } => {
                format!("не удалось собрать запись пары: {reason}")
            }
            PairStoreError::Read { path, reason } => {
                format!("не удалось прочитать {path}: {reason}")
            }
            PairStoreError::Parse { path, reason } => {
                format!("запись пары {path} испорчена: {reason}")
            }
        }
    }
}

impl std::fmt::Display for PairStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code(), self.message())
    }
}

impl PairStore {
    pub fn new(path: impl Into<PathBuf>) -> PairStore {
        PairStore { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the record. «Файла нет» and «файл битый» are different outcomes
    /// (quality return R2): only the first is «телефон ещё ни с чем не парен».
    pub fn load(&self) -> Remembered {
        let path = self.path.display().to_string();
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Remembered::Absent;
            }
            Err(error) => {
                return Remembered::Broken(PairStoreError::Read {
                    path,
                    reason: error.to_string(),
                });
            }
        };
        let doc: StoredPairing = match serde_json::from_str(&raw) {
            Ok(doc) => doc,
            Err(error) => {
                return Remembered::Broken(PairStoreError::Parse {
                    path,
                    reason: error.to_string(),
                });
            }
        };
        let home = match doc.home.as_deref() {
            Some(recorded) => match parse_home_addr(recorded) {
                Ok(addr) => Some(addr),
                Err(_) => {
                    return Remembered::Broken(PairStoreError::Parse {
                        path,
                        reason: format!("адрес {recorded} не домашний"),
                    });
                }
            },
            None => None,
        };
        Remembered::Record(StoredPairingRead {
            home,
            handled: doc.handled,
        })
    }

    /// Write the remembered home together with the digest of the link that
    /// opened it. EVERY step is a typed failure (R1): a swallowed
    /// `create_dir_all`/`write`/`rename` left the person believing the phone
    /// would re-open the home on the next launch.
    pub fn save(&self, home: SocketAddr, handled: Option<&str>) -> Result<(), PairStoreError> {
        self.write(Some(home), handled)
    }

    /// Remember that a link was ALREADY handled, keeping the home the record
    /// holds (quality return R4: the launching intent is re-read after a
    /// restart, so the mark has to outlive the process).
    pub fn mark_handled(&self, digest: &str) -> Result<(), PairStoreError> {
        let home = match self.load() {
            Remembered::Record(record) => record.home,
            _ => None,
        };
        self.write(home, Some(digest))
    }

    fn write(&self, home: Option<SocketAddr>, handled: Option<&str>) -> Result<(), PairStoreError> {
        let dir = self.path.parent().ok_or(PairStoreError::NoDirectory)?;
        std::fs::create_dir_all(dir).map_err(|error| PairStoreError::CreateDirectory {
            path: dir.display().to_string(),
            reason: error.to_string(),
        })?;
        let body = serde_json::to_string(&StoredPairing {
            home: home.map(|addr| addr.to_string()),
            handled: handled.map(str::to_string),
        })
        .map_err(|error| PairStoreError::Encode {
            reason: error.to_string(),
        })?;
        let tmp = self.path.with_extension(TMP_EXTENSION);
        std::fs::write(&tmp, body).map_err(|error| PairStoreError::Write {
            path: tmp.display().to_string(),
            reason: error.to_string(),
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|error| PairStoreError::Rename {
            path: self.path.display().to_string(),
            reason: error.to_string(),
        })
    }

    /// Forget the previous home — its address and (through the webview's own
    /// cookie store) its session key (REQ-QRAPP-04 AC3). A failed removal is
    /// not swallowed either: the caller reads the error and logs its code and
    /// path (never a token — there is none in this file).
    pub fn forget(&self) -> Result<(), PairStoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PairStoreError::Write {
                path: self.path.display().to_string(),
                reason: error.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> SocketAddr {
        "192.168.1.20:4567".parse().unwrap()
    }

    fn ip(raw: &str) -> Ipv4Addr {
        raw.parse().unwrap()
    }

    fn frontend(path: &str) -> String {
        std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// REQ-QRAPP-01 AC4 + the single-data constraint: the app link and the
    /// browser link are built from one invitation, and the app link carries
    /// that same address and code — a round trip through the strict reader
    /// proves nothing was lost or rewired.
    #[test]
    fn the_app_link_carries_the_same_address_and_code_it_was_built_from() {
        let token = "0123456789abcdef0123456789abcdef";
        let link = app_link(home(), token);
        assert!(link.starts_with("devai://pair?"));
        let parsed = parse_app_link(&link).expect("our own link must parse");
        assert_eq!(parsed.addr, home());
        assert_eq!(parsed.token, token);
        // The colon inside the address is encoded, so the query has exactly
        // two separators and no stray `:` of its own.
        assert!(link.contains("addr=192.168.1.20%3A4567"));
    }

    /// REQ-QRAPP-07 AC1-2: only the exact pairing shape is opened. Everything
    /// else is refused BEFORE a socket is touched.
    #[test]
    fn a_link_that_is_not_exactly_the_pairing_shape_is_refused() {
        for bad in [
            "http://192.168.1.20:4567/?token=abc", // the browser link is not a deep link
            "devai://pair",                        // no query at all
            "devai://pair?",                       // empty query
            "devai://pair/?addr=192.168.1.20:4567&token=abc", // a path
            "devai://pair?addr=192.168.1.20:4567&token=abc&x=1", // extra parameter
            "devai://pair?addr=192.168.1.20:4567", // no token
            "devai://pair?token=abc",              // no address
            "devai://pair?addr=192.168.1.20:4567&addr=192.168.1.21:4567&token=abc",
            "devai://pair?addr=192.168.1.20:4567&token=",
            "devai://pair?addr=8.8.8.8:4567&token=abc", // outside a home network
            "devai://pair?addr=127.0.0.1:4567&token=abc", // the phone's own loopback
            "devai://pair?addr=169.254.1.1:4567&token=abc", // link-local
            "devai://pair?addr=192.168.1.20:0&token=abc", // port 0
            "devai://pair?addr=192.168.1.20:4567&token=abc%00", // binary in the code
            "devai://pair?addr=192.168.1.20:4567&token=abc%zz", // broken escape
            "devai://other?addr=192.168.1.20:4567&token=abc", // another verb
            "devai://pair/../../etc/passwd?addr=192.168.1.20:4567&token=abc",
        ] {
            assert_eq!(
                parse_app_link(bad),
                Err(PairRefusal::BadLink),
                "must be refused: {bad}"
            );
        }
        // The same link in uppercase percent-escapes is still ours.
        let ok = app_link(home(), "abc");
        assert!(parse_app_link(&ok.replace("%3A", "%3a")).is_ok());
    }

    /// REQ-QRAPP-06 constraint: «своя сеть» is the machine's subnet among the
    /// phone's own addresses; loopback/link-local interfaces never count.
    #[test]
    fn the_phone_knows_whether_it_is_in_the_machines_network() {
        let local = vec![ip("192.168.1.42"), ip("127.0.0.1")];
        assert!(same_network(ip("192.168.1.20"), &local));
        assert!(!same_network(ip("192.168.2.20"), &local));
        assert!(!same_network(ip("10.0.0.5"), &local));
        assert!(!same_network(ip("192.168.1.20"), &[ip("127.0.0.1")]));
        assert!(!same_network(ip("192.168.1.20"), &[ip("169.254.3.4")]));
        assert!(!same_network(ip("192.168.1.20"), &[]));
    }

    /// REQ-QRAPP-06 AC1/AC3 + R5: the three refusals come from three
    /// DIFFERENT facts — a used code, a turned-off access and a silent address
    /// are not one generic «ошибка».
    #[test]
    fn the_refusal_follows_the_fact_not_a_generic_error() {
        let local = vec![ip("192.168.1.42")];
        // A presented invitation that the machine refuses is SPENT/REPLACED.
        assert_eq!(
            classify_invite(Probe::Refused, home(), &local),
            Some(PairRefusal::InviteExpired)
        );
        // A machine whose access is off is CLOSED, not «просрочено».
        assert_eq!(
            classify_invite(Probe::Closed, home(), &local),
            Some(PairRefusal::AccessClosed)
        );
        // A live invitation: no refusal at all.
        assert_eq!(classify_invite(Probe::Invite, home(), &local), None);
        // Silence in the phone's own network reads as a closed door; silence
        // outside it is the wrong network.
        assert_eq!(
            classify_invite(Probe::Silent, home(), &local),
            Some(PairRefusal::AccessClosed)
        );
        assert_eq!(
            classify_invite(Probe::Silent, home(), &[ip("10.0.0.9")]),
            Some(PairRefusal::WrongNetwork)
        );
    }

    /// R6: the machine's status is read in ONE place, and only the three known
    /// answers mean something — a proxy page, a broken answer or no answer at
    /// all is silence, never a guessed reason.
    #[test]
    fn the_status_is_read_in_one_place() {
        assert_eq!(probe_from_status(Some(STATUS_INVITE)), Probe::Invite);
        assert_eq!(probe_from_status(Some(STATUS_REFUSED)), Probe::Refused);
        assert_eq!(probe_from_status(Some(STATUS_CLOSED)), Probe::Closed);
        for unknown in [Some(0), Some(204), Some(404), Some(500), None] {
            assert_eq!(
                probe_from_status(unknown),
                Probe::Silent,
                "an unknown answer is silence: {unknown:?}"
            );
        }
        // The four facts are four different states, not aliases.
        let all = [Probe::Invite, Probe::Refused, Probe::Closed, Probe::Silent];
        assert_eq!(all.len(), 4);
        assert!(
            all.iter()
                .all(|p| all.iter().filter(|q| *q == p).count() == 1)
        );
    }

    /// R4: the question the page asks and the link the QR carries are built
    /// from the same shape — the probe address is the machine's own `/pair`
    /// with the same one-time code.
    #[test]
    fn the_probe_address_is_the_machines_own_pair_door() {
        let url = probe_url(home(), "0123456789abcdef");
        assert_eq!(url, "http://192.168.1.20:4567/pair?token=0123456789abcdef");
        // The token is percent-encoded exactly like in the app link, so the
        // two producers cannot disagree.
        assert_eq!(
            probe_url(home(), "a b"),
            "http://192.168.1.20:4567/pair?token=a%20b"
        );
    }

    /// The enum↔data link, checked by the COMPILER: the list is spelled ONCE
    /// and the generated match has no wildcard arm, so a new variant stops the
    /// test build until it is walked here — and the loop below then demands
    /// exactly one row for it in the data file. This is the link the review
    /// found missing between the enum and the hand-kept list.
    macro_rules! declare_refusal_variants {
        ($($variant:ident),+ $(,)?) => {
            const EVERY_REFUSAL: &[PairRefusal] = &[$(PairRefusal::$variant),+];
            fn refusal_is_walked(kind: PairRefusal) {
                match kind {
                    $(PairRefusal::$variant => {})+
                }
            }
        };
    }
    declare_refusal_variants!(InviteExpired, WrongNetwork, AccessClosed, BadLink);

    /// R6: the refusal catalogue is DATA (`data/pair-vocabulary.json`), not a
    /// typed table in code — and every enum variant has exactly one row.
    #[test]
    fn every_refusal_variant_has_exactly_one_data_row() {
        for kind in EVERY_REFUSAL {
            refusal_is_walked(*kind);
            assert_eq!(
                refusals().iter().filter(|row| row.kind == *kind).count(),
                1,
                "{} must have exactly one row in data/pair-vocabulary.json",
                kind.code()
            );
            assert_eq!(kind.row().kind, *kind);
        }
        assert_eq!(
            refusals().len(),
            EVERY_REFUSAL.len(),
            "a row names no variant"
        );
        assert_eq!(PairRefusal::all().count(), refusals().len());
    }

    /// R5: the words are declared ONCE in data and the frontend module is
    /// GENERATED from the same rows — a hand-written mirror cannot drift into a
    /// generic «ошибка» on the phone (REQ-QRAPP-06 AC4).
    #[test]
    fn the_refusal_data_is_rendered_into_the_frontend() {
        let generated = frontend("frontend/src/generated/pair-vocabulary.ts");
        let mut codes: Vec<&str> = Vec::new();
        let mut titles: Vec<&str> = Vec::new();
        for row in refusals() {
            assert_eq!(row.kind.code(), row.code, "a row answers for its kind");
            assert_eq!(row.kind.text(), row.title);
            assert_eq!(row.kind.hint(), row.hint);
            assert!(
                generated.contains(row.code.as_str())
                    && generated.contains(row.title.as_str())
                    && generated.contains(row.hint.as_str()),
                "the generated module does not carry `{}`",
                row.code
            );
            codes.push(row.code.as_str());
            titles.push(row.title.as_str());
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), refusals().len(), "a code is shared");
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), refusals().len(), "a text is shared");
        // The page-side helper does not spell a text of its own: it imports
        // the generated module, so there is exactly one declaration site.
        let state = frontend("frontend/src/pairState.ts");
        assert!(
            state.contains("generated/pair-vocabulary"),
            "pairState.ts must consume the generated vocabulary"
        );
        assert!(
            !state.contains("title: \""),
            "pairState.ts must not declare a title of its own"
        );
    }

    /// R5: the notice rows are in the same data file and reach the page too.
    #[test]
    fn the_notice_data_is_rendered_into_the_frontend() {
        let generated = frontend("frontend/src/generated/pair-vocabulary.ts");
        assert_eq!(notices().len(), 2, "two notices are declared");
        for notice in notices() {
            assert!(
                generated.contains(notice.code.as_str())
                    && generated.contains(notice.title.as_str())
                    && generated.contains(notice.hint.as_str()),
                "the generated module does not carry the notice `{}`",
                notice.code
            );
        }
        // The codes the caller logs are the same ones the data declares.
        assert!(notices().iter().any(|n| n.code == NOTICE_NOT_REMEMBERED));
        assert!(notices().iter().any(|n| n.code == NOTICE_RECORD_BROKEN));
    }

    /// REQ-QRAPP-05 AC1: the machine's answer must fit inside one check
    /// cycle, and the page is given the SAME budget Rust declares — the phone
    /// does not keep two different clocks.
    #[test]
    fn one_check_cycle_is_under_five_seconds_in_both_halves() {
        assert!(
            WATCH_POLL <= std::time::Duration::from_secs(5),
            "the check cycle must stay under five seconds"
        );
        assert!(
            PROBE_TIMEOUT <= WATCH_POLL,
            "the question must fit inside one cycle"
        );
        let mirror = frontend("frontend/src/pairing.ts");
        assert!(
            mirror.contains(&format!("PAIR_POLL_MS = {}", WATCH_POLL.as_millis())),
            "the page's cycle drifted from lan::pair::WATCH_POLL"
        );
    }

    /// The remembered record holds an ADDRESS and a link FINGERPRINT — no
    /// invitation code, no session key (REQ-QRAPP-07 AC3).
    #[test]
    fn the_remembered_home_holds_no_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PairStore::new(dir.path().join("phone-pairing.json"));
        assert_eq!(store.load(), Remembered::Absent, "nothing was ever kept");
        let digest = link_digest(&app_link(home(), "0123456789abcdef"));
        store.save(home(), Some(&digest)).unwrap();
        assert_eq!(
            store.load(),
            Remembered::Record(StoredPairingRead {
                home: Some(home()),
                handled: Some(digest.clone()),
            })
        );
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(!raw.contains("token"));
        assert!(!raw.contains("secret"));
        assert!(
            !raw.contains("0123456789abcdef"),
            "the code itself is not kept"
        );
        assert!(raw.contains("192.168.1.20:4567"));
        store.save(home(), None).unwrap();
        store.forget().unwrap();
        assert_eq!(store.load(), Remembered::Absent);
        // The mark of a handled link survives a launch that keeps the home.
        store.save(home(), None).unwrap();
        store.mark_handled("deadbeefdeadbeef").unwrap();
        assert_eq!(
            store.load(),
            Remembered::Record(StoredPairingRead {
                home: Some(home()),
                handled: Some("deadbeefdeadbeef".into()),
            }),
            "marking a link keeps the remembered home"
        );
    }

    /// The three outcomes of `load` are three different facts (quality return
    /// R2): a garbled record or a public address is NAMED, never silently
    /// dropped to «ничего не помню».
    #[test]
    fn a_broken_record_is_named_not_silently_dropped() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = PairStore::new(dir.path().join("phone-pairing.json"));
        std::fs::write(store.path(), "{ not json").unwrap();
        match store.load() {
            Remembered::Broken(error) => {
                assert_eq!(error.code(), "pair_record_broken");
                assert_eq!(error.code(), PairStoreError::RECORD_BROKEN);
                assert!(error.message().contains("испорчена"));
            }
            other => panic!("a garbled record must be named: {other:?}"),
        }
        std::fs::write(store.path(), "{\"home\":\"8.8.8.8:80\"}").unwrap();
        assert!(matches!(store.load(), Remembered::Broken(_)));
    }

    /// The fingerprint is stable, 16 hex characters and not the link itself.
    #[test]
    fn the_link_fingerprint_is_stable_and_not_the_link() {
        assert_eq!(
            link_digest("devai://pair?addr=a&token=b"),
            link_digest("devai://pair?addr=a&token=b")
        );
        assert_ne!(
            link_digest("devai://pair?addr=a&token=b"),
            link_digest("devai://pair?addr=a&token=c")
        );
        assert_eq!(link_digest("abc").len(), 16);
    }

    /// R1: a write that could not happen is a TYPED, nameable fact — a code,
    /// the path it failed on and a sentence for the person. A swallowed string
    /// left the phone silently forgetting the home it had just paired with.
    #[test]
    fn a_failed_remembered_write_names_the_code_and_the_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, "not a directory").unwrap();
        let store = PairStore::new(blocker.join("phone-pairing.json"));
        let error = store
            .save(home(), None)
            .expect_err("the write cannot succeed");
        assert_eq!(error.code(), "pair_not_remembered");
        assert_eq!(error.code(), PairStoreError::NOT_REMEMBERED);
        let path = error.path().expect("the failing path is named");
        assert!(
            path.contains("blocked"),
            "the failure names where it failed: {path}"
        );
        assert!(
            error.message().contains(&path),
            "the sentence carries the path: {}",
            error.message()
        );
        assert!(
            error.to_string().contains("[pair_not_remembered]"),
            "the log line leads with the code: {error}"
        );
        // Nothing was invented on disk.
        assert!(matches!(
            store.load(),
            Remembered::Absent | Remembered::Broken(_)
        ));
    }

    /// R4: the app's own screens (`#/pair?…`) come from ONE route builder —
    /// the progress screen and the refusal screen cannot drift apart, a base
    /// that already carries a hash does not accumulate a second one, and the
    /// progress address is DIFFERENT for every invitation (its fingerprint is
    /// part of the address, so a warm scan is a real navigation).
    #[test]
    fn the_apps_own_screens_are_built_from_one_route() {
        let base = "http://tauri.localhost/";
        assert_eq!(
            connect_url(base, "0123456789abcdef"),
            "http://tauri.localhost/#/pair?step=connect&scan=0123456789abcdef"
        );
        assert_ne!(
            connect_url(base, "0123456789abcdef"),
            connect_url(base, "fedcba9876543210"),
            "a second invitation is a different address, not the same fragment"
        );
        assert_eq!(
            refusal_url(base, PairRefusal::WrongNetwork, false),
            "http://tauri.localhost/#/pair?reason=wrong_network"
        );
        assert_eq!(
            refusal_url(
                "http://tauri.localhost/#/tasks",
                PairRefusal::AccessClosed,
                false
            ),
            "http://tauri.localhost/#/pair?reason=access_closed"
        );
    }

    /// R4: the route, its query keys and its `connect` value are a contract
    /// with `router.ts`/`PairView.vue` — pinned on BOTH sides, like the poll
    /// interval and the task wire shape, so the page cannot silently stop
    /// understanding an address the native half builds (quality return R4).
    #[test]
    fn the_app_screen_contract_is_pinned_in_the_frontend() {
        let router = frontend("frontend/src/router.ts");
        assert!(
            router.contains("path: \"/pair\""),
            "the app's own route is not registered in router.ts"
        );
        let view = frontend("frontend/src/views/PairView.vue");
        for needle in [
            "route.query.step === \"connect\"",
            "route.query.reason",
            "route.query.return === \"1\"",
        ] {
            assert!(
                view.contains(needle),
                "PairView.vue does not read `{needle}`"
            );
        }
        // The addresses the native half builds carry exactly those names.
        assert!(connect_url("http://x/", "abc").contains("?step=connect&scan=abc"));
        assert!(
            refusal_url("http://x/", PairRefusal::BadLink, true)
                .contains("?reason=pair_bad_link&return=1")
        );
    }

    /// The scan listener lives in `pairingWatch.ts` and keys off the SAME
    /// fragment this module builds; the page constant is pinned here so the
    /// warm-scan fix cannot be lost by renaming one side (quality return R4).
    #[test]
    fn the_warm_scan_listener_uses_the_same_route() {
        let watch = frontend("frontend/src/pairingWatch.ts");
        assert!(
            watch.contains("CONNECT_ROUTE = \"#/pair?step=connect\""),
            "pairingWatch.ts must key off the native connect route"
        );
        assert!(
            watch.contains("addEventListener(\"hashchange\""),
            "the page must notice a same-document address change"
        );
    }

    /// REQ-QRAPP-02: the scheme and host the Android intent filter is written
    /// from are declared in `tauri.conf.json` and must equal the constants the
    /// link parser checks — one declaration, two readers, pinned by a test.
    #[test]
    fn the_deep_link_scheme_is_declared_once() {
        let conf = frontend("tauri.conf.json");
        for needle in [
            format!("\"scheme\": [\"{PAIR_SCHEME}\"]"),
            format!("\"host\": \"{PAIR_HOST}\""),
        ] {
            assert!(
                conf.contains(&needle),
                "tauri.conf.json does not declare `{needle}`"
            );
        }
    }

    /// R1/R4: the notice that the home was not remembered travels in the
    /// address the window opens, and the address it decorates stays the one
    /// producer's address (no second string builder).
    #[test]
    fn the_notice_is_added_to_the_page_the_app_opens() {
        let url = home_url(home(), Some("abc"));
        assert_eq!(url, "http://192.168.1.20:4567/?token=abc");
        assert_eq!(
            with_notice(&url, "pair_not_remembered"),
            "http://192.168.1.20:4567/?token=abc&notice=pair_not_remembered"
        );
        // A page without a query still gets a well-formed address.
        assert_eq!(
            with_notice("http://192.168.1.20:4567/", "pair_not_remembered"),
            "http://192.168.1.20:4567/?notice=pair_not_remembered"
        );
    }

    /// REQ-QRAPP-06 AC4: the `/pair` answer vocabulary is declared ONCE, and
    /// the surface publishes exactly those tokens.
    #[test]
    fn the_pair_answer_states_are_declared_once() {
        let mut unique = vec![STATE_PAIRED, STATE_INVITE, STATE_INVALID, STATE_CLOSED];
        assert_eq!(unique.len(), 4);
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 4, "a state token is shared");
        let surface =
            std::fs::read_to_string(format!("{}/src/lan/http.rs", env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        for state in unique {
            assert!(
                surface.contains(&format!("STATE_{}", state.to_uppercase())),
                "the surface does not publish `{state}`"
            );
        }
    }
}
