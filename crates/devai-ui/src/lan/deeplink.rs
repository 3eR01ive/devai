//! The app side of the pairing (task 01a1bf78, REQ-QRAPP-02…07): a scanned
//! `devai://pair` link, a remembered home and the one question the interface
//! asks about it.
//!
//! Division of labour, forced by the project's `no-network` gate (local-first
//! sovereignty forbids outbound sockets in our own code — the LAN access is a
//! SERVER, and the phone's interface is a page):
//!
//!   * this process parses the link, decides whether the phone is in the
//!     machine's network, remembers the home and picks the screen to show;
//!   * the page (the app's own window content) asks the machine ONE cheap
//!     question — `GET /pair` — and reports the status back;
//!   * the answer never spends the one-time code: opening the page does that,
//!     and the page is the same interface a browser gets (REQ-QRAPP-02 AC5).
//!
//! Why the window first goes back to its OWN page before a scan is handled:
//! the machine's page is a remote origin, and Tauri gives the IPC bridge to
//! the app's own origin only. The bundled page has it, so the handshake runs
//! there and then the window opens the machine's page (top level — the session
//! cookie is SameSite, so an iframe would never receive it).
//!
//! A scan that arrives while the app is ALREADY showing its own page is a
//! same-document navigation (only the fragment changes), so `onMounted` does
//! not run again. The address therefore carries the link's fingerprint
//! ([`pair::connect_url`]) and the page listens for the address change
//! (`frontend/src/pairingWatch.ts`) — otherwise the person is left on
//! «подключаюсь к дому…» for ever (quality return R1, warm scan).
//!
//! Desktop has no pairing SESSION: nothing is constructed, nothing navigates,
//! and both commands answer `idle`, so the window keeps its own flow
//! (REQ-QRAPP-08).

// The pairing handshake is reached by the mobile window only; it stays
// compiled on every target so its decisions are unit-tested by the same test
// run that guards the rest of the crate.
#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::lan::pair::{self, PairLink, PairRefusal, PairStore, PairStoreError, Remembered};

/// The address a refusal belongs on while a home works. A dead NEW invitation
/// must never cost the working session (ratified Q3, quality return R2): every
/// reason — not only «просрочено» — stays on the current home.
pub fn refusal_base(current: Option<SocketAddr>, base: Option<&str>) -> Option<String> {
    match current {
        // The machine's own root, built by the ONE address producer.
        Some(addr) => Some(pair::home_url(addr, None)),
        None => base.map(str::to_string),
    }
}

/// Where the refusal of a NEW invitation is shown. `Some` = the screen exists;
/// `None` = there is nowhere honest to point at (the caller logs the code).
pub fn refusal_target(
    refusal: PairRefusal,
    current: Option<SocketAddr>,
    base: Option<&str>,
) -> Option<String> {
    refusal_base(current, base).map(|base| pair::refusal_url(&base, refusal, current.is_some()))
}

/// Parse a scanned link and refuse it BEFORE anything is opened when the phone
/// is not in the machine's network (REQ-QRAPP-06 AC2, REQ-QRAPP-07).
pub fn plan_link(raw: &str, local: &[Ipv4Addr]) -> Result<PairLink, PairRefusal> {
    let link = pair::parse_app_link(raw)?;
    if !pair::same_network_addr(link.addr, local) {
        return Err(PairRefusal::WrongNetwork);
    }
    Ok(link)
}

/// What the window has to do next. It is a VALUE: the decision is made here,
/// the page only carries it out, so the whole flow is readable and testable
/// without a window (quality return R6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PairTask {
    /// Nothing to do: no link and no remembered home (always on desktop).
    Idle,
    /// Ask the machine this one question and report the status back.
    Probe {
        url: String,
        addr: String,
        timeout_ms: u64,
    },
    /// Open this page now (a paired home or a refusal screen).
    Go { url: String },
}

impl PairTask {
    pub fn go(url: impl Into<String>) -> PairTask {
        PairTask::Go { url: url.into() }
    }
}

/// The task for one scanned (or launching) link.
pub fn link_task(
    raw: &str,
    local: &[Ipv4Addr],
    current: Option<SocketAddr>,
    base: Option<&str>,
) -> PairTask {
    match plan_link(raw, local) {
        Ok(link) => ask_task(&link),
        Err(refusal) => refusal_task(refusal, current, base),
    }
}

/// The question about a concrete invitation. Never spends it.
pub fn ask_task(link: &PairLink) -> PairTask {
    PairTask::Probe {
        url: pair::probe_url(link.addr, &link.token),
        addr: link.addr.to_string(),
        timeout_ms: pair::PROBE_TIMEOUT.as_millis() as u64,
    }
}

/// The screen for a refusal, or `Idle` when there is nowhere to show it (the
/// code still reaches the log through [`log_refusal`]).
pub fn refusal_task(
    refusal: PairRefusal,
    current: Option<SocketAddr>,
    base: Option<&str>,
) -> PairTask {
    match refusal_target(refusal, current, base) {
        Some(url) => PairTask::go(url),
        None => PairTask::Idle,
    }
}

/// What a launch without a link means (ratified Q4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resumed {
    /// No home was ever remembered.
    Nothing,
    /// The remembered record exists but cannot be read: the phone forgot the
    /// home and says so (quality return R2 — a broken record is not «never
    /// paired»).
    Broken(PairStoreError),
    /// The remembered home is not reachable from this network any more.
    Gone(PairRefusal),
    /// Re-open this home with the session the phone already has.
    Home { addr: SocketAddr, url: String },
}

/// Re-open the remembered home — WITHOUT a question: on a top-level page load
/// the machine itself decides whether the session still lives (a dead one
/// answers «доступ закрыт»), and only the network comparison is local.
pub fn resume(remembered: Remembered, local: &[Ipv4Addr]) -> Resumed {
    let record = match remembered {
        Remembered::Absent => return Resumed::Nothing,
        Remembered::Broken(error) => return Resumed::Broken(error),
        Remembered::Record(record) => record,
    };
    let Some(addr) = record.home else {
        return Resumed::Nothing;
    };
    if !pair::same_network_addr(addr, local) {
        return Resumed::Gone(PairRefusal::WrongNetwork);
    }
    Resumed::Home {
        url: pair::home_url(addr, None),
        addr,
    }
}

/// What a reported status means for the invitation that was asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairOutcome {
    /// No question is pending (a stale page answered): change nothing.
    Ignored,
    /// Open this home: the invitation is alive.
    Open { addr: SocketAddr, url: String },
    /// Say why. `url` is `None` when there is no screen to say it on.
    Refuse {
        url: Option<String>,
        refusal: PairRefusal,
    },
}

/// The decision after the page reported the machine's status. The invitation
/// must be the one this process asked about — an answer for another address
/// changes nothing (a second scan may have replaced the question).
pub fn answer(
    asking: Option<&PairLink>,
    addr: &str,
    status: Option<u16>,
    local: &[Ipv4Addr],
    current: Option<SocketAddr>,
    base: Option<&str>,
) -> PairOutcome {
    let Some(link) = asking.filter(|link| link.addr.to_string() == addr) else {
        return PairOutcome::Ignored;
    };
    let probe = pair::probe_from_status(status);
    match pair::classify_invite(probe, link.addr, local) {
        None => PairOutcome::Open {
            addr: link.addr,
            url: pair::home_url(link.addr, Some(&link.token)),
        },
        Some(refusal) => PairOutcome::Refuse {
            url: refusal_target(refusal, current, base),
            refusal,
        },
    }
}

/// The refusal is a fact of the run even when no screen exists: the code and
/// the words go to the journal, the token never does (REQ-QRAPP-07 AC3).
pub fn log_refusal(refusal: PairRefusal) {
    eprintln!(
        "devai-ui: [{}] приглашение не принято ({})",
        refusal.code(),
        refusal.text()
    );
}

// ---------------------------------------------------------------------------
// the window's half: it holds the pending link and answers the two commands
// ---------------------------------------------------------------------------

/// The window's two side effects, behind a trait: the phone's half owns the
/// webview, a test owns a recorder — so every branch of the wiring (scan,
/// boot, report, switch, refusal, resume) runs without a device (quality
/// return R7). Neither method returns a secret-bearing string: `open` answers
/// only whether the window took the address, because the address carries the
/// one-time code and must never reach a log.
pub trait Screen: Send + Sync {
    /// Carry the ONE window to this page.
    fn open(&self, url: &str) -> bool;
    /// Drop the previous home's cookies and its session key (REQ-QRAPP-04 AC3).
    fn forget_session(&self) -> bool;
}

/// The log code of a window that refused a navigation (no address, no code:
/// the address carries the one-time token).
const NAVIGATE_FAILED: &str = "pair_navigate_failed";
/// The log code of a webview that could not drop the previous home's cookies.
const SESSION_NOT_CLEARED: &str = "pair_session_not_cleared";

/// Everything the pairing knows right now. One lock, so the boot, the scan and
/// the answer cannot interleave halfway.
#[derive(Default)]
struct Inner {
    /// The home the window is showing (or was told to show).
    current: Option<SocketAddr>,
    /// A link the window has to act on, taken by exactly one boot.
    pending: Option<String>,
    /// The last raw link this process already handled: `get_current` keeps
    /// returning the launching intent, and handling it twice would loop
    /// between the app page and the machine page.
    handled: Option<String>,
    /// The digest of that link, so the mark outlives the process (R4).
    digest: Option<String>,
    /// The invitation a boot handed to the page; an answer is accepted only
    /// for this one.
    asking: Option<PairLink>,
}

/// The pairing of the phone's window: what was scanned, what is open, what the
/// machine answered. No Tauri type appears here — the window is [`Screen`].
pub struct PairSession {
    store: PairStore,
    inner: Mutex<Inner>,
    base: Option<String>,
    screen: Arc<dyn Screen>,
}

impl PairSession {
    pub fn new(store: PairStore, base: Option<String>, screen: Arc<dyn Screen>) -> PairSession {
        PairSession {
            store,
            inner: Mutex::new(Inner::default()),
            base,
            screen,
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut inner)
    }

    fn current(&self) -> Option<SocketAddr> {
        self.with(|inner| inner.current)
    }

    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// A scanned link is taken and the app's OWN page comes up: the page has
    /// the IPC bridge (a machine page does not), asks the boot question and
    /// carries the decision out. The progress screen is the app's own route, so
    /// no unrelated home flashes while the machine is asked.
    ///
    /// The SAME intent delivered twice (Android replays the launch intent, and
    /// the plugin may hand over the same link again) is taken ONCE: it is
    /// already being handled, so a second `pending` would only loop the window
    /// back to the progress screen (quality return R2). A link that was
    /// REFUSED is not held and can be scanned again (see [`Self::release`]).
    pub fn accept_scan(&self, raw: &str) -> PairTask {
        let digest = pair::link_digest(raw);
        let repeat = self.with(|inner| {
            if inner.handled.as_deref() == Some(raw) {
                return true;
            }
            inner.pending = Some(raw.to_string());
            inner.handled = Some(raw.to_string());
            inner.digest = Some(digest.clone());
            inner.asking = None;
            false
        });
        if repeat {
            return PairTask::Idle;
        }
        match self.base() {
            Some(base) => self.go(pair::connect_url(base, &digest)),
            None => PairTask::Idle,
        }
    }

    /// The page's first question: what to do now. `launched` is the intent that
    /// started the process (the plugin reads it while the page loads).
    ///
    /// A handshake that was interrupted (the page reloaded before it reported)
    /// is ASKED AGAIN instead of being dropped: the question never spends the
    /// code, so retrying is safe — and the «продолжить» action on the progress
    /// screen has a real pass to run (quality return R1).
    pub fn boot(&self, launched: Option<&str>, local: &[Ipv4Addr]) -> PairTask {
        if let Some(raw) = self.with(|inner| inner.pending.take()) {
            self.with(|inner| inner.asking = None);
            return self.start_link(&raw, local);
        }
        if let Some(link) = self.with(|inner| inner.asking.clone()) {
            return ask_task(&link);
        }
        match launched {
            Some(raw) if self.first_time(raw) => self.start_link(raw, local),
            _ => self.resume_home(local),
        }
    }

    /// `true` when this link was never handled by THIS PHONE — neither by this
    /// process nor by an earlier one. Android re-reads the launching intent on
    /// every `load(webView)`, so an in-memory marker alone replayed a spent
    /// invitation after a restart (quality return R4); the digest of the link
    /// is kept on disk instead, and the link itself — and its code — never is.
    ///
    /// The mark is WRITTEN by [`Self::opened`], after the invitation actually
    /// opened the home. A refused link writes nothing: the person must be able
    /// to scan it again after fixing the network or getting a new code, and
    /// nothing was spent by the question (quality return R2 — a mark on a
    /// refusal would make the app swallow the retry).
    fn first_time(&self, raw: &str) -> bool {
        let digest = pair::link_digest(raw);
        let seen_here = self.with(|inner| {
            if inner.handled.as_deref() == Some(raw) {
                return true;
            }
            inner.handled = Some(raw.to_string());
            inner.digest = Some(digest.clone());
            false
        });
        if seen_here {
            return false;
        }
        match self.store.load() {
            Remembered::Record(record) if record.handled.as_deref() == Some(digest.as_str()) => {
                return false;
            }
            // A record nobody can read is a fact to say out loud before the
            // next write overwrites it.
            Remembered::Broken(error) => eprintln!("devai-ui: отметка о приглашении: {error}"),
            _ => {}
        }
        true
    }

    /// The invitation was NOT opened: drop its mark, so the same code can be
    /// delivered again. Only a successful open is remembered (see
    /// [`Self::first_time`]); holding a refused link would leave the person on
    /// a dead screen with no way to retry.
    fn release(&self) {
        self.with(|inner| {
            inner.handled = None;
            inner.digest = None;
        });
    }

    pub fn start_link(&self, raw: &str, local: &[Ipv4Addr]) -> PairTask {
        match plan_link(raw, local) {
            Ok(link) => {
                self.with(|inner| inner.asking = Some(link.clone()));
                ask_task(&link)
            }
            Err(refusal) => {
                // A refusal is a SCREEN, and the window must actually open it:
                // returning `Go` without navigating left the person on the
                // progress screen (the caller only carries out `Idle`, and the
                // native half owns navigation).
                let url = refusal_target(refusal, self.current(), self.base());
                self.refuse(url, refusal)
            }
        }
    }

    /// The page reported what the machine answered. Only the asked invitation
    /// counts; the code is spent by the page load that follows, never here.
    pub fn report(&self, addr: &str, status: Option<u16>, local: &[Ipv4Addr]) -> PairTask {
        let asking = self.with(|inner| inner.asking.take());
        match answer(
            asking.as_ref(),
            addr,
            status,
            local,
            self.current(),
            self.base(),
        ) {
            PairOutcome::Ignored => PairTask::Idle,
            PairOutcome::Open { addr, url } => self.opened(addr, url),
            PairOutcome::Refuse { url, refusal } => self.refuse(url, refusal),
        }
    }

    /// A different home forgets the previous one — its address and its session
    /// key (REQ-QRAPP-04 AC3) — and is remembered as an address plus the digest
    /// of the handled link (REQ-QRAPP-07 AC3). A failure to remember is VISIBLE
    /// (R1): the opened page carries the notice.
    fn opened(&self, addr: SocketAddr, url: String) -> PairTask {
        let previous = self.current().or_else(|| match self.store.load() {
            Remembered::Record(record) => record.home,
            _ => None,
        });
        if previous.is_some_and(|previous| previous != addr) && !self.screen.forget_session() {
            eprintln!("devai-ui: [{SESSION_NOT_CLEARED}] окно не сбросило прежний сеанс");
        }
        self.with(|inner| inner.current = Some(addr));
        let digest = self.with(|inner| inner.digest.clone());
        match self.store.save(addr, digest.as_deref()) {
            Ok(()) => self.go(url),
            Err(error) => {
                eprintln!("devai-ui: сохранение дома пары: {error}");
                self.go(pair::with_notice(&url, PairStoreError::NOT_REMEMBERED))
            }
        }
    }

    /// Open the page in THIS window, through the window's own API (the same
    /// navigation a browser does, but owned by the shell — no second load and
    /// no double-spent invitation). A window that refused says so by code; the
    /// address is not echoed (it carries the one-time code), and the task is
    /// `Idle` because nothing was opened.
    fn go(&self, url: String) -> PairTask {
        if self.screen.open(&url) {
            PairTask::go(url)
        } else {
            eprintln!("devai-ui: [{NAVIGATE_FAILED}] окно не открыло адрес приглашения");
            PairTask::Idle
        }
    }

    fn refuse(&self, url: Option<String>, refusal: PairRefusal) -> PairTask {
        self.release();
        match url {
            Some(url) => self.go(url),
            None => {
                log_refusal(refusal);
                PairTask::Idle
            }
        }
    }

    /// No link this boot: re-open the remembered home, or say why it is not
    /// there. A record that cannot be read is forgotten and named (R2) instead
    /// of looking exactly like a phone that never paired.
    fn resume_home(&self, local: &[Ipv4Addr]) -> PairTask {
        match resume(self.store.load(), local) {
            Resumed::Nothing => PairTask::Idle,
            Resumed::Broken(error) => {
                eprintln!("devai-ui: [{0}] запись пары: {error}", error.code());
                if let Err(forget) = self.store.forget() {
                    eprintln!("devai-ui: забыть испорченную запись: {forget}");
                }
                match self.base() {
                    Some(base) => self.go(pair::with_notice(base, PairStoreError::RECORD_BROKEN)),
                    None => PairTask::Idle,
                }
            }
            Resumed::Gone(refusal) => {
                self.refuse(refusal_target(refusal, None, self.base()), refusal)
            }
            Resumed::Home { addr, url } => {
                self.with(|inner| inner.current = Some(addr));
                self.go(url)
            }
        }
    }
}

#[cfg(test)]
impl PairSession {
    /// The remembered record as the tests read it: a test that expects a home
    /// or a digest and finds an unreadable file must fail, not pass silently.
    fn store_load_for_tests(&self) -> pair::StoredPairingRead {
        match self.store.load() {
            Remembered::Record(record) => record,
            other => panic!("the remembered record must be readable: {other:?}"),
        }
    }
}

#[cfg(mobile)]
mod mobile {
    use tauri::{AppHandle, Manager};
    use tauri_plugin_deep_link::DeepLinkExt;

    use super::*;
    use crate::lan::net;
    use crate::lan::pair::PairStore;

    /// The real window: the ONE webview the app shows.
    struct PhoneWindow {
        app: AppHandle,
    }

    impl Screen for PhoneWindow {
        fn open(&self, url: &str) -> bool {
            let (Some(window), Ok(target)) =
                (self.app.get_webview_window("main"), tauri::Url::parse(url))
            else {
                return false;
            };
            window.navigate(target).is_ok()
        }

        fn forget_session(&self) -> bool {
            self.app
                .get_webview_window("main")
                .is_some_and(|window| window.clear_all_browsing_data().is_ok())
        }
    }

    /// Called once from `run()` on a mobile target.
    pub fn start(app: &AppHandle, store_path: PathBuf) {
        // The bundled page address is known NOW and stays valid even when the
        // paired machine is gone: the refusal screen has to exist somewhere.
        let base = app
            .get_webview_window("main")
            .and_then(|window| window.url().ok())
            .map(|url| url.to_string());
        app.manage(PairSession::new(
            PairStore::new(store_path),
            base,
            Arc::new(PhoneWindow { app: app.clone() }),
        ));

        // A link that arrives while the app runs (Android singleTask keeps one
        // activity, so the second scan lands in the SAME window — REQ-QRAPP-02
        // AC4, REQ-QRAPP-04 AC1).
        let handle = app.clone();
        app.deep_link().on_open_url(move |event| {
            let session = handle.state::<PairSession>();
            for url in event.urls() {
                let _ = session.accept_scan(url.as_str());
            }
        });
    }

    /// The page's first question. The launching intent is read HERE — the
    /// plugin learns it while the page loads, which is exactly when the page
    /// asks — and handed to the session, so the decision itself stays testable.
    pub fn boot(app: &AppHandle) -> PairTask {
        let launched = launched_link(app);
        app.state::<PairSession>()
            .boot(launched.as_deref(), &net::local_ipv4s())
    }

    pub fn report(app: &AppHandle, addr: &str, status: Option<u16>) -> PairTask {
        app.state::<PairSession>()
            .report(addr, status, &net::local_ipv4s())
    }

    /// The intent that launched the process. No sleep and no second thread: the
    /// plugin has it by the time the page asks.
    fn launched_link(app: &AppHandle) -> Option<String> {
        let urls = app.deep_link().get_current().ok().flatten()?;
        urls.into_iter().next().map(|url| url.to_string())
    }
}

#[cfg(not(mobile))]
mod mobile {
    //! Desktop: no pairing state, no navigation, no new screen — both commands
    //! answer «нечего делать» (REQ-QRAPP-08).
    use super::*;

    pub fn start(_app: &tauri::AppHandle, _store_path: PathBuf) {}

    pub fn boot(_app: &tauri::AppHandle) -> PairTask {
        PairTask::Idle
    }

    pub fn report(_app: &tauri::AppHandle, _addr: &str, _status: Option<u16>) -> PairTask {
        PairTask::Idle
    }
}

#[allow(unused_imports)] // desktop registers nothing to start
pub use mobile::start;

/// The window asks what to do (once per page mount, in the app's own page).
#[tauri::command]
pub fn pair_boot(app: tauri::AppHandle) -> PairTask {
    mobile::boot(&app)
}

/// The window reports the status it read from `GET /pair`; the answer is the
/// next thing to do.
#[tauri::command]
pub fn pair_report(app: tauri::AppHandle, addr: String, status: Option<u16>) -> PairTask {
    mobile::report(&app, &addr, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> Ipv4Addr {
        raw.parse().unwrap()
    }

    fn addr(raw: &str) -> SocketAddr {
        raw.parse().unwrap()
    }

    fn home() -> SocketAddr {
        addr("192.168.1.20:4567")
    }

    fn local() -> Vec<Ipv4Addr> {
        vec![ip("192.168.1.42")]
    }

    /// REQ-QRAPP-06 AC2: a link to another network is refused by comparing the
    /// invitation's address with the phone's own addresses — before anything
    /// is opened and before any question is asked.
    #[test]
    fn a_link_from_another_network_is_named_as_such() {
        let link = pair::app_link(home(), "abc");
        assert!(plan_link(&link, &local()).is_ok());
        let other = pair::app_link(addr("10.0.0.5:4567"), "abc");
        assert_eq!(plan_link(&other, &local()), Err(PairRefusal::WrongNetwork));
        // A broken link is a bad link, not a network fact.
        assert_eq!(
            plan_link("http://192.168.1.20:4567/", &local()),
            Err(PairRefusal::BadLink)
        );
    }

    /// R2: a refusal NEVER moves the window off the working home — every
    /// reason, not only «просрочено» (the review's counter-example was exactly
    /// «не та сеть» while a home was open).
    #[test]
    fn a_refusal_never_leaves_the_working_home() {
        let current = Some(home());
        let base = Some("http://tauri.localhost/");
        for refusal in [
            PairRefusal::InviteExpired,
            PairRefusal::WrongNetwork,
            PairRefusal::AccessClosed,
            PairRefusal::BadLink,
        ] {
            let target = refusal_target(refusal, current, base).expect("a screen exists");
            assert!(
                target.starts_with("http://192.168.1.20:4567/#/pair?reason="),
                "{} must stay on the open home, got {target}",
                refusal.code()
            );
            assert!(target.contains("&return=1"), "the way back is offered");
            assert!(target.contains(refusal.code()));
        }
        // With no home open, the bundled page shows it and offers no way back.
        let target = refusal_target(PairRefusal::WrongNetwork, None, base).unwrap();
        assert_eq!(target, "http://tauri.localhost/#/pair?reason=wrong_network");
        // Nowhere to point at: the caller logs the code instead.
        assert_eq!(refusal_target(PairRefusal::AccessClosed, None, None), None);
    }

    /// The whole path of a scanned invitation: parse → ask → answer → open.
    /// A live code is asked about WITHOUT being spent (the page load spends
    /// it), and the task carries the machine's address and the code.
    #[test]
    fn a_scanned_invitation_asks_and_then_opens_the_home() {
        let token = "0123456789abcdef";
        let raw = pair::app_link(home(), token);
        let task = link_task(&raw, &local(), None, Some("http://tauri.localhost/"));
        let PairTask::Probe {
            url,
            addr: asked,
            timeout_ms,
        } = task.clone()
        else {
            panic!("a valid invitation must be asked about: {task:?}");
        };
        assert_eq!(asked, "192.168.1.20:4567");
        assert_eq!(url, "http://192.168.1.20:4567/pair?token=0123456789abcdef");
        assert_eq!(timeout_ms, pair::PROBE_TIMEOUT.as_millis() as u64);

        let link = plan_link(&raw, &local()).unwrap();
        let outcome = answer(
            Some(&link),
            &asked,
            Some(pair::STATUS_INVITE),
            &local(),
            None,
            Some("http://tauri.localhost/"),
        );
        assert_eq!(
            outcome,
            PairOutcome::Open {
                addr: home(),
                url: "http://192.168.1.20:4567/?token=0123456789abcdef".into(),
            }
        );
    }

    /// R5: the three ways an invitation dies are told apart by the status the
    /// page read — «просрочено», «доступ закрыт» and silence (which in the
    /// machine's network means a closed door).
    #[test]
    fn a_spent_closed_or_silent_invitation_is_named_apart() {
        let link = plan_link(&pair::app_link(home(), "abc"), &local()).unwrap();
        let base = Some("http://tauri.localhost/");
        let cases = [
            (Some(pair::STATUS_REFUSED), PairRefusal::InviteExpired),
            (Some(pair::STATUS_CLOSED), PairRefusal::AccessClosed),
            (None, PairRefusal::AccessClosed),
            (Some(500), PairRefusal::AccessClosed),
        ];
        for (status, expected) in cases {
            let outcome = answer(
                Some(&link),
                "192.168.1.20:4567",
                status,
                &local(),
                None,
                base,
            );
            match outcome {
                PairOutcome::Refuse { url, refusal } => {
                    assert_eq!(refusal, expected, "status {status:?}");
                    let url = url.expect("the bundled page is there");
                    assert!(url.contains(expected.code()), "{url}");
                }
                other => panic!("must be refused: {other:?}"),
            }
        }
    }

    /// A refusal about a NEW invitation while a home works is shown on THAT
    /// home — the working session is never traded for a dead code.
    #[test]
    fn a_refused_invitation_is_shown_on_the_working_home() {
        let link = plan_link(&pair::app_link(home(), "abc"), &local()).unwrap();
        let outcome = answer(
            Some(&link),
            "192.168.1.20:4567",
            Some(pair::STATUS_REFUSED),
            &local(),
            Some(addr("192.168.1.30:4567")),
            Some("http://tauri.localhost/"),
        );
        match outcome {
            PairOutcome::Refuse { url, refusal } => {
                assert_eq!(refusal, PairRefusal::InviteExpired);
                assert_eq!(
                    url.as_deref(),
                    Some("http://192.168.1.30:4567/#/pair?reason=invite_expired&return=1")
                );
            }
            other => panic!("must be refused: {other:?}"),
        }
    }

    /// R5 edge case: a second scan replaces the question. An answer for the
    /// invitation nobody asked about any more changes NOTHING — the window
    /// does not jump to a home the person no longer pointed at.
    #[test]
    fn a_stale_answer_changes_nothing() {
        let first = plan_link(&pair::app_link(home(), "one"), &local()).unwrap();
        let base = Some("http://tauri.localhost/");
        assert_eq!(
            answer(None, "192.168.1.20:4567", Some(200), &local(), None, base),
            PairOutcome::Ignored
        );
        assert_eq!(
            answer(
                Some(&first),
                "192.168.1.99:4567",
                Some(200),
                &local(),
                None,
                base
            ),
            PairOutcome::Ignored,
            "another address is not the asked invitation"
        );
        // And a question about a link whose network changed is a refusal, not
        // an open home.
        let outcome = answer(Some(&first), "192.168.1.20:4567", None, &[], None, base);
        assert!(matches!(
            outcome,
            PairOutcome::Refuse {
                refusal: PairRefusal::WrongNetwork,
                ..
            }
        ));
    }

    fn remembered(addr: SocketAddr, handled: Option<&str>) -> Remembered {
        Remembered::Record(pair::StoredPairingRead {
            home: Some(addr),
            handled: handled.map(str::to_string),
        })
    }

    /// Ratified Q4: a launch without a link re-opens the remembered home, and
    /// only the network is checked locally — whether the session still lives
    /// is answered by the machine's own page (a dead one says «доступ закрыт»).
    #[test]
    fn a_launch_without_a_link_returns_to_the_remembered_home() {
        assert_eq!(resume(Remembered::Absent, &local()), Resumed::Nothing);
        assert_eq!(
            resume(remembered(home(), None), &local()),
            Resumed::Home {
                addr: home(),
                url: "http://192.168.1.20:4567/".into(),
            }
        );
        assert_eq!(
            resume(remembered(home(), None), &[ip("10.0.0.9")]),
            Resumed::Gone(PairRefusal::WrongNetwork)
        );
        // A record with no home in it (a scan whose home write never ran) is
        // «нечего открывать», not a home.
        assert_eq!(
            resume(
                Remembered::Record(pair::StoredPairingRead::default()),
                &local()
            ),
            Resumed::Nothing
        );
        // A home that has gone missing from the phone's network is said in
        // words on the bundled page, not silently skipped.
        let task = refusal_task(
            PairRefusal::WrongNetwork,
            None,
            Some("http://tauri.localhost/"),
        );
        assert_eq!(
            task,
            PairTask::go("http://tauri.localhost/#/pair?reason=wrong_network")
        );
    }

    /// R2: a record that cannot be read is its OWN outcome — the caller says
    /// so and forgets it; it never looks like a phone that never paired.
    #[test]
    fn a_broken_record_is_named_instead_of_looking_absent() {
        let error = PairStoreError::Parse {
            path: "/data/phone-pairing.json".into(),
            reason: "expected value at line 1".into(),
        };
        assert_eq!(
            resume(Remembered::Broken(error.clone()), &local()),
            Resumed::Broken(error)
        );
    }

    /// REQ-QRAPP-04 AC5: a dead NEW invitation while a home works is shown on
    /// that home and does NOT navigate to the new machine — not even when the
    /// refusal is «не та сеть» (the review's exact counter-example).
    #[test]
    fn a_dead_new_invitation_stays_on_the_current_home() {
        let current = Some(home());
        // The new machine is in ANOTHER network: named «не та сеть», shown on
        // the working home.
        let elsewhere = pair::app_link(addr("10.0.0.30:4567"), "abc");
        assert_eq!(
            link_task(
                &elsewhere,
                &local(),
                current,
                Some("http://tauri.localhost/")
            ),
            PairTask::go("http://192.168.1.20:4567/#/pair?reason=wrong_network&return=1")
        );
        // The new machine IS in the phone's network: the invitation is asked
        // about, and the machine's refusal lands on the CURRENT home.
        let nearby = pair::app_link(addr("192.168.1.30:4567"), "abc");
        let PairTask::Probe { addr: asked, .. } = link_task(&nearby, &local(), current, None)
        else {
            panic!("the invitation is in the phone's network");
        };
        assert_eq!(asked, "192.168.1.30:4567");
        let link = plan_link(&nearby, &local()).unwrap();
        let outcome = answer(
            Some(&link),
            &asked,
            Some(pair::STATUS_REFUSED),
            &local(),
            current,
            Some("http://tauri.localhost/"),
        );
        assert_eq!(
            outcome,
            PairOutcome::Refuse {
                url: Some("http://192.168.1.20:4567/#/pair?reason=invite_expired&return=1".into()),
                refusal: PairRefusal::InviteExpired,
            }
        );
    }

    /// The handshake's wire shape is a contract with `frontend/src/pairingNative.ts`:
    /// the tag and the field names are pinned, so the page cannot silently stop
    /// understanding the task.
    #[test]
    fn the_task_wire_shape_is_pinned_in_the_frontend() {
        let task = PairTask::Probe {
            url: "http://192.168.1.20:4567/pair?token=abc".into(),
            addr: "192.168.1.20:4567".into(),
            timeout_ms: 1500,
        };
        let value = serde_json::to_value(&task).unwrap();
        assert_eq!(value["kind"], "probe");
        for key in ["kind", "url", "addr", "timeout_ms"] {
            assert!(value.get(key).is_some(), "the task carries `{key}`");
        }
        let page = std::fs::read_to_string(format!(
            "{}/frontend/src/pairingNative.ts",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        for token in ["\"probe\"", "\"go\"", "\"idle\"", "timeout_ms"] {
            assert!(
                page.contains(token),
                "pairingNative.ts does not speak `{token}`"
            );
        }
        assert!(serde_json::to_value(PairTask::Idle).unwrap()["kind"] == "idle");
    }

    // -----------------------------------------------------------------------
    // the wiring itself (quality return R7): every branch below runs without a
    // device — the window is a recorder, the store is a temp file
    // -----------------------------------------------------------------------

    use std::sync::atomic::{AtomicBool, Ordering};

    /// A [`Screen`] that records what the window was told to do. It can also
    /// refuse the address, which is the branch a real webview takes when the
    /// navigation fails.
    #[derive(Default)]
    struct Recorder {
        opened: Mutex<Vec<String>>,
        forgotten: Mutex<usize>,
        refuse: AtomicBool,
    }

    impl Recorder {
        fn opened(&self) -> Vec<String> {
            self.opened.lock().unwrap().clone()
        }

        fn last(&self) -> String {
            self.opened()
                .pop()
                .expect("the window was told to open something")
        }

        fn forgotten(&self) -> usize {
            *self.forgotten.lock().unwrap()
        }
    }

    impl Screen for Recorder {
        fn open(&self, url: &str) -> bool {
            if self.refuse.load(Ordering::SeqCst) {
                return false;
            }
            self.opened.lock().unwrap().push(url.to_string());
            true
        }

        fn forget_session(&self) -> bool {
            *self.forgotten.lock().unwrap() += 1;
            true
        }
    }

    /// One phone: a temp store, the bundled page as its base, a recorder as the
    /// window.
    fn phone(dir: &tempfile::TempDir) -> (PairSession, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let session = PairSession::new(
            PairStore::new(dir.path().join("phone-pairing.json")),
            Some("http://tauri.localhost/".into()),
            recorder.clone(),
        );
        (session, recorder)
    }

    /// A scanned link: the progress screen comes up (the person sees the scan
    /// was taken), and the next boot asks the machine WITHOUT spending the code.
    /// The address carries the link's FINGERPRINT, so a second invitation is a
    /// real address change rather than the same fragment (warm scan, R1).
    #[test]
    fn a_scan_shows_the_progress_screen_and_then_asks_the_machine() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        let raw = pair::app_link(home(), "abc");
        let connect = pair::connect_url("http://tauri.localhost/", &pair::link_digest(&raw));

        let task = session.accept_scan(&raw);
        assert_eq!(task, PairTask::go(connect.clone()));
        assert_eq!(window.last(), connect);
        assert!(
            connect.contains("scan="),
            "the progress address must differ per invitation: {connect}"
        );

        let PairTask::Probe {
            url, addr: asked, ..
        } = session.boot(None, &local())
        else {
            panic!("a scanned invitation must be asked about");
        };
        assert_eq!(asked, "192.168.1.20:4567");
        assert_eq!(url, "http://192.168.1.20:4567/pair?token=abc");
    }

    /// R2: the SAME intent delivered twice while the process runs is taken
    /// once. A second `pending` would send the window back to the progress
    /// screen and replay the question instead of the one already under way.
    #[test]
    fn the_same_scan_delivered_twice_is_taken_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        let raw = pair::app_link(home(), "abc");
        assert!(matches!(session.accept_scan(&raw), PairTask::Go { .. }));
        assert_eq!(
            session.accept_scan(&raw),
            PairTask::Idle,
            "a repeated delivery of the same invitation is not re-armed"
        );
        assert_eq!(
            window.opened().len(),
            1,
            "the window did not navigate again"
        );
        // The one taken link is asked about; a boot while it is still
        // unanswered repeats the QUESTION (safe, nothing is spent) instead of
        // dropping the person on a dead progress screen.
        assert!(matches!(
            session.boot(None, &local()),
            PairTask::Probe { .. }
        ));
        assert!(matches!(
            session.boot(None, &local()),
            PairTask::Probe { .. }
        ));
    }

    /// R2: a refused invitation is RELEASED, not remembered — after fixing the
    /// network the same code can be delivered again. A mark on a refusal would
    /// make the app swallow the retry and leave the person on a dead screen.
    #[test]
    fn a_refused_scan_can_be_delivered_again() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        // The first delivery is refused by the NETWORK check, before any ask.
        let elsewhere = pair::app_link(addr("10.0.0.5:4567"), "abc");
        assert!(matches!(
            session.accept_scan(&elsewhere),
            PairTask::Go { .. }
        ));
        let task = session.boot(None, &local());
        assert!(matches!(task, PairTask::Go { .. }), "refusal screen shown");
        assert!(
            window.last().contains("reason=wrong_network"),
            "{}",
            window.last()
        );
        // The same link is taken again (the network may be fixed by now).
        assert!(
            matches!(session.accept_scan(&elsewhere), PairTask::Go { .. }),
            "a refused invitation must remain retryable"
        );
    }

    /// A live invitation opens the machine's page in the SAME window and the
    /// home is remembered — address only, plus the link's fingerprint.
    #[test]
    fn a_live_answer_opens_the_machine_page_and_remembers_the_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        let raw = pair::app_link(home(), "abc");
        session.accept_scan(&raw);
        let PairTask::Probe { addr: asked, .. } = session.boot(None, &local()) else {
            panic!("probe expected");
        };
        let task = session.report(&asked, Some(pair::STATUS_INVITE), &local());
        assert_eq!(task, PairTask::go("http://192.168.1.20:4567/?token=abc"));
        assert_eq!(window.last(), "http://192.168.1.20:4567/?token=abc");
        // The home is remembered; the code is not.
        let remembered = session.store_load_for_tests();
        assert_eq!(remembered.home, Some(home()));
        assert_eq!(remembered.handled, Some(pair::link_digest(&raw)));
        // The first home needs no cookie wipe: there was nothing before it.
        assert_eq!(window.forgotten(), 0);
    }

    /// A question answered twice — or answered by a page nobody asked — changes
    /// nothing: the window does not jump to a home the person no longer
    /// pointed at.
    #[test]
    fn an_answer_to_a_question_nobody_asked_changes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        assert_eq!(
            session.report("192.168.1.20:4567", Some(200), &local()),
            PairTask::Idle
        );
        assert!(window.opened().is_empty(), "nothing was opened");
        // The same invitation reported twice: only the first answer counts.
        session.accept_scan(&pair::app_link(home(), "abc"));
        let _ = session.boot(None, &local());
        let _ = session.report("192.168.1.20:4567", Some(200), &local());
        let opened = window.opened().len();
        assert_eq!(
            session.report("192.168.1.20:4567", Some(200), &local()),
            PairTask::Idle
        );
        assert_eq!(
            window.opened().len(),
            opened,
            "the second answer opened nothing"
        );
    }

    /// A dead NEW invitation never trades the working home for itself: the
    /// refusal is shown ON the open home, with the way back.
    #[test]
    fn a_dead_new_invitation_is_shown_on_the_open_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        // Pair with the first machine.
        session.accept_scan(&pair::app_link(home(), "one"));
        let PairTask::Probe { addr: asked, .. } = session.boot(None, &local()) else {
            panic!("probe expected");
        };
        let _ = session.report(&asked, Some(pair::STATUS_INVITE), &local());

        // A second machine in the same network refuses its invitation.
        let second = addr("192.168.1.30:4567");
        session.accept_scan(&pair::app_link(second, "two"));
        let PairTask::Probe { addr: asked, .. } = session.boot(None, &local()) else {
            panic!("the second invitation must be asked about");
        };
        assert_eq!(asked, "192.168.1.30:4567");
        let task = session.report(&asked, Some(pair::STATUS_REFUSED), &local());
        assert_eq!(
            task,
            PairTask::go("http://192.168.1.20:4567/#/pair?reason=invite_expired&return=1")
        );
        assert_eq!(
            window.last(),
            "http://192.168.1.20:4567/#/pair?reason=invite_expired&return=1",
            "the working home keeps the screen"
        );
    }

    /// A new invitation switches the open home: the previous session is wiped,
    /// the new home is remembered, and no page of the old home is shown.
    #[test]
    fn a_new_invitation_switches_the_home_and_forgets_the_old_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        let first = addr("192.168.1.20:4567");
        let second = addr("192.168.1.30:4567");
        session.accept_scan(&pair::app_link(first, "one"));
        let PairTask::Probe { .. } = session.boot(None, &local()) else {
            panic!("probe expected");
        };
        let _ = session.report("192.168.1.20:4567", Some(pair::STATUS_INVITE), &local());

        session.accept_scan(&pair::app_link(second, "two"));
        let PairTask::Probe { addr: asked, .. } = session.boot(None, &local()) else {
            panic!("the second invitation must be asked about");
        };
        let task = session.report(&asked, Some(pair::STATUS_INVITE), &local());
        assert_eq!(task, PairTask::go("http://192.168.1.30:4567/?token=two"));
        assert_eq!(window.forgotten(), 1, "the old session key is dropped");
        assert_eq!(session.store_load_for_tests().home, Some(second));
        assert!(
            !window
                .opened()
                .iter()
                .skip(2)
                .any(|url| url.contains("192.168.1.20:4567")),
            "no page of the previous home is opened after the switch"
        );
    }

    /// R4: Android re-reads the launching intent after a restart. The SAME
    /// link is handled once; the next process resumes the remembered home
    /// instead of asking about a spent invitation again.
    #[test]
    fn a_replayed_launching_intent_is_handled_only_once_per_phone() {
        let dir = tempfile::TempDir::new().unwrap();
        let raw = pair::app_link(home(), "abc");
        {
            let (session, _window) = phone(&dir);
            assert!(
                matches!(session.boot(Some(&raw), &local()), PairTask::Probe { .. }),
                "the first launch handles the link"
            );
            let _ = session.report("192.168.1.20:4567", Some(pair::STATUS_INVITE), &local());
        }
        // A new process over the same store file: the intent is replayed.
        let (restarted, window) = phone(&dir);
        let task = restarted.boot(Some(&raw), &local());
        assert_eq!(
            task,
            PairTask::go("http://192.168.1.20:4567/"),
            "the replayed intent must not be handled again"
        );
        assert_eq!(window.last(), "http://192.168.1.20:4567/");
    }

    /// R2: a record that cannot be read is said on the bundled page and
    /// forgotten — the person reads it instead of a silent fresh start.
    #[test]
    fn a_broken_record_is_named_on_the_bundled_page() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("phone-pairing.json"), "{ not json").unwrap();
        let (session, window) = phone(&dir);
        let task = session.boot(None, &local());
        assert_eq!(
            task,
            PairTask::go("http://tauri.localhost/?notice=pair_record_broken")
        );
        assert_eq!(
            window.last(),
            "http://tauri.localhost/?notice=pair_record_broken"
        );
        assert!(
            !dir.path().join("phone-pairing.json").exists(),
            "the record nobody can read is forgotten"
        );
    }

    /// A window that refuses the address is not reported as opened — the fact
    /// reaches the journal by code, never by echoing the address (it carries
    /// the one-time code).
    #[test]
    fn a_window_that_refuses_the_address_is_not_reported_as_opened() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        session.accept_scan(&pair::app_link(home(), "abc"));
        let _ = session.boot(None, &local());
        window.refuse.store(true, Ordering::SeqCst);
        let task = session.report("192.168.1.20:4567", Some(pair::STATUS_INVITE), &local());
        assert_eq!(task, PairTask::Idle, "nothing was opened");
        assert_eq!(
            session.store_load_for_tests().home,
            Some(home()),
            "the home is still remembered even when the window refused"
        );
    }

    /// A scan with no window page to show it on still takes the link; nothing
    /// is navigated, and the boot asks the machine.
    #[test]
    fn a_scan_without_a_base_page_still_takes_the_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let window = Arc::new(Recorder::default());
        let session = PairSession::new(
            PairStore::new(dir.path().join("phone-pairing.json")),
            None,
            window.clone(),
        );
        assert_eq!(
            session.accept_scan(&pair::app_link(home(), "abc")),
            PairTask::Idle
        );
        assert!(window.opened().is_empty());
        assert!(matches!(
            session.boot(None, &local()),
            PairTask::Probe { .. }
        ));
    }

    /// A launch with nothing scanned and nothing remembered has nothing to do
    /// (the desktop answer, from the phone's own state machine).
    #[test]
    fn a_launch_with_nothing_to_do_answers_idle() {
        let dir = tempfile::TempDir::new().unwrap();
        let (session, window) = phone(&dir);
        assert_eq!(session.boot(None, &local()), PairTask::Idle);
        assert!(window.opened().is_empty());
    }
}
