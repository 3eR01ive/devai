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
//! Desktop has no pairing state at all: both commands answer `idle`, nothing
//! navigates, and the window keeps its own flow (REQ-QRAPP-08).

// The pairing handshake is reached by the mobile window only; it stays
// compiled on every target so its decisions are unit-tested by the same test
// run that guards the rest of the crate.
#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use serde::Serialize;

use crate::lan::pair::{self, PairLink, PairRefusal};

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
    /// The remembered home is not reachable from this network any more.
    Gone(PairRefusal),
    /// Re-open this home with the session the phone already has.
    Home { addr: SocketAddr, url: String },
}

/// Re-open the remembered home — WITHOUT a question: on a top-level page load
/// the machine itself decides whether the session still lives (a dead one
/// answers «доступ закрыт»), and only the network comparison is local.
pub fn resume(remembered: Option<SocketAddr>, local: &[Ipv4Addr]) -> Resumed {
    let Some(addr) = remembered else {
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

#[cfg(mobile)]
mod mobile {
    use std::sync::Mutex;

    use tauri::{AppHandle, Manager};
    use tauri_plugin_deep_link::DeepLinkExt;

    use super::*;
    use crate::lan::net;
    use crate::lan::pair::{PairStore, PairStoreError};

    /// Everything the pairing knows right now. One lock, so the boot, the
    /// scan and the answer cannot interleave halfway.
    #[derive(Default)]
    struct Inner {
        /// The home the window is showing (or was told to show).
        current: Option<SocketAddr>,
        /// A link the window has to act on, taken by exactly one boot.
        pending: Option<String>,
        /// The last raw link this process already handled: `get_current`
        /// keeps returning the launching intent, and handling it twice would
        /// loop between the app page and the machine page.
        handled: Option<String>,
        /// The invitation a boot handed to the page; an answer is accepted
        /// only for this one.
        asking: Option<PairLink>,
    }

    pub struct PairSession {
        store: PairStore,
        inner: Mutex<Inner>,
        base: Option<String>,
    }

    impl PairSession {
        pub fn new(store: PairStore, base: Option<String>) -> PairSession {
            PairSession {
                store,
                inner: Mutex::new(Inner::default()),
                base,
            }
        }

        fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut inner)
        }

        fn current(&self) -> Option<SocketAddr> {
            self.with(|inner| inner.current)
        }

        fn base(&self) -> Option<&str> {
            self.base.as_deref()
        }

        /// The next boot takes the new link; any older question is dropped.
        fn hold(&self, raw: &str) {
            self.with(|inner| {
                inner.pending = Some(raw.to_string());
                inner.handled = Some(raw.to_string());
                inner.asking = None;
            });
        }

        fn take_pending(&self) -> Option<String> {
            self.with(|inner| {
                inner.asking = None;
                inner.pending.take()
            })
        }

        /// `true` when this raw link is new to the process.
        fn first_time(&self, raw: &str) -> bool {
            self.with(|inner| {
                if inner.handled.as_deref() == Some(raw) {
                    return false;
                }
                inner.handled = Some(raw.to_string());
                true
            })
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
        app.manage(PairSession::new(PairStore::new(store_path), base));

        // A link that arrives while the app runs (Android singleTask keeps one
        // activity, so the second scan lands in the SAME window — REQ-QRAPP-02
        // AC4, REQ-QRAPP-04 AC1).
        let handle = app.clone();
        app.deep_link().on_open_url(move |event| {
            for url in event.urls() {
                accept_scan(&handle, url.as_str());
            }
        });
    }

    /// Remember the scanned link and bring the app's OWN page up: the page has
    /// the IPC bridge (a machine page does not), asks the boot question and
    /// carries the decision out. The progress screen is the app's own route, so
    /// no unrelated home flashes while the machine is asked.
    fn accept_scan(app: &AppHandle, raw: &str) {
        let session = app.state::<PairSession>();
        session.hold(raw);
        let (Some(window), Some(base)) = (app.get_webview_window("main"), session.base()) else {
            return;
        };
        if let Ok(url) = tauri::Url::parse(&pair::connect_url(base)) {
            let _ = window.navigate(url);
        }
    }

    /// The page's first question: what to do now.
    pub fn boot(app: &AppHandle) -> PairTask {
        let session = app.state::<PairSession>();
        session.with(|inner| inner.asking = None);
        if let Some(raw) = session.take_pending() {
            return start_link(app, &raw);
        }
        match launched_link(app) {
            Some(raw) if session.first_time(&raw) => start_link(app, &raw),
            _ => resume_home(app),
        }
    }

    /// The intent that launched the process. The plugin learns it while the
    /// page loads, which is exactly when the page asks — no sleep and no
    /// second thread.
    fn launched_link(app: &AppHandle) -> Option<String> {
        let urls = app.deep_link().get_current().ok().flatten()?;
        urls.into_iter().next().map(|url| url.to_string())
    }

    fn start_link(app: &AppHandle, raw: &str) -> PairTask {
        let session = app.state::<PairSession>();
        let local = net::local_ipv4s();
        match plan_link(raw, &local) {
            Ok(link) => {
                session.with(|inner| inner.asking = Some(link.clone()));
                ask_task(&link)
            }
            Err(refusal) => {
                let task = refusal_task(refusal, session.current(), session.base());
                if matches!(task, PairTask::Idle) {
                    // A refusal with no screen: the code still reaches the
                    // journal (the token never does).
                    log_refusal(refusal);
                }
                task
            }
        }
    }

    /// The page reported what the machine answered. Only the asked invitation
    /// counts; the code is spent by the page load that follows, never here.
    pub fn report(app: &AppHandle, addr: &str, status: Option<u16>) -> PairTask {
        let session = app.state::<PairSession>();
        let asking = session.with(|inner| inner.asking.take());
        let local = net::local_ipv4s();
        match answer(
            asking.as_ref(),
            addr,
            status,
            &local,
            session.current(),
            session.base(),
        ) {
            PairOutcome::Ignored => PairTask::Idle,
            PairOutcome::Open { addr, url } => opened(app, &session, addr, url),
            PairOutcome::Refuse { url, refusal } => refuse(app, url, refusal),
        }
    }

    /// A different home forgets the previous one — its address and its session
    /// key (REQ-QRAPP-04 AC3) — and is remembered as the address only
    /// (REQ-QRAPP-07 AC3). A failure to remember is VISIBLE (R1).
    fn opened(app: &AppHandle, session: &PairSession, addr: SocketAddr, url: String) -> PairTask {
        let previous = session.current().or_else(|| session.store.load());
        if previous.is_some_and(|previous| previous != addr) {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.clear_all_browsing_data();
            }
        }
        session.with(|inner| inner.current = Some(addr));
        match session.store.save(addr) {
            Ok(()) => go(app, url),
            Err(error) => {
                eprintln!("devai-ui: сохранение дома пары: {error}");
                go(app, pair::with_notice(&url, PairStoreError::CODE))
            }
        }
    }

    /// Open the page in THIS window, through the window's own API (the same
    /// navigation a browser does, but owned by the shell — no second load and
    /// no double-spent invitation).
    fn go(app: &AppHandle, url: String) -> PairTask {
        if let (Some(window), Ok(target)) =
            (app.get_webview_window("main"), tauri::Url::parse(&url))
        {
            let _ = window.navigate(target);
        }
        PairTask::go(url)
    }

    fn refuse(app: &AppHandle, url: Option<String>, refusal: PairRefusal) -> PairTask {
        match url {
            Some(url) => go(app, url),
            None => {
                log_refusal(refusal);
                PairTask::Idle
            }
        }
    }

    fn resume_home(app: &AppHandle) -> PairTask {
        let session = app.state::<PairSession>();
        match resume(session.store.load(), &net::local_ipv4s()) {
            Resumed::Nothing => PairTask::Idle,
            Resumed::Gone(refusal) => {
                refuse(app, refusal_target(refusal, None, session.base()), refusal)
            }
            Resumed::Home { addr, url } => {
                session.with(|inner| inner.current = Some(addr));
                go(app, url)
            }
        }
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

    /// Ratified Q4: a launch without a link re-opens the remembered home, and
    /// only the network is checked locally — whether the session still lives
    /// is answered by the machine's own page (a dead one says «доступ закрыт»).
    #[test]
    fn a_launch_without_a_link_returns_to_the_remembered_home() {
        assert_eq!(resume(None, &local()), Resumed::Nothing);
        assert_eq!(
            resume(Some(home()), &local()),
            Resumed::Home {
                addr: home(),
                url: "http://192.168.1.20:4567/".into(),
            }
        );
        assert_eq!(
            resume(Some(home()), &[ip("10.0.0.9")]),
            Resumed::Gone(PairRefusal::WrongNetwork)
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
}
