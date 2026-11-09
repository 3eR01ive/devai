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
use crate::lan::state::{DeviceClaim, LanAccess, PageAuth};

const SESSION_COOKIE: &str = "devai_lan";

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
                plain(axum::http::StatusCode::FORBIDDEN, &message)
            } else {
                plain(axum::http::StatusCode::UNAUTHORIZED, &message)
            }
        }
        PageAuth::Allow { set_cookie } => {
            let mut response = index_response(&ctx);
            if let Some(secret) = set_cookie {
                let value = format!("{SESSION_COOKIE}={secret}; Path=/; HttpOnly; SameSite=Lax");
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
}
