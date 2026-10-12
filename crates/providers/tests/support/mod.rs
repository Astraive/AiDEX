//! An in-test HTTP/1.1 server on 127.0.0.1 that records every connection and
//! request and answers with scripted replies. No external network is used.
#![allow(dead_code)]

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use aidex_drive::DrivePaths;

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Every byte received for this request (head and body).
    pub raw: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }
}

#[derive(Clone, Debug)]
pub enum Reply {
    /// A JSON body with this status.
    Json(u16, String),
    /// `text/event-stream` chunks, each written after its delay.
    Sse(Vec<(Duration, String)>),
    /// Status, extra headers, and body.
    Raw(u16, Vec<(String, String)>, String),
    Binary(Vec<u8>),
    /// The inner reply, sent only once the flag is set (or after ten seconds),
    /// so a test can act while the client waits for the answer.
    Held(Arc<AtomicBool>, Box<Reply>),
}

impl Reply {
    pub fn sse(events: &[&str]) -> Self {
        Self::Sse(
            events
                .iter()
                .map(|event| (Duration::ZERO, format!("data: {event}\n\n")))
                .collect(),
        )
    }
}

pub struct FakeServer {
    pub addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (connections, requests, replies, stop) = (
                Arc::clone(&connections),
                Arc::clone(&requests),
                Arc::clone(&replies),
                Arc::clone(&stop),
            );
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            connections.fetch_add(1, Ordering::SeqCst);
                            stream.set_nonblocking(false).unwrap();
                            let reply = replies.lock().unwrap().pop_front();
                            serve(stream, reply, &requests, &stop);
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
            replies,
            stop,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:<port>/v1`.
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn push(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn wait_for_requests(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.requests().len() < count {
            assert!(
                Instant::now() < deadline,
                "server did not receive {count} requests"
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

fn serve(
    stream: TcpStream,
    reply: Option<Reply>,
    requests: &Mutex<Vec<RecordedRequest>>,
    stop: &AtomicBool,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut raw = Vec::new();
    let mut head = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        raw.extend_from_slice(line.as_bytes());
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
    raw.extend_from_slice(&body);
    requests.lock().unwrap().push(RecordedRequest {
        method,
        path,
        headers,
        body,
        raw,
    });
    let mut stream = stream;
    let mut reply =
        reply.unwrap_or_else(|| Reply::Json(500, r#"{"error":{"code":"no_reply"}}"#.into()));
    while let Reply::Held(release, inner) = reply {
        let until = Instant::now() + Duration::from_secs(10);
        while !release.load(Ordering::SeqCst) && Instant::now() < until {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        reply = *inner;
    }
    let _ = match reply {
        Reply::Binary(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.amazon.eventstream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(head.as_bytes())
                .and_then(|()| stream.write_all(&body))
        }
        Reply::Json(status, body) => write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        Reply::Raw(status, extra, body) => {
            let mut head = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (key, value) in extra {
                head.push_str(&format!("{key}: {value}\r\n"));
            }
            write!(stream, "{head}\r\n{body}")
        }
        Reply::Sse(chunks) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
            );
            for (delay, chunk) in chunks {
                let until = Instant::now() + delay;
                while Instant::now() < until {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                if stream
                    .write_all(chunk.as_bytes())
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
            Ok(())
        }
        Reply::Held(..) => unreachable!("held replies are unwrapped above"),
    };
    let _ = stream.flush();
}

/// A temporary initialized drive and its paths.
pub fn temp_drive() -> (tempfile::TempDir, DrivePaths) {
    let root = tempfile::tempdir().unwrap();
    aidex_drive::init_with(
        root.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .unwrap();
    let paths = DrivePaths::open(root.path()).unwrap();
    (root, paths)
}

/// A secret value generated per run, so no credential lives in a fixture.
pub fn sentinel_secret() -> String {
    format!(
        "sk-test-{}",
        aidex_core::EntityId::generate("secret").unwrap().as_str()
    )
}

/// Every file under `dir`, recursively.
pub fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}
