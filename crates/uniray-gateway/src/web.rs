//! The web UI and its JSON API.
//!
//! ```text
//! GET  /              the page
//! GET  /api/state     servers, selection, status, traffic
//! POST /api/select    {"id": "..."}
//! POST /api/update    refetch the subscriptions
//! POST /api/ping      measure the servers
//! POST /api/restart   restart the core
//! GET  /api/logs      the last log lines
//! ```

use std::io::Read;
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::app::App;

const PAGE: &str = include_str!("ui.html");
const MAX_BODY: u64 = 4096;

pub fn serve(app: &'static App) -> Result<(), String> {
    let server = Server::http(&app.options.web_listen).map_err(|err| format!("cannot listen on {}: {err}", app.options.web_listen))?;
    app.log(format!("uniray: web UI on http://{}", app.options.web_listen));
    for request in server.incoming_requests() {
        // Requests are short; the slow ones (update, ping) run in the background.
        thread::spawn(move || handle(app, request));
    }
    Ok(())
}

fn handle(app: &'static App, mut request: Request) {
    if !authorized(app, &request) {
        let header = Header::from_bytes("WWW-Authenticate", "Basic realm=\"UniRay\"").expect("static header");
        let _ = request.respond(Response::from_string("authorization required").with_status_code(401).with_header(header));
        return;
    }
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_owned();
    let response = match (method, path.as_str()) {
        (Method::Get, "/") => {
            let page = Response::from_string(PAGE).with_header(header("Content-Type", "text/html; charset=utf-8"));
            let _ = request.respond(page);
            return;
        }
        (Method::Get, "/api/state") => {
            let mut view = app.view();
            if let Some((up, down)) = app.traffic() {
                view["traffic"] = json!({ "up": up, "down": down });
            }
            Ok(view)
        }
        (Method::Post, "/api/select") => match read_json(&mut request) {
            Some(body) => match body["id"].as_str() {
                Some(id) => app.select(id).map(|()| json!({ "ok": true })),
                None => Err("missing id".to_owned()),
            },
            None => Err("bad request".to_owned()),
        },
        (Method::Post, "/api/update") => {
            thread::spawn(move || app.update_subscriptions());
            Ok(json!({ "ok": true }))
        }
        (Method::Post, "/api/ping") => {
            app.ping_all();
            Ok(json!({ "ok": true }))
        }
        (Method::Post, "/api/restart") => {
            thread::spawn(move || app.restart_core());
            Ok(json!({ "ok": true }))
        }
        (Method::Get, "/api/logs") => Ok(json!({ "lines": app.logs.lock().unwrap().iter().collect::<Vec<_>>() })),
        _ => {
            let _ = request.respond(Response::from_string("not found").with_status_code(404));
            return;
        }
    };
    let (status, body) = match response {
        Ok(body) => (200, body),
        Err(message) => (400, json!({ "error": message })),
    };
    let _ = request.respond(
        Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cache-Control", "no-store")),
    );
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("valid header")
}

fn read_json(request: &mut Request) -> Option<Value> {
    let mut body = String::new();
    request.as_reader().take(MAX_BODY).read_to_string(&mut body).ok()?;
    serde_json::from_str(&body).ok()
}

/// HTTP basic auth against `WEB_PASSWORD`; any user name is accepted.
fn authorized(app: &App, request: &Request) -> bool {
    let Some(password) = &app.options.web_password else { return true };
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .and_then(|header| header.value.as_str().strip_prefix("Basic ").map(str::to_owned))
        .and_then(|encoded| decode_base64(encoded.trim()))
        .and_then(|credentials| credentials.split_once(':').map(|(_, given)| given.to_owned()))
        .is_some_and(|given| constant_time_eq(given.as_bytes(), password.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn decode_base64(input: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in input.bytes().filter(|byte| *byte != b'=') {
        let value = ALPHABET.iter().position(|c| *c == byte)? as u32;
        bits = bits << 6 | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_credentials_decode() {
        assert_eq!(decode_base64("YWRtaW46c2VjcmV0").as_deref(), Some("admin:secret"));
        assert_eq!(decode_base64("OnNlY3JldA==").as_deref(), Some(":secret"));
        assert_eq!(decode_base64("@@@"), None);
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
    }
}
