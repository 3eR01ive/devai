//! The HTTP boundary of the phone access: the SAME frontend bundle the window
//! renders, plus the RPC door that dispatches to the window's command plane
//! (REQ-LAN-02, REQ-LAN-04, REQ-LAN-05).
//!
//! The boundary knows nothing about domain data. It knows: how a client
//! proves a paired session, which asset is asked for, and that the JSON
//! envelope is `{command, params}` → `{ok, data | error}`. Everything else
//! goes through `crate::lan::commands`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::commands::UiError;
use crate::lan::commands::dispatch_rpc;
use crate::lan::pair::{PairRefusal, STATE_CLOSED, STATE_INVALID, STATE_INVITE, STATE_PAIRED};
use crate::lan::state::{DeviceClaim, LanAccess, PageAuth, PairState};

const SESSION_COOKIE: &str = "devai_lan";

/// The session cookie is kept on the device, not only in the tab: the app
/// remembers the home it was paired with and re-opens it on the next launch
/// (ratified Q4). There is no clock-based expiry anywhere in the access — the
/// ratified decision of 09.11 is «живёт, пока его явно не закроют» — so the
/// cookie's lifetime is long and the door is closed by the machine, not by
/// time.
const SESSION_MAX_AGE_SECS: u64 = 365 * 24 * 60 * 60;

/// One embedded asset of the frontend bundle.
pub struct Asset {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Everything the HTTP boundary needs from the process it serves. The window
/// supplies the Tauri implementation (embedded assets + the command plane);
/// tests supply a fake, so the WHOLE request path — token → cookie → RPC —
/// runs without a display or a real socket.
pub trait Surface: Send + Sync + 'static {
    fn asset(&self, path: &str) -> Option<Asset>;
    fn dispatch(
        &self,
        device: &DeviceClaim,
        command: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, UiError>;
}

pub struct AppSurface {
    app: tauri::AppHandle,
}

impl AppSurface {
    pub fn new(app: tauri::AppHandle) -> AppSurface {
        AppSurface { app }
    }
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

#[derive(serde::Deserialize)]
struct RpcRequest {
    command: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// The routes the phone may reach. Kept beside the handlers so a route
/// cannot exist without a handler and vice versa; the shape is registered
/// for second consumers by `lan::commands::http_surface_is_registered`.
pub fn routes(lan: Arc<LanAccess>, surface: Arc<dyn Surface>) -> axum::Router {
    let ctx = ServerCtx { lan, surface };
    axum::Router::new()
        .route("/", axum::routing::get(page))
        .route("/pair", axum::routing::get(pair_probe))
        .route("/rpc", axum::routing::post(rpc))
        .route("/*path", axum::routing::get(asset))
        .with_state(ctx)
}

/// Normalize a request path into an asset id, or refuse it. The path is
/// CLIENT INPUT (quality return R7): `..`, absolute escapes, backslashes,
/// percent-encoding tricks and drive prefixes are rejected instead of being
/// passed to the asset resolver. The bundle's own names are plain relative
/// segments, so nothing legitimate is lost.
pub fn asset_path(uri_path: &str) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();
    for segment in uri_path.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            s if s.contains(['\\', '\0', '%', ':']) => return None,
            s => segments.push(s),
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// `GET /pair[?token=…]` — the phone's cheap health question, and the only
/// answer a revoked access can still get (REQ-QRAPP-05): it carries no data,
/// only the fact. `200 paired` = this device's session lives; `200 invite` =
/// the presented one-time code is valid and NOT spent; `401` = the code or the
/// session is dead; `403` = the access is off.
///
/// The answer is readable from ANOTHER origin on purpose (`ACAO: *`): the app
/// page asks the candidate machine this question before the window switches
/// homes, and a switch that cannot be checked would cost the working session
/// (quality return R2). Nothing but the three tokens above is revealed, no
/// cookie is accepted on that path, and the request never spends a code.
async fn pair_probe(
    axum::extract::State(ctx): axum::extract::State<ServerCtx>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let token = query.get("token").map(String::as_str);
    let cookie = cookie_value(&headers, SESSION_COOKIE);
    let (status, body) = match ctx.lan.pair_state(token, cookie.as_deref()) {
        PairState::Paired => (
            axum::http::StatusCode::OK,
            serde_json::json!({ "state": STATE_PAIRED }),
        ),
        PairState::Invite => (
            axum::http::StatusCode::OK,
            serde_json::json!({ "state": STATE_INVITE }),
        ),
        PairState::Refused(code) => (
            axum::http::StatusCode::UNAUTHORIZED,
            serde_json::json!({ "state": STATE_INVALID, "code": code }),
        ),
        PairState::Off => (
            axum::http::StatusCode::FORBIDDEN,
            serde_json::json!({ "state": STATE_CLOSED, "code": "lan_off" }),
        ),
    };
    json_readable_across_origins(status, body)
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
    let cookie = cookie_value(&headers, SESSION_COOKIE);
    match ctx.lan.page_load(token, cookie.as_deref(), &device) {
        PageAuth::Deny { code, message } => {
            if code == "lan_off" {
                refusal_page(axum::http::StatusCode::FORBIDDEN, code, &message)
            } else {
                refusal_page(axum::http::StatusCode::UNAUTHORIZED, code, &message)
            }
        }
        PageAuth::Allow { set_cookie } => {
            let mut response = index_response(&ctx);
            if let Some(secret) = set_cookie {
                let value = format!(
                    "{SESSION_COOKIE}={secret}; Path=/; HttpOnly; SameSite=Lax; \
                     Max-Age={SESSION_MAX_AGE_SECS}"
                );
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
    let claim = DeviceClaim::new(header_str(&headers, "user-agent").unwrap_or(""), peer);
    let cookie = cookie_value(&headers, SESSION_COOKIE);
    let Some(device) = ctx.lan.touch_session(cookie.as_deref(), &claim) else {
        return unauthorized_rpc();
    };
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
    let cookie = cookie_value(&headers, SESSION_COOKIE);
    if ctx.lan.touch_session(cookie.as_deref(), &device).is_none() {
        return plain(
            axum::http::StatusCode::UNAUTHORIZED,
            "нужен код приглашения",
        );
    }
    let Some(path) = asset_path(uri.path()) else {
        return plain(axum::http::StatusCode::BAD_REQUEST, "недопустимый путь");
    };
    match ctx.surface.asset(&path) {
        Some(asset) => asset_response(asset),
        None => plain(axum::http::StatusCode::NOT_FOUND, "не найдено"),
    }
}

fn asset_response(asset: Asset) -> axum::response::Response {
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

fn unauthorized_rpc() -> axum::response::Response {
    json(
        axum::http::StatusCode::UNAUTHORIZED,
        serde_json::json!({
            "ok": false,
            "error": {
                "code": "lan_unauthorized",
                "message": "доступ не подтверждён: откройте страницу по коду приглашения",
            }
        }),
    )
}

/// The LAN page is served by the phone door itself, so it must talk HTTP
/// (`POST /rpc`) exactly like the browser does. Inside the app window the Tauri
/// IPC bridge also exists — the page has to know which door is its own, and
/// guessing by origin is fragile. The server that serves the shell says it
/// outright (REQ-QRAPP-02 AC5: the same interface, the same rights).
fn lan_shell(bytes: Vec<u8>) -> Vec<u8> {
    const MARKER: &[u8] = b"<script>window.__DEVAI_LAN__=true</script>";
    let html = String::from_utf8_lossy(&bytes).to_string();
    let at = html.find("</head>").unwrap_or(0);
    let mut out = Vec::with_capacity(bytes.len() + MARKER.len());
    out.extend_from_slice(html[..at].as_bytes());
    out.extend_from_slice(MARKER);
    out.extend_from_slice(html[at..].as_bytes());
    out
}

fn index_response(ctx: &ServerCtx) -> axum::response::Response {
    match ctx.surface.asset("index.html") {
        Some(asset) => {
            let mut response =
                axum::response::Response::new(axum::body::Body::from(lan_shell(asset.bytes)));
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

/// The refusal a browser sees when it presents a dead code or knocks on a
/// closed door: the SAME three sentences the app shows, in an HTML page, so a
/// phone without the app also reads «приглашение просрочено» instead of a bare
/// HTTP status (REQ-QRAPP-06 AC1-3). A phone that remembered a home and comes
/// back without a working session reads «доступ закрыт» — that is the ratified
/// «отзыв виден и на следующем запуске» (Q4), and the page itself cannot say
/// it because a dead session is refused the shell. Anything else falls back to
/// the message the state machine produced.
fn refusal_page(
    status: axum::http::StatusCode,
    code: &str,
    message: &str,
) -> axum::response::Response {
    let refusal = match code {
        "lan_token_invalid" => Some(PairRefusal::InviteExpired),
        "lan_token_required" => Some(PairRefusal::AccessClosed),
        "lan_off" => Some(PairRefusal::AccessClosed),
        _ => None,
    };
    let body = match refusal {
        Some(refusal) => format!(
            "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
             <h1>{title}</h1><p>{hint}</p>",
            title = refusal.text(),
            hint = refusal.hint(),
        ),
        None => message.to_string(),
    };
    let mut response = axum::response::Response::new(axum::body::Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
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

/// The same JSON answer, readable from another origin (`GET /pair` only). The
/// app asks the candidate machine before it switches homes; without this
/// header the question is a CORS failure and would read as silence — a working
/// session would then never be able to move (quality return R2).
fn json_readable_across_origins(
    status: axum::http::StatusCode,
    body: serde_json::Value,
) -> axum::response::Response {
    let mut response = json(status, body);
    response.headers_mut().insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode};
    use tower::ServiceExt;

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

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn context() -> (Arc<LanAccess>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let lan = LanAccess::new(dir.path().join("lan-access.json"));
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let addr: SocketAddr = "192.168.1.20:4567".parse().unwrap();
        lan.activate(addr, tx);
        (Arc::new(lan), dir)
    }

    fn router(lan: Arc<LanAccess>) -> axum::Router {
        routes(lan, Arc::new(FakeSurface))
    }

    fn call(
        router: &axum::Router,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: &str,
    ) -> (StatusCode, HeaderMap, String) {
        let peer: SocketAddr = "192.168.1.42:50000".parse().unwrap();
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .extension(axum::extract::ConnectInfo(peer))
            .header(
                "user-agent",
                "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36",
            )
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", format!("{SESSION_COOKIE}={cookie}"));
        }
        let request = builder.body(Body::from(body.to_string())).unwrap();
        block_on(async move {
            let response = router.clone().oneshot(request).await.expect("router");
            let status = response.status();
            let headers = response.headers().clone();
            let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            (status, headers, String::from_utf8_lossy(&bytes).to_string())
        })
    }

    fn cookie_of(headers: &HeaderMap) -> String {
        let value = headers
            .get("set-cookie")
            .expect("pairing sets the session cookie")
            .to_str()
            .unwrap();
        let rest = value.trim_start_matches(&format!("{SESSION_COOKIE}="));
        rest.split(';').next().unwrap().to_string()
    }

    /// Pair one device and return the HttpOnly session cookie it received.
    fn pair(router: &axum::Router, invite: &str) -> (HeaderMap, String) {
        let (status, headers, _) = call(router, "GET", &format!("/?token={invite}"), None, "");
        assert_eq!(status, StatusCode::OK);
        let cookie = cookie_of(&headers);
        (headers, cookie)
    }

    /// The shell and the RPC door are both closed before a token is presented
    /// (REQ-LAN-04 AC1: nothing is served without an invitation).
    #[test]
    fn an_unpaired_client_gets_neither_shell_nor_rpc() {
        let (lan, _dir) = context();
        let router = router(lan);
        let (status, _, _) = call(&router, "GET", "/", None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = call(
            &router,
            "POST",
            "/rpc",
            None,
            "{\"command\":\"tasks_list\"}",
        );
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// The invitation becomes an HttpOnly session cookie, carries the RPC door
    /// afterwards, and is dead for a second client (REQ-LAN-04 AC4).
    #[test]
    fn the_invitation_pairs_exactly_one_device_into_a_cookie() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.expect("a fresh invitation");
        let (headers, cookie) = pair(&router, &invite);
        let raw_cookie = headers.get("set-cookie").unwrap().to_str().unwrap();
        assert!(
            raw_cookie.to_ascii_lowercase().contains("httponly"),
            "the session cookie must not be readable from JS"
        );
        assert!(
            raw_cookie.contains("Max-Age="),
            "the phone keeps the remembered home between launches (ratified Q4): {raw_cookie}"
        );
        let (status, _, _) = call(&router, "GET", "/", Some(&cookie), "");
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = call(&router, "GET", &format!("/?token={invite}"), None, "");
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "a used invitation is dead"
        );
        let (status, _, body) = call(
            &router,
            "POST",
            "/rpc",
            Some(&cookie),
            "{\"command\":\"tasks_list\",\"params\":{}}",
        );
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"ok\":true"));
        assert!(body.contains("tasks_list"));
        assert!(
            body.contains("Pixel 7"),
            "the RPC door sees the claimed device name: {body}"
        );
    }

    /// «Отключить» revokes the session: the very next RPC is out even though
    /// the page is still open in the phone's browser (REQ-LAN-04 AC6).
    #[test]
    fn disable_revokes_the_open_session_at_once() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.unwrap();
        let (_, cookie) = pair(&router, &invite);
        lan.deactivate();
        let (status, _, _) = call(
            &router,
            "POST",
            "/rpc",
            Some(&cookie),
            "{\"command\":\"tasks_list\"}",
        );
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = call(&router, "GET", "/", Some(&cookie), "");
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// The shell is part of what a token unlocks: an unpaired client gets no
    /// asset at all, a paired one gets the bundle the window renders.
    #[test]
    fn the_asset_door_needs_the_session_and_serves_the_same_bundle() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let (status, _, _) = call(&router, "GET", "/assets/app.js", None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let invite = lan.status().token.unwrap();
        let (_, headers, _) = call(&router, "GET", &format!("/?token={invite}"), None, "");
        let cookie = cookie_of(&headers);
        let (status, _, body) = call(&router, "GET", "/assets/app.js", Some(&cookie), "");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "assets/app.js");
    }

    /// R7: the client-supplied path is normalized and refused when it tries to
    /// leave the bundle root — it never reaches the asset resolver raw.
    #[test]
    fn the_asset_path_cannot_leave_the_bundle_root() {
        assert_eq!(asset_path("/assets/app.js"), Some("assets/app.js".into()));
        assert_eq!(asset_path("assets//app.js"), Some("assets/app.js".into()));
        assert_eq!(asset_path("/./assets/app.js"), Some("assets/app.js".into()));
        assert_eq!(asset_path("/assets/../secret"), None);
        assert_eq!(asset_path("/../../etc/passwd"), None);
        assert_eq!(asset_path("/assets/..%2f..%2fsecret"), None);
        assert_eq!(asset_path("/assets\\..\\secret"), None);
        assert_eq!(asset_path("/"), None);

        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.unwrap();
        let (_, headers, _) = call(&router, "GET", &format!("/?token={invite}"), None, "");
        let cookie = cookie_of(&headers);
        let (status, _, _) = call(&router, "GET", "/assets/../secret", Some(&cookie), "");
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// REQ-QRAPP-02 AC5: the page served by the phone door declares itself as
    /// the LAN surface, so the same bundle knows to talk `/rpc` inside the app
    /// window too — one interface, one transport, two shells.
    #[test]
    fn the_served_shell_declares_the_lan_surface() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.unwrap();
        let (status, _, body) = call(&router, "GET", &format!("/?token={invite}"), None, "");
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("__DEVAI_LAN__=true"),
            "the shell must say which door serves it: {body}"
        );
        assert!(
            body.matches("__DEVAI_LAN__").count() == 1,
            "the marker is injected once, by the server that serves the shell"
        );
    }

    /// REQ-QRAPP-05/06: one cheap question answers «paired», «this code is
    /// valid but unused», «refused» — and asking it never SPENDS the code, so
    /// the app can validate a scanned link before it opens the page.
    #[test]
    fn the_pair_question_distinguishes_a_live_session_from_a_dead_code() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.expect("a fresh invitation");

        // No session and no code: the door is shut, and says which way.
        let (status, _, body) = call(&router, "GET", "/pair", None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("lan_token_required"), "{body}");

        // A valid invitation is reported as such and stays usable: the app
        // asks first, the page load later is what pairs.
        let (status, _, body) = call(&router, "GET", &format!("/pair?token={invite}"), None, "");
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(STATE_INVITE), "{body}");
        assert_eq!(
            lan.status().token.as_deref(),
            Some(invite.as_str()),
            "the question must not consume the code"
        );

        // Pair, then the same question answers «paired» with the cookie.
        let (_, cookie) = pair(&router, &invite);
        let (status, _, body) = call(&router, "GET", "/pair", Some(&cookie), "");
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(STATE_PAIRED), "{body}");
        assert!(lan.status().devices[0].live, "the question keeps it live");

        // A wrong code is refused with its own machine code.
        let (status, _, body) = call(&router, "GET", "/pair?token=deadbeef", None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("lan_token_invalid"), "{body}");
    }

    /// The two ways a paired session dies are two different answers of the
    /// same question — «токен отозван» and «доступ выключен» (REQ-QRAPP-06).
    #[test]
    fn a_revoked_session_and_a_switched_off_access_are_two_answers() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.expect("a fresh invitation");
        let (_, cookie) = pair(&router, &invite);

        // Revoked device: its cookie is dead, the access still serves.
        lan.disconnect(&lan.status().devices[0].id);
        let (status, _, body) = call(&router, "GET", "/pair", Some(&cookie), "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains(STATE_INVALID), "{body}");

        // Access off: FORBIDDEN, and a phone already in the network reads it
        // as «доступ закрыт» rather than «просрочено».
        lan.deactivate();
        let (status, _, body) = call(&router, "GET", "/pair", Some(&cookie), "");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains(STATE_CLOSED), "{body}");
    }

    /// REQ-QRAPP-06 AC1/AC3: a browser that presents a dead code reads the
    /// same sentence the app would show — «приглашение просрочено» — instead
    /// of a bare status or a generic «ошибка».
    #[test]
    fn a_dead_code_reads_as_an_expired_invitation_in_the_browser_too() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let (status, headers, body) = call(&router, "GET", "/?token=deadbeef", None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            headers.get("content-type").unwrap().to_str().unwrap(),
            "text/html; charset=utf-8"
        );
        assert!(body.contains("приглашение просрочено"), "{body}");

        // The consumed code reads the same way (one-time per pairing).
        let invite = lan.status().token.unwrap();
        let _ = pair(&router, &invite);
        let (status, _, body) = call(&router, "GET", &format!("/?token={invite}"), None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("приглашение просрочено"), "{body}");
    }

    /// R2: the question the app page asks the candidate machine is readable
    /// from ANOTHER origin, otherwise the window could never check an
    /// invitation before leaving the working home. Only `/pair` says so.
    #[test]
    fn only_the_pair_question_is_readable_across_origins() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.unwrap();
        let (status, headers, _) = call(&router, "GET", &format!("/pair?token={invite}"), None, "");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get("access-control-allow-origin").unwrap(),
            "*",
            "the app's own page must be able to read the answer"
        );
        // A refused answer is readable too — the reason has to be told apart.
        let (status, headers, _) = call(&router, "GET", "/pair?token=deadbeef", None, "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers.get("access-control-allow-origin").unwrap(), "*");
        // The shell and the data door stay closed to other origins.
        for uri in ["/", "/rpc"] {
            let (_, headers, _) = call(&router, "GET", uri, None, "");
            assert!(
                headers.get("access-control-allow-origin").is_none(),
                "{uri} must not invite foreign origins"
            );
        }
    }

    /// Ratified Q4 + R5: a phone that remembered this home and comes back
    /// after the access was turned off reads «доступ закрыт» — the sentence is
    /// in the page the machine serves, because a dead session is refused the
    /// application shell itself (there is no SPA left to poll `/pair`).
    #[test]
    fn a_revoked_remembered_session_reads_as_closed_access() {
        let (lan, _dir) = context();
        let router = router(lan.clone());
        let invite = lan.status().token.unwrap();
        let (_, cookie) = pair(&router, &invite);
        // The device was disconnected: its cookie is worthless, the access on.
        lan.disconnect(&lan.status().devices[0].id);
        let (status, _, body) = call(&router, "GET", "/", Some(&cookie), "");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("доступ закрыт"), "{body}");
        assert!(!body.contains("нужен код приглашения"), "{body}");
    }
}
