//! The app side of the pairing (task 01a1bf78, REQ-QRAPP-02…07): a scanned
//! `devai://pair` link, a remembered home and the periodic question that
//! notices a revoked access.
//!
//! The decision itself lives in [`super::pair`] and is unit-tested there; this
//! module only WIRES it to the window: parse → check the phone's network →
//! validate the code without spending it → open the machine's page in the SAME
//! window (so the phone sees exactly the interface a browser sees, with the
//! same rights), and change the screen when the machine stops answering.
//!
//! Scope: everything below is compiled for a mobile target only. The desktop
//! window has no deep-link handling and no pairing state — its behaviour is
//! unchanged (REQ-QRAPP-08).

// The phone-side half of the pairing is reached by the mobile window only; it
// stays compiled on every target so its decisions are unit-tested by the same
// test run that guards the rest of the crate. Nothing here is reachable from
// the desktop flow — that is exactly REQ-QRAPP-08.
#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr};

use crate::lan::pair::{self, PairLink, PairRefusal};

/// Parse a scanned link and refuse it BEFORE any socket is opened when the
/// phone is not in the machine's network (REQ-QRAPP-06 AC2, REQ-QRAPP-07).
pub fn plan_link(raw: &str, local: &[Ipv4Addr]) -> Result<PairLink, PairRefusal> {
    let link = pair::parse_app_link(raw)?;
    if !pair::same_network_addr(link.addr, local) {
        return Err(PairRefusal::WrongNetwork);
    }
    Ok(link)
}

/// Where the refusal for a link/fact has to be shown: inside the machine's own
/// page while that page is alive (a dead NEW invitation must not cost the
/// working session, ratified Q3), on the bundled app when the machine is the
/// thing that failed.
pub fn refusal_target(
    refusal: PairRefusal,
    current: Option<SocketAddr>,
    base: Option<&str>,
) -> Option<String> {
    match (refusal, current) {
        (PairRefusal::InviteExpired | PairRefusal::BadLink, Some(addr)) => {
            Some(pair::refusal_url(&format!("http://{addr}/"), refusal))
        }
        _ => base.map(|base| pair::refusal_url(base, refusal)),
    }
}

#[cfg(mobile)]
mod mobile {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use tauri::{AppHandle, Manager};
    use tauri_plugin_deep_link::DeepLinkExt;

    use crate::lan::net;
    use crate::lan::pair::PairStore;

    /// The pairing the app is living in right now: the remembered address (no
    /// secrets) and the bundled page used for refusals.
    pub struct PairSession {
        store: PairStore,
        current: Mutex<Option<SocketAddr>>,
        base: Option<String>,
    }

    impl PairSession {
        pub fn new(store: PairStore, base: Option<String>) -> PairSession {
            PairSession {
                store,
                current: Mutex::new(None),
                base,
            }
        }

        fn current(&self) -> Option<SocketAddr> {
            *self.current.lock().unwrap_or_else(|p| p.into_inner())
        }

        fn set_current(&self, addr: Option<SocketAddr>) {
            *self.current.lock().unwrap_or_else(|p| p.into_inner()) = addr;
        }
    }

    /// Called once from `run()` on a mobile target.
    pub fn start(app: &AppHandle, store_path: PathBuf) {
        // The bundled page address is known NOW and stays valid even when the
        // paired machine is gone (REQ-QRAPP-05: the phone must still be able
        // to say «доступ закрыт»).
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
            // The listener runs on the main thread; the pairing question is a
            // socket call, so it goes to a worker and the window keeps drawing.
            let urls: Vec<String> = event
                .urls()
                .into_iter()
                .map(|url| url.to_string())
                .collect();
            let handle = handle.clone();
            std::thread::spawn(move || {
                for url in urls {
                    accept(&handle, &url);
                }
            });
        });

        // A cold start: the intent that launched the process is the first link.
        // The plugin learns that address when the webview loads, which may
        // happen AFTER this setup — so the question is asked once more from a
        // helper thread when the window is there. Asking on the main thread
        // would wait for the Android plugin inside its own startup.
        let cold = app.clone();
        std::thread::Builder::new()
            .name("devai-pair-start".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(700));
                if cold.state::<PairSession>().current().is_some() {
                    return; // a link already opened a home through the event
                }
                match cold.deep_link().get_current() {
                    Ok(Some(urls)) if !urls.is_empty() => {
                        for url in &urls {
                            accept(&cold, url.as_str());
                        }
                    }
                    _ => resume(&cold),
                }
            })
            .ok();

        watch(app.clone());
    }

    /// A scanned link: parse, check the network, validate the one-time code
    /// WITHOUT spending it (the page load spends it), then open the home.
    fn accept(app: &AppHandle, raw: &str) {
        let local = net::local_ipv4s();
        let link = match plan_link(raw, &local) {
            Ok(link) => link,
            Err(refusal) => {
                refuse(app, refusal);
                return;
            }
        };
        let probe = pair::probe_pair(link.addr, Some(&link.token), None, pair::PROBE_TIMEOUT);
        if let Some(refusal) = pair::classify_invite(probe, link.addr, &local) {
            refuse(app, refusal);
            return;
        }
        open(app, link.addr, Some(&link.token));
    }

    /// Show `addr` inside the app window. A different home forgets the
    /// previous one — its address and its session key (REQ-QRAPP-04 AC3) — and
    /// is remembered only as the address (REQ-QRAPP-07 AC3).
    fn open(app: &AppHandle, addr: SocketAddr, token: Option<&str>) {
        let session = app.state::<PairSession>();
        // Switching means LEAVING a different home: only then is the previous
        // address and (through the webview's cookie store) its session key
        // forgotten (REQ-QRAPP-04 AC3). Re-opening the same remembered home
        // must keep the cookie it was just confirmed with.
        let previous = session.current().or_else(|| session.store.load());
        if previous.is_some_and(|previous| previous != addr) {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.clear_all_browsing_data();
            }
        }
        let _ = session.store.save(addr);
        session.set_current(Some(addr));
        navigate(app, &pair::home_url(addr, token));
    }

    /// Say why an invitation was not accepted. The words come from ONE
    /// vocabulary (`pair::PairRefusal`), the screen from the frontend mirror.
    fn refuse(app: &AppHandle, refusal: PairRefusal) {
        let session = app.state::<PairSession>();
        match refusal_target(refusal, session.current(), session.base.as_deref()) {
            Some(url) => navigate(app, &url),
            None => eprintln!(
                "devai-ui: [{}] приглашение не принято ({})",
                refusal.code(),
                refusal.text()
            ),
        }
    }

    /// A launch without a link: re-open the remembered home, but only if its
    /// session still lives; otherwise say what happened (ratified Q4).
    fn resume(app: &AppHandle) {
        let session = app.state::<PairSession>();
        let Some(addr) = session.store.load() else {
            return;
        };
        let local = net::local_ipv4s();
        if !pair::same_network_addr(addr, &local) {
            refuse(app, PairRefusal::WrongNetwork);
            return;
        }
        // `current` is set only once the session is confirmed (by `open`):
        // otherwise a refused resume would keep the watcher knocking on a dead
        // door every cycle.
        let probe = pair::probe_pair(
            addr,
            None,
            cookie_for(app, addr).as_deref(),
            pair::PROBE_TIMEOUT,
        );
        match pair::classify(probe, addr, &local) {
            None => open(app, addr, None),
            Some(refusal) => refuse(app, refusal),
        }
    }

    /// The app notices a revoked access by itself: one cheap question every
    /// few seconds, because after the machine turns the access off nothing is
    /// sent from it any more (REQ-QRAPP-05 constraint). The screen leaves the
    /// dead home — the app does not keep showing data under a spent code.
    fn watch(app: AppHandle) {
        std::thread::Builder::new()
            .name("devai-pair-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(pair::WATCH_POLL);
                    let session = app.state::<PairSession>();
                    let Some(addr) = session.current() else {
                        continue;
                    };
                    let local = net::local_ipv4s();
                    let probe = pair::probe_pair(
                        addr,
                        None,
                        cookie_for(&app, addr).as_deref(),
                        pair::PROBE_TIMEOUT,
                    );
                    match pair::classify(probe, addr, &local) {
                        None => {}
                        Some(refusal) => {
                            session.set_current(None);
                            refuse(&app, refusal);
                        }
                    }
                }
            })
            .ok();
    }

    /// The session secret as the webview stores it — read, never logged and
    /// never written to our own files (REQ-QRAPP-07 AC3).
    fn cookie_for(app: &AppHandle, addr: SocketAddr) -> Option<String> {
        let url = tauri::Url::parse(&format!("http://{addr}/")).ok()?;
        let window = app.get_webview_window("main")?;
        let cookies = window.cookies_for_url(url).ok()?;
        cookies
            .iter()
            .find(|cookie| cookie.name() == "devai_lan")
            .map(|cookie| cookie.value().to_string())
    }

    fn navigate(app: &AppHandle, url: &str) {
        let Ok(url) = tauri::Url::parse(url) else {
            return;
        };
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.navigate(url);
        }
    }
}

#[cfg(mobile)]
pub use mobile::start;

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> Ipv4Addr {
        raw.parse().unwrap()
    }

    fn addr(raw: &str) -> SocketAddr {
        raw.parse().unwrap()
    }

    /// REQ-QRAPP-06 AC2: a link to another network is refused by comparing the
    /// invitation's address with the phone's own addresses — before a socket
    /// is opened.
    #[test]
    fn a_link_from_another_network_is_named_as_such() {
        let local = vec![ip("192.168.1.42")];
        let link = pair::app_link(addr("192.168.1.20:4567"), "abc");
        assert!(plan_link(&link, &local).is_ok());
        let other = pair::app_link(addr("10.0.0.5:4567"), "abc");
        assert_eq!(plan_link(&other, &local), Err(PairRefusal::WrongNetwork));
        // A broken link is a bad link, not a network fact.
        assert_eq!(
            plan_link("http://192.168.1.20:4567/", &local),
            Err(PairRefusal::BadLink)
        );
    }

    /// Ratified Q3: a dead NEW invitation is shown INSIDE the working home; a
    /// revoked/absent machine is shown on the bundled app, which needs no
    /// network at all.
    #[test]
    fn a_refusal_lands_where_the_working_session_is_not_lost() {
        let current = Some(addr("192.168.1.20:4567"));
        assert_eq!(
            refusal_target(
                PairRefusal::InviteExpired,
                current,
                Some("http://tauri.localhost/")
            ),
            Some("http://192.168.1.20:4567/#/pair?reason=invite_expired".into())
        );
        assert_eq!(
            refusal_target(
                PairRefusal::AccessClosed,
                current,
                Some("http://tauri.localhost/")
            ),
            Some("http://tauri.localhost/#/pair?reason=access_closed".into())
        );
        assert_eq!(
            refusal_target(
                PairRefusal::WrongNetwork,
                None,
                Some("http://tauri.localhost/#/")
            ),
            Some("http://tauri.localhost/#/pair?reason=wrong_network".into())
        );
        // Without a bundled address there is nowhere honest to point at.
        assert_eq!(refusal_target(PairRefusal::AccessClosed, None, None), None);
    }
}
