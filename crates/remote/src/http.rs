//! A deliberately small HTTP/1.1 server for the browser remote.
//!
//! One request per connection, no chunked bodies and no keep-alive. The parser
//! bounds headers and bodies, the command endpoint feeds the same bounded
//! dispatch channel as OSC, and SSE only pushes snapshots the engine already
//! builds for `Status`.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use engine::{Engine, OutputStatus};

use crate::token::constant_time_eq;
use crate::{CommandReply, DispatchRequest};

const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 4 * 1024;
const PAGE: &str = include_str!("page.html");

#[derive(Debug)]
enum Failure {
    BadRequest,
    TooLarge,
    Io,
}

struct Head {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

pub(crate) fn serve(
    listener: TcpListener,
    engine: Arc<Mutex<Engine>>,
    sender: SyncSender<DispatchRequest>,
    token: Arc<String>,
    status: Arc<Mutex<OutputStatus>>,
    stop: Arc<AtomicBool>,
) {
    let limiter = Arc::new(RateLimiter::default());
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                bump(&status, |remote| remote.http_clients += 1);
                let engine = Arc::clone(&engine);
                let sender = sender.clone();
                let token = Arc::clone(&token);
                let status = Arc::clone(&status);
                let stop = Arc::clone(&stop);
                let limiter = Arc::clone(&limiter);
                thread::spawn(move || {
                    handle_connection(
                        stream, peer, &engine, &sender, &token, &status, &stop, &limiter,
                    );
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    engine: &Arc<Mutex<Engine>>,
    sender: &SyncSender<DispatchRequest>,
    token: &str,
    status: &Arc<Mutex<OutputStatus>>,
    stop: &Arc<AtomicBool>,
    limiter: &RateLimiter,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(Failure::TooLarge) => {
            let _ = respond(
                &mut stream,
                413,
                "Content Too Large",
                "text/plain",
                "too large",
            );
            return;
        }
        Err(Failure::BadRequest) => {
            let _ = respond(&mut stream, 400, "Bad Request", "text/plain", "bad request");
            return;
        }
        Err(Failure::Io) => return,
    };
    let (path, query) = split_target(&request.target);
    let allow_query_token = request.method == "GET" && path == "/events";
    if !(request.method == "GET" && path == "/")
        && !authorized(&request.headers, query, token, allow_query_token)
    {
        bump(status, |remote| remote.http_rejected += 1);
        let _ = respond(
            &mut stream,
            401,
            "Unauthorized",
            "application/json",
            "{\"error\":\"missing or invalid token\"}",
        );
        return;
    }

    match (request.method.as_str(), path) {
        ("GET", "/") => {
            let _ = respond(&mut stream, 200, "OK", "text/html; charset=utf-8", PAGE);
        }
        ("GET", "/state") => {
            let body = engine::output::lock(engine).remote_state_json();
            let _ = respond(&mut stream, 200, "OK", "application/json", &body);
        }
        ("GET", "/events") => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                  Cache-Control: no-cache\r\nConnection: close\r\n\r\n",
            );
            stream_sse(stream, engine, stop);
        }
        ("POST", "/command") => {
            if request
                .headers
                .get("content-type")
                .map(|value| !value.to_ascii_lowercase().starts_with("application/json"))
                .unwrap_or(true)
            {
                bump(status, |remote| remote.http_rejected += 1);
                let _ = respond(
                    &mut stream,
                    415,
                    "Unsupported Media Type",
                    "application/json",
                    "{\"error\":\"Content-Type must be application/json\"}",
                );
                return;
            }
            if !origin_allowed(&request.headers) {
                bump(status, |remote| remote.http_rejected += 1);
                let _ = respond(
                    &mut stream,
                    403,
                    "Forbidden",
                    "application/json",
                    "{\"error\":\"foreign Origin\"}",
                );
                return;
            }
            if !limiter.allow(peer.ip()) {
                bump(status, |remote| {
                    remote.http_rate_limited += 1;
                    remote.http_rejected += 1;
                });
                let _ = respond(
                    &mut stream,
                    429,
                    "Too Many Requests",
                    "application/json",
                    "{\"error\":\"rate limit\"}",
                );
                return;
            }
            let Some(line) = parse_command_line(&request.body) else {
                let _ = respond(
                    &mut stream,
                    400,
                    "Bad Request",
                    "application/json",
                    "{\"error\":\"body must be {\\\"line\\\":\\\"…\\\"}\"}",
                );
                return;
            };
            let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
            let dispatched = sender.try_send(DispatchRequest {
                command: crate::mapping::RemoteCommand::Line(line),
                reply: Some(reply_sender),
            });
            if dispatched.is_err() {
                bump(status, |remote| remote.dropped += 1);
                let _ = respond(
                    &mut stream,
                    503,
                    "Service Unavailable",
                    "application/json",
                    "{\"error\":\"remote queue is full\"}",
                );
                return;
            }
            match reply_receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(CommandReply { lines, error }) => {
                    let body = serde_json::json!({"lines": lines, "error": error}).to_string();
                    let _ = respond(&mut stream, 200, "OK", "application/json", &body);
                }
                Err(_) => {
                    bump(status, |remote| remote.dropped += 1);
                    let _ = respond(
                        &mut stream,
                        503,
                        "Service Unavailable",
                        "application/json",
                        "{\"error\":\"command timed out\"}",
                    );
                }
            }
        }
        _ => {
            let _ = respond(&mut stream, 404, "Not Found", "text/plain", "not found");
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Head, Failure> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(position) = find_bytes(&buffer, b"\r\n\r\n") {
            break position;
        }
        if buffer.len() > MAX_HEADER {
            return Err(Failure::TooLarge);
        }
        let count = stream.read(&mut chunk).map_err(|_| Failure::Io)?;
        if count == 0 {
            return Err(Failure::BadRequest);
        }
        buffer.extend_from_slice(&chunk[..count]);
    };
    if header_end > MAX_HEADER {
        return Err(Failure::TooLarge);
    }
    let header = std::str::from_utf8(&buffer[..header_end]).map_err(|_| Failure::BadRequest)?;
    let mut lines = header.split("\r\n");
    let request_line = lines.next().ok_or(Failure::BadRequest)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(Failure::BadRequest)?.to_string();
    let target = parts.next().ok_or(Failure::BadRequest)?.to_string();
    let version = parts.next().ok_or(Failure::BadRequest)?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(Failure::BadRequest);
    }
    if !matches!(method.as_str(), "GET" | "POST") || !target.starts_with('/') {
        return Err(Failure::BadRequest);
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(Failure::BadRequest);
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(Failure::BadRequest);
        }
        headers.insert(name, value.trim().to_string());
    }
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(Failure::BadRequest);
    }
    let content_length = match headers.get("content-length") {
        Some(value) => value.parse::<usize>().map_err(|_| Failure::BadRequest)?,
        None => 0,
    };
    if content_length > MAX_BODY {
        return Err(Failure::TooLarge);
    }
    let body_start = header_end + 4;
    let mut body = buffer.get(body_start..).unwrap_or_default().to_vec();
    if body.len() > content_length {
        return Err(Failure::BadRequest);
    }
    while body.len() < content_length {
        let count = stream.read(&mut chunk).map_err(|_| Failure::Io)?;
        if count == 0 {
            return Err(Failure::BadRequest);
        }
        body.extend_from_slice(&chunk[..count]);
        if body.len() > MAX_BODY {
            return Err(Failure::TooLarge);
        }
    }
    if method == "POST" && content_length == 0 {
        return Err(Failure::BadRequest);
    }
    Ok(Head {
        method,
        target,
        headers,
        body,
    })
}

fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    }
}

fn authorized(
    headers: &BTreeMap<String, String>,
    query: Option<&str>,
    token: &str,
    allow_query: bool,
) -> bool {
    if let Some(value) = headers.get("authorization") {
        if let Some(value) = value.strip_prefix("Bearer ") {
            return constant_time_eq(value.trim(), token);
        }
    }
    if allow_query {
        if let Some(query) = query {
            for pair in query.split('&') {
                if let Some((name, value)) = pair.split_once('=') {
                    if name == "token" {
                        return constant_time_eq(value, token);
                    }
                }
            }
        }
    }
    false
}

fn origin_allowed(headers: &BTreeMap<String, String>) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    let Some(host) = headers.get("host") else {
        return false;
    };
    origin == &format!("http://{host}") || origin == &format!("https://{host}")
}

fn parse_command_line(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.get("line")?.as_str().map(str::to_string)
}

fn stream_sse(mut stream: TcpStream, engine: &Arc<Mutex<Engine>>, stop: &Arc<AtomicBool>) {
    let mut last_revision = None;
    let mut heartbeat = Instant::now();
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1)));
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut probe = [0u8; 1];
        match stream.peek(&mut probe) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
        let (revision, state) = {
            let engine = engine::output::lock(engine);
            (engine.revision, engine.remote_state_json())
        };
        if last_revision != Some(revision) {
            if stream
                .write_all(format!("data: {state}\n\n").as_bytes())
                .is_err()
            {
                return;
            }
            last_revision = Some(revision);
        }
        if heartbeat.elapsed() >= Duration::from_secs(15) {
            if stream.write_all(b": heartbeat\n\n").is_err() {
                return;
            }
            heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn bump(status: &Arc<Mutex<OutputStatus>>, change: impl FnOnce(&mut engine::RemoteStatus)) {
    let mut status = engine::output::lock(status);
    if let Some(remote) = status.remote.as_mut() {
        change(remote);
    }
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(body.as_bytes())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// One token bucket per peer address. A browser refresh or script loop cannot
/// spend the engine's tick; normal remote use is far under 50 commands/s.
struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }
}

impl RateLimiter {
    const CAPACITY: f64 = 100.0;
    const RATE: f64 = 50.0;

    fn allow(&self, address: IpAddr) -> bool {
        let now = Instant::now();
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bucket = buckets.entry(address).or_insert(Bucket {
            tokens: Self::CAPACITY,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * Self::RATE).min(Self::CAPACITY);
        bucket.last = now;
        if bucket.tokens < 1.0 {
            false
        } else {
            bucket.tokens -= 1.0;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    fn directory(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "openlight-http-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn start(
        name: &str,
    ) -> (
        crate::Remote,
        Arc<Mutex<Engine>>,
        String,
        std::path::PathBuf,
    ) {
        let directory = directory(name);
        let engine = Arc::new(Mutex::new(Engine::empty()));
        let config = crate::RemoteConfig {
            osc_port: 0,
            http_port: 0,
            bind: IpAddr::from([127, 0, 0, 1]),
            token_path: directory.join("remote.token"),
        };
        let remote = crate::Remote::start(Arc::clone(&engine), config).unwrap();
        let token = std::fs::read_to_string(remote.token_path())
            .unwrap()
            .trim()
            .to_string();
        (remote, engine, token, directory)
    }

    fn request(remote: &crate::Remote, request: &str) -> String {
        let mut stream = TcpStream::connect(remote.http_addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn status(response: &str) -> &str {
        response.lines().next().unwrap_or_default()
    }

    #[test]
    fn the_page_is_public_but_state_needs_a_token() {
        let (remote, _engine, token, directory) = start("auth");
        let page = request(
            &remote,
            "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        let missing = request(
            &remote,
            "GET /state HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(status(&missing).contains("401"), "{missing}");
        let wrong = request(
            &remote,
            "GET /state HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer wrong\r\nConnection: close\r\n\r\n",
        );
        assert!(status(&wrong).contains("401"), "{wrong}");
        let good = request(
            &remote,
            &format!(
                "GET /state HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(status(&good).contains("200"), "{good}");
        assert!(good.contains("\"executors\""), "{good}");
        drop(remote);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_command_returns_the_engine_reply_and_a_foreign_origin_is_refused() {
        let (remote, _engine, token, directory) = start("command");
        let body = r#"{"line":"Master Grand At 75"}"#;
        let good = request(
            &remote,
            &format!(
                "POST /command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\nOrigin: http://localhost\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(status(&good).contains("200"), "{good}");
        assert!(good.contains("Grand"), "{good}");
        let foreign = request(
            &remote,
            &format!(
                "POST /command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\nOrigin: http://evil.example\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(status(&foreign).contains("403"), "{foreign}");
        drop(remote);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn commands_need_json_content_and_a_line() {
        let (remote, _engine, token, directory) = start("body");
        let body = "{\"line\":\"Status\"}";
        let no_type = request(
            &remote,
            &format!(
                "POST /command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(status(&no_type).contains("415"), "{no_type}");
        let no_line = "{}";
        let bad_body = request(
            &remote,
            &format!(
                "POST /command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{no_line}",
                no_line.len()
            ),
        );
        assert!(status(&bad_body).contains("400"), "{bad_body}");
        drop(remote);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_oversized_body_is_413() {
        let (remote, _engine, token, directory) = start("large");
        let response = request(
            &remote,
            &format!(
                "POST /command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(status(&response).contains("413"), "{response}");
        drop(remote);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_locked_desk_refuses_the_remote_command_like_the_keyboard() {
        let (remote, engine, token, directory) = start("locked");
        engine::output::lock(&engine).execute("Lock 123").unwrap();
        let body = r#"{"line":"Master Grand At 10"}"#;
        let response = request(
            &remote,
            &format!(
                "POST /command HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(status(&response).contains("200"), "{response}");
        assert!(response.contains("locked"), "{response}");
        drop(remote);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn sse_sends_a_new_state_when_the_engine_revision_changes() {
        let (remote, engine, token, directory) = start("sse");
        let mut stream = TcpStream::connect(remote.http_addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "GET /events?token={token} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let first = read_until(&mut stream, "\n\n");
        assert!(first.contains("data: "), "{first}");
        assert!(first.contains("\"grand\":1.0"), "{first}");
        engine::output::lock(&engine)
            .execute("Master Grand At 40")
            .unwrap();
        let next = read_until(&mut stream, "\"grand\":0.4");
        assert!(next.contains("data: "), "{next}");
        drop(stream);
        drop(remote);
        let _ = std::fs::remove_dir_all(directory);
    }

    fn read_until(stream: &mut TcpStream, needle: &str) -> String {
        let mut response = String::new();
        let mut chunk = [0u8; 1024];
        loop {
            if response.contains(needle) {
                return response;
            }
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0, "SSE closed before `{needle}`");
            response.push_str(&String::from_utf8_lossy(&chunk[..count]));
        }
    }

    #[test]
    fn a_token_bucket_rejects_a_burst_past_its_capacity() {
        let limiter = RateLimiter::default();
        let ip = IpAddr::from([127, 0, 0, 1]);
        for _ in 0..100 {
            assert!(limiter.allow(ip));
        }
        assert!(!limiter.allow(ip));
    }
}
