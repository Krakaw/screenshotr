mod capture;
mod jpeg;
mod permissions;
mod sys;

use std::sync::Mutex;

use subtle::ConstantTimeEq;
use tiny_http::{Header, Request, Response, Server};

const DEFAULT_BIND: &str = "0.0.0.0:8765";
const DEFAULT_QUALITY: u8 = 85;

/// The browser UI, embedded at compile time so the binary stays self-contained.
const INDEX_HTML: &str = include_str!("index.html");

/// Shared by every endpoint that touches ScreenCaptureKit, so a revoked grant
/// reads the same whether the client was capturing or enumerating.
const PERMISSION_DENIED_JSON: &str = r#"{"error":"screen recording permission not granted","hint":"System Settings > Privacy & Security > Screen Recording"}"#;

/// Captures are serialised: concurrent ScreenCaptureKit calls buy nothing for
/// a request-driven service and keep the FFI boundary single-threaded.
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let bind = std::env::var("SCREENSHOTR_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    let token = match load_token() {
        Ok(t) => t,
        Err(e) => {
            log::error!("{e}");
            log::error!("refusing to start without a token; run `make token`");
            std::process::exit(2);
        }
    };

    // Gate on the ground-truth capability, not on preflight: preflight can
    // report true while ScreenCaptureKit returns no displays (a grant gone
    // stale after the bundle was replaced). A listener that can only ever
    // return 503 is worse than not being up.
    if !capture::can_capture() {
        permissions::ensure_access_or_exit();
    }
    // Reset the one-shot prompt marker so that if the grant is ever revoked,
    // the next startup opens System Settings again.
    permissions::clear_prompt_marker();

    let server = match Server::http(&bind) {
        Ok(s) => s,
        Err(e) => {
            log::error!("failed to bind {bind}: {e}");
            std::process::exit(1);
        }
    };

    log::info!("screenshotr listening on {bind}");
    if bind.starts_with("0.0.0.0") || bind.starts_with("[::]") {
        log::warn!(
            "bound to all interfaces: this screen-capture endpoint is reachable \
             from your LAN. Access is bearer-token gated."
        );
    }

    for request in server.incoming_requests() {
        handle(request, &token);
    }
}

fn load_token() -> Result<String, String> {
    let path = std::env::var("SCREENSHOTR_TOKEN_FILE").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.config/screenshotr/token")
    });

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read token file {path}: {e}"))?;
    let token = raw.trim().to_string();
    if token.is_empty() {
        return Err(format!("token file {path} is empty"));
    }
    Ok(token)
}

fn handle(request: Request, token: &str) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("");

    match path {
        "/" => respond_html(request, INDEX_HTML),
        // Browsers auto-request this; answer quietly to keep logs clean.
        "/favicon.ico" => {
            let _ = request.respond(Response::empty(204));
        }
        "/healthz" => respond_json(
            request,
            200,
            &format!(
                r#"{{"status":"ok","version":"{}","screen_recording":{},"active_display":{}}}"#,
                env!("CARGO_PKG_VERSION"),
                permissions::has_access(),
                sys::display_under_cursor()
            ),
        ),
        "/displays" => {
            if !authorized(&request, token) {
                log::warn!("unauthorized request from {:?}", request.remote_addr());
                respond_json(request, 401, r#"{"error":"unauthorized"}"#);
                return;
            }
            displays(request);
        }
        "/screenshot" => {
            if !authorized(&request, token) {
                log::warn!("unauthorized request from {:?}", request.remote_addr());
                respond_json(request, 401, r#"{"error":"unauthorized"}"#);
                return;
            }
            screenshot(request, &url);
        }
        _ => respond_json(request, 404, r#"{"error":"not found"}"#),
    }
}

fn authorized(request: &Request, token: &str) -> bool {
    let Some(header) = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
    else {
        return false;
    };

    let Some(presented) = header.value.as_str().strip_prefix("Bearer ") else {
        return false;
    };

    // Constant-time compare. ct_eq is only defined for equal-length slices, so
    // a length mismatch short-circuits; that leaks the token length, which is
    // unavoidable and not sensitive.
    presented.as_bytes().ct_eq(token.as_bytes()).into()
}

/// List the attached displays so a client can offer a picker.
fn displays(request: Request) {
    let listed = {
        let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        capture::list_displays()
    };

    let displays = match listed {
        Ok(d) => d,
        Err(capture::CaptureError::PermissionDenied) => {
            respond_json(request, 503, PERMISSION_DENIED_JSON);
            return;
        }
        Err(e) => {
            log::error!("listing displays failed: {e}");
            respond_json(request, 500, r#"{"error":"could not list displays"}"#);
            return;
        }
    };

    let items = displays
        .iter()
        .map(|d| {
            format!(
                r#"{{"id":{},"width":{},"height":{},"x":{},"y":{},"active":{}}}"#,
                d.id, d.width, d.height, d.x, d.y, d.active
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    respond_json(request, 200, &format!(r#"{{"displays":[{items}]}}"#));
}

fn screenshot(request: Request, url: &str) {
    let quality = parse_quality(url);
    let target = match parse_target(url) {
        Ok(t) => t,
        Err(raw) => {
            log::warn!("rejected display selector {raw:?}");
            respond_json(
                request,
                400,
                r#"{"error":"invalid display selector","hint":"use display=all, display=active, or a numeric display id from /displays"}"#,
            );
            return;
        }
    };

    let result = {
        let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        capture::capture(target)
    };

    let frame = match result {
        Ok(f) => f,
        Err(capture::CaptureError::PermissionDenied) => {
            log::error!("capture failed: screen recording permission revoked");
            respond_json(request, 503, PERMISSION_DENIED_JSON);
            return;
        }
        Err(e @ capture::CaptureError::NoSuchDisplay(_)) => {
            log::warn!("capture failed: {e}");
            respond_json(
                request,
                404,
                r#"{"error":"no such display","hint":"call /displays for the current list"}"#,
            );
            return;
        }
        // Deterministic limits on what the request asked for, not transient
        // failures: a caller that retries will get the same answer, so say so
        // distinctly rather than hiding them in a generic 500.
        Err(e @ (capture::CaptureError::FrameTooLarge { .. }
        | capture::CaptureError::OutOfMemory { .. })) => {
            log::error!("capture failed: {e}");
            respond_json(
                request,
                413,
                r#"{"error":"capture too large","hint":"capture a single display instead of display=all"}"#,
            );
            return;
        }
        Err(e) => {
            log::error!("capture failed: {e}");
            respond_json(request, 500, r#"{"error":"capture failed"}"#);
            return;
        }
    };

    match jpeg::encode(&frame, quality) {
        Ok(bytes) => {
            log::info!(
                "captured {}x{} -> {} KB (q={})",
                frame.width,
                frame.height,
                bytes.len() / 1024,
                quality
            );
            let header = Header::from_bytes(&b"Content-Type"[..], &b"image/jpeg"[..])
                .expect("static header is valid");
            let _ = request.respond(Response::from_data(bytes).with_header(header));
        }
        Err(e) => {
            log::error!("jpeg encode failed: {e}");
            respond_json(request, 500, r#"{"error":"encode failed"}"#);
        }
    }
}

/// First value for `key` in the URL's query string, if present.
fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    url.split_once('?')
        .map(|(_, qs)| qs)
        .into_iter()
        .flat_map(|qs| qs.split('&'))
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then_some(v)
        })
}

fn parse_quality(url: &str) -> u8 {
    query_param(url, "quality")
        .and_then(|v| v.parse::<u8>().ok())
        .map_or(DEFAULT_QUALITY, |q| q.clamp(1, 100))
}

/// Resolve `?display=` into a capture target.
///
/// Absent means the active display, so callers written before multi-display
/// support keep the behaviour they had. An unparseable value is an error
/// rather than a silent fallback: quietly capturing the wrong screen and
/// returning 200 is worse than a 400.
fn parse_target(url: &str) -> Result<capture::Target, &str> {
    match query_param(url, "display") {
        None | Some("") | Some("active") => Ok(capture::Target::Active),
        Some("all") => Ok(capture::Target::All),
        Some(raw) => raw.parse().map(capture::Target::Id).map_err(|_| raw),
    }
}

fn respond_html(request: Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("static header is valid");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn respond_json(request: Request, status: u16, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static header is valid");
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}
