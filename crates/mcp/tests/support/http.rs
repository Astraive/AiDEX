//! A loopback HTTP/1.1 server (FakeServer style) that records every request and
//! answers through a handler, so a test can script a Streamable HTTP MCP server.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The JSON-RPC body (`Null` when there is none).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn rpc_method(&self) -> Option<String> {
        self.json()
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
}

#[derive(Clone, Debug)]
pub enum Reply {
    /// Status, extra headers, and a JSON body.
    Json(u16, Vec<(String, String)>, String),
    /// A `text/event-stream` body made of these `data:` events.
    Sse(Vec<(String, String)>, Vec<String>),
    /// Status with an empty body.
    Empty(u16),
}

type Handler = dyn Fn(&RecordedRequest) -> Reply + Send + Sync;

pub struct FakeServer {
    pub addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start(handler: impl Fn(&RecordedRequest) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let handle = {
            let (connections, requests, stop) = (
                Arc::clone(&connections),
                Arc::clone(&requests),
                Arc::clone(&stop),
            );
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            connections.fetch_add(1, Ordering::SeqCst);
                            stream.set_nonblocking(false).unwrap();
                            let requests = Arc::clone(&requests);
                            let handler = Arc::clone(&handler);
                            thread::spawn(move || serve(stream, handler.as_ref(), &requests));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        Self {
            addr,
            connections,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:<port>/mcp`.
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// Wait until a request matching `predicate` arrived (at most ten seconds).
    pub fn wait_for(&self, predicate: impl Fn(&RecordedRequest) -> bool) -> RecordedRequest {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = self.requests().into_iter().find(|r| predicate(r)) {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "the expected request never arrived"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(stream: TcpStream, handler: &Handler, requests: &Mutex<Vec<RecordedRequest>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut head = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push(line.trim_end().to_owned());
    }
    let request_line = head.first().cloned().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = head
        .iter()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request = RecordedRequest {
        method,
        path,
        headers,
        body,
    };
    requests.lock().unwrap().push(request.clone());
    let mut stream = stream;
    let head = |status: u16,
                content_type: Option<&str>,
                extra: &[(String, String)],
                length: Option<usize>| {
        let mut text = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
        if let Some(content_type) = content_type {
            text.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        if let Some(length) = length {
            text.push_str(&format!("Content-Length: {length}\r\n"));
        }
        for (key, value) in extra {
            text.push_str(&format!("{key}: {value}\r\n"));
        }
        text.push_str("\r\n");
        text
    };
    let written = match handler(&request) {
        Reply::Json(status, extra, body) => stream.write_all(
            format!(
                "{}{body}",
                head(status, Some("application/json"), &extra, Some(body.len()))
            )
            .as_bytes(),
        ),
        Reply::Sse(extra, events) => {
            let mut text = head(200, Some("text/event-stream"), &extra, None);
            text.push_str(": keep-alive comment\n\n");
            for event in events {
                text.push_str(&format!("event: message\ndata: {event}\n\n"));
            }
            stream.write_all(text.as_bytes())
        }
        Reply::Empty(status) => stream.write_all(head(status, None, &[], Some(0)).as_bytes()),
    };
    let _ = written.and_then(|()| stream.flush());
}
