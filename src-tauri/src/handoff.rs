//! Import from a phone, for a TV where typing a link with the remote is no
//! fun. While the import dialog is open, a one-off page on the local network
//! takes a link, a subscription URL or a config pasted on the phone; the TV
//! shows its address as a QR code.
//!
//! The page lives at a path with a 128-bit random token, answers nothing
//! else (404), and stops once something was imported, when the dialog
//! closes, or after five minutes. It is plain HTTP inside the local network:
//! the token keeps others on it from sending or reading anything, and the
//! page exists only while the user is looking at the QR code.

// Started from the TV app only; built everywhere, so its tests run on the desktop.
#![cfg_attr(not(mobile), allow(dead_code))]

use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tiny_http::{Header, Method, Request, Response, Server};

/// How long the page stays up without anything sent to it.
const LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Links and configs are small; a subscription's contents are not sent here.
const MAX_BODY: usize = 64 * 1024;

/// The running page; dropping it stops it.
pub struct Handoff {
    stop: Arc<AtomicBool>,
    pub url: String,
}

impl Drop for Handoff {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Starts the page on `ip` (the TV's address in the local network). `import`
/// takes the text sent and returns how many servers it added, or why it
/// could not; after an import the page stops.
pub fn start<F>(ip: IpAddr, import: F) -> Result<Handoff, String>
where
    F: Fn(String) -> Result<usize, String> + Send + 'static,
{
    let server = Server::http(SocketAddr::new(ip, 0)).map_err(|err| err.to_string())?;
    let port = server.server_addr().to_ip().map(|addr| addr.port()).ok_or("the page has no port")?;
    // 32 hex digits: 128 random bits.
    let token = uniray_core::engine::secret()[..32].to_owned();
    let url = format!("http://{}/{token}", SocketAddr::new(ip, port));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::Builder::new()
        .name("handoff".into())
        .spawn(move || serve(&server, &token, &flag, &import))
        .map_err(|err| err.to_string())?;
    Ok(Handoff { stop, url })
}

fn serve(server: &Server, token: &str, stop: &AtomicBool, import: &dyn Fn(String) -> Result<usize, String>) {
    let deadline = Instant::now() + LIFETIME;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(300)) else {
            continue;
        };
        if handle(request, token, import) {
            break;
        }
    }
}

/// Answers one request; true when something was imported (the page stops).
fn handle(mut request: Request, token: &str, import: &dyn Fn(String) -> Result<usize, String>) -> bool {
    let path = request.url().split('?').next().unwrap_or("").trim_start_matches('/');
    if !constant_time_eq(path.as_bytes(), token.as_bytes()) {
        let _ = request.respond(Response::from_string("Not found").with_status_code(404));
        return false;
    }
    let ru = russian(&request);
    match request.method() {
        Method::Get => {
            let _ = request.respond(html(&form(ru, None)));
            false
        }
        Method::Post => {
            let mut body = Vec::new();
            let read = request.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body);
            if read.is_err() || body.len() > MAX_BODY {
                let _ = request.respond(html(&form(ru, Some(if ru { "Слишком много текста" } else { "Too much text" }))));
                return false;
            }
            let text = form_field(&String::from_utf8_lossy(&body), "text").unwrap_or_default();
            if text.trim().is_empty() {
                let _ = request.respond(html(&form(ru, Some(if ru { "Вставьте ссылку" } else { "Paste a link" }))));
                return false;
            }
            match import(text) {
                Ok(added) => {
                    let _ = request.respond(html(&done(ru, added)));
                    true
                }
                Err(reason) => {
                    let _ = request.respond(html(&form(ru, Some(&reason))));
                    false
                }
            }
        }
        _ => {
            let _ = request.respond(Response::from_string("").with_status_code(405));
            false
        }
    }
}

fn russian(request: &Request) -> bool {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Accept-Language"))
        .is_some_and(|header| header.value.as_str().trim_start().to_ascii_lowercase().starts_with("ru"))
}

fn html(body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let page = format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>UniRay</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>"
    );
    Response::from_string(page)
        .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("a valid header"))
        .with_header(Header::from_bytes("Cache-Control", "no-store").expect("a valid header"))
        .with_header(Header::from_bytes("Referrer-Policy", "no-referrer").expect("a valid header"))
}

const STYLE: &str = "body{margin:0;font:16px system-ui,sans-serif;background:#f3f4f6;color:#111}\
main{max-width:32rem;margin:0 auto;padding:1.5rem}h1{font-size:1.25rem}\
textarea{box-sizing:border-box;width:100%;height:10rem;padding:.75rem;font:13px ui-monospace,monospace;border:1px solid #ccc;border-radius:.75rem}\
button{margin-top:.75rem;width:100%;padding:.85rem;font-size:1rem;color:#fff;background:#0f8f6f;border:0;border-radius:.75rem}\
.err{color:#b42318}.muted{color:#666;font-size:.9rem}";

fn form(ru: bool, error: Option<&str>) -> String {
    let (title, hint, button) = if ru {
        ("Отправить на ТВ", "Вставьте ссылку на сервер, URL подписки или конфиг WireGuard / AmneziaWG.", "Отправить")
    } else {
        ("Send to the TV", "Paste a server link, a subscription URL or a WireGuard / AmneziaWG config.", "Send")
    };
    let error = error.map(|text| format!("<p class=err>{}</p>", escape(text))).unwrap_or_default();
    format!(
        "<h1>UniRay · {title}</h1><p class=muted>{hint}</p>{error}<form method=post>\
         <textarea name=text autofocus autocapitalize=off autocomplete=off spellcheck=false></textarea>\
         <button type=submit>{button}</button></form>"
    )
}

fn done(ru: bool, added: usize) -> String {
    if ru {
        format!("<h1>Готово</h1><p>На ТВ добавлено серверов: {added}. Эту страницу можно закрыть.</p>")
    } else {
        format!("<h1>Done</h1><p>Servers added on the TV: {added}. You can close this page.</p>")
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The value of `name` in an `application/x-www-form-urlencoded` body.
fn form_field(body: &str, name: &str) -> Option<String> {
    body.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(key) == name).then(|| percent_decode(value))
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => match (hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
                (Some(high), Some(low)) => {
                    out.push(high << 4 | low);
                    i += 2;
                }
                // Not an escape: kept as it is.
                _ => out.push(b'%'),
            },
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: Option<&u8>) -> Option<u8> {
    match byte? {
        digit @ b'0'..=b'9' => Some(digit - b'0'),
        letter @ b'a'..=b'f' => Some(letter - b'a' + 10),
        letter @ b'A'..=b'F' => Some(letter - b'A' + 10),
        _ => None,
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::sync::mpsc;

    fn send(url: &str, request: &str) -> String {
        let addr = url.trim_start_matches("http://").split('/').next().unwrap();
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn path(url: &str) -> &str {
        &url[url.trim_start_matches("http://").find('/').unwrap() + "http://".len()..]
    }

    #[test]
    fn the_page_takes_a_link_once_and_only_at_its_token() {
        let (tx, rx) = mpsc::channel();
        let handoff = start("127.0.0.1".parse().unwrap(), move |text| {
            tx.send(text).unwrap();
            Ok(2)
        })
        .unwrap();
        let url = handoff.url.clone();
        let token_path = path(&url).to_owned();
        assert_eq!(token_path.len(), 33, "{url}");

        let wrong = send(&url, "GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(wrong.starts_with("HTTP/1.1 404"), "{wrong}");
        let page = send(&url, &format!("GET {token_path} HTTP/1.1\r\nHost: x\r\nAccept-Language: ru-RU\r\nConnection: close\r\n\r\n"));
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("Отправить на ТВ"), "{page}");

        let body = "text=vless%3A%2F%2Fid%40host%3A443%23My+server%0Avmess%3A%2F%2Fx";
        let posted = send(
            &url,
            &format!(
                "POST {token_path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(posted.contains("Servers added on the TV: 2"), "{posted}");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "vless://id@host:443#My server\nvmess://x");

        // Stopped after the import: nothing listens there any more.
        std::thread::sleep(Duration::from_millis(500));
        let addr = url.trim_start_matches("http://").split('/').next().unwrap();
        assert!(TcpStream::connect(addr).is_err());
    }

    #[test]
    fn a_failed_import_keeps_the_page_and_says_why() {
        let handoff = start("127.0.0.1".parse().unwrap(), |_| Err("no servers <found>".into())).unwrap();
        let url = handoff.url.clone();
        let body = "text=nonsense";
        let posted = send(
            &url,
            &format!("POST {} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", path(&url), body.len()),
        );
        assert!(posted.contains("no servers &lt;found&gt;"), "{posted}");
        let again = send(&url, &format!("GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", path(&url)));
        assert!(again.starts_with("HTTP/1.1 200"));
        drop(handoff);
    }

    #[test]
    fn form_fields_decode() {
        assert_eq!(form_field("a=1&text=a%2Bb+c%25", "text").as_deref(), Some("a+b c%"));
        assert_eq!(form_field("text=%zz%4", "text").as_deref(), Some("%zz%4"));
        assert_eq!(form_field("other=1", "text"), None);
        assert_eq!(percent_decode("%D0%BF%D1%80%D0%B8"), "при");
    }
}
