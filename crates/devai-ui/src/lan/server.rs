//! The listener lifecycle: bind the machine's home-segment address, serve the
//! HTTP surface on its own thread, stop on demand, restore at startup
//! (REQ-LAN-01, REQ-LAN-03, ratified Q1).
//!
//! The order matters and is the reason `activate` is called only AFTER the
//! bind succeeded: the window must never say «включён» while the socket is
//! not there (REQ-LAN-01 AC2/AC4).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use crate::commands::UiError;
use crate::lan::http::{AppSurface, routes};
use crate::lan::net::{local_ipv4s, pick_lan_address, spawn_watchdog};
use crate::lan::state::{LanAccess, LanStatus};

/// Bind, spawn the server thread and mark the access on. Returns the status
/// the window renders. Called by the `lan_enable` command and at startup when
/// the persisted flag says the access was on (ratified Q1).
pub fn start_access(app: tauri::AppHandle, lan: Arc<LanAccess>) -> Result<LanStatus, UiError> {
    if lan.is_on() {
        return Ok(lan.status());
    }
    let Some(ip) = pick_lan_address(&local_ipv4s()) else {
        return Err(refuse(
            &lan,
            "no_local_address",
            "у машины нет адреса в локальной сети — включить доступ нельзя".to_string(),
        ));
    };
    let listener = bind_listener(&lan, ip)?;
    let address = listener_address(&lan, ip, &listener)?;
    let (tx, rx) = tokio::sync::watch::channel(false);
    lan.activate(address, tx);
    spawn_server(app, lan.clone(), listener, rx)?;
    if let IpAddr::V4(bound) = address.ip() {
        spawn_watchdog(lan.clone(), bound);
    }
    // NOT the token: the address is the only fact that may reach the log.
    eprintln!("devai-ui: доступ с телефона включён на {address}");
    Ok(lan.status())
}

/// «отключить»: state off immediately, every session revoked, the listener
/// told to stop (REQ-LAN-09 AC4).
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

/// Port 0: the OS picks a free port; the QR carries it, so there is nothing to
/// configure and nothing to collide with.
fn bind_listener(lan: &LanAccess, ip: Ipv4Addr) -> Result<std::net::TcpListener, UiError> {
    match std::net::TcpListener::bind(SocketAddr::from((ip, 0))) {
        Ok(listener) => {
            let _ = listener.set_nonblocking(true);
            Ok(listener)
        }
        Err(e) => Err(refuse(
            lan,
            "bind_failed",
            format!("не удалось открыть порт на {ip}: {e}"),
        )),
    }
}

fn listener_address(
    lan: &LanAccess,
    ip: Ipv4Addr,
    listener: &std::net::TcpListener,
) -> Result<SocketAddr, UiError> {
    listener.local_addr().map_err(|e| {
        refuse(
            lan,
            "bind_failed",
            format!("не удалось определить порт на {ip}: {e}"),
        )
    })
}

/// A listener without a thread is not an access: on any failure the state is
/// turned off with the reason, never left claiming «включён».
fn spawn_server(
    app: tauri::AppHandle,
    lan: Arc<LanAccess>,
    listener: std::net::TcpListener,
    rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), UiError> {
    let server_lan = lan.clone();
    let spawned = std::thread::Builder::new()
        .name("devai-lan".into())
        .spawn(move || {
            // The process is not a tokio app: the server owns its runtime so
            // the access can stop cleanly without touching the window loop.
            let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            else {
                server_lan.refuse("runtime_failed", "не удалось запустить сервер доступа");
                return;
            };
            rt.block_on(serve(listener, rx, app, server_lan));
        });
    match spawned {
        Ok(_) => Ok(()),
        Err(e) => Err(refuse(
            &lan,
            "runtime_failed",
            format!("не удалось запустить поток доступа: {e}"),
        )),
    }
}

async fn serve(
    listener: std::net::TcpListener,
    rx: tokio::sync::watch::Receiver<bool>,
    app: tauri::AppHandle,
    lan: Arc<LanAccess>,
) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        lan.refuse("bind_failed", "не удалось открыть порт доступа");
        return;
    };
    let surface = Arc::new(AppSurface::new(app));
    let served = axum::serve(
        listener,
        routes(lan, surface).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(wait_shutdown(rx));
    if let Err(e) = served.await {
        eprintln!("devai-ui: LAN-сервер остановлен: {e}");
    }
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

/// Turn the access off and hand the refusal back as the typed error the user
/// sees. The state is the single place that records the reason.
fn refuse(lan: &LanAccess, code: &'static str, message: String) -> UiError {
    lan.refuse(code, message.clone());
    UiError {
        code: code.into(),
        message,
    }
}
