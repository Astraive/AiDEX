//! An in-test HTTP/1.1 server on 127.0.0.1 that records every connection and
//! request and answers with scripted replies (one per connection). No
//! external network is used.
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

use aidex_web::{
    CredentialRequest, DnsResolver, EgressPurpose, EgressRequest, Secret, WebClient, WebError,
    WebSettings,
};

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

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }

    /// Everything received, for "the secret never appears here" checks.
    pub fn text(&self) -> String {
        let mut text = format!("{} {}\n", self.method, self.path);
        for (key, value) in &self.headers {
            text.push_str(&format!("{key}: {value}\n"));
        }
        text.push_str(&String::from_utf8_lossy(&self.body));
        text
    }
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Sent only once the flag is set (or after ten seconds).
    pub hold: Option<Arc<AtomicBool>>,
}

impl Reply {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
            hold: None,
        }
    }

    pub fn json(status: u16, body: &str) -> Self {
        Self::new(status, "application/json", body)
    }

    pub fn html(body: &str) -> Self {
        Self::new(200, "text/html; charset=utf-8", body)
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self {
            status,
            headers: vec![("Location".into(), location.into())],
            body: Vec::new(),
            hold: None,
        }
    }

    pub fn without_content_type(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            hold: None,
        }
    }

    pub fn held(mut self, flag: Arc<AtomicBool>) -> Self {
        self.hold = Some(flag);
        self
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

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
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
    requests.lock().unwrap().push(RecordedRequest {
        method,
        path,
        headers,
        body,
    });
    let reply = reply.unwrap_or_else(|| Reply::json(500, r#"{"error":"no scripted reply"}"#));
    if let Some(flag) = &reply.hold {
        let until = Instant::now() + Duration::from_secs(10);
        while !flag.load(Ordering::SeqCst) && Instant::now() < until {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    let mut stream = stream;
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (key, value) in &reply.headers {
        head.push_str(&format!("{key}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&reply.body))
        .and_then(|()| stream.flush());
}

/// A key value generated per run, so no credential lives in a fixture.
pub fn sentinel_secret() -> String {
    format!(
        "sk-test-{}",
        aidex_core::EntityId::generate("secret").unwrap().as_str()
    )
}

/// One approved or refused connection, as the egress check saw it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EgressCall {
    pub purpose: EgressPurpose,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub hop: usize,
}

/// Records egress calls and answers with `decide`.
#[derive(Clone, Default)]
pub struct EgressLog(pub Arc<Mutex<Vec<EgressCall>>>);

impl EgressLog {
    pub fn calls(&self) -> Vec<EgressCall> {
        self.0.lock().unwrap().clone()
    }

    pub fn check<F>(
        &self,
        decide: F,
    ) -> impl Fn(&EgressRequest<'_>) -> Result<(), WebError> + Send + Sync + 'static + use<F>
    where
        F: Fn(&EgressCall) -> Result<(), WebError> + Send + Sync + 'static,
    {
        let log = Arc::clone(&self.0);
        move |request: &EgressRequest<'_>| {
            let call = EgressCall {
                purpose: request.purpose,
                scheme: request.scheme.to_owned(),
                host: request.host.to_owned(),
                port: request.port,
                hop: request.hop,
            };
            log.lock().unwrap().push(call.clone());
            decide(&call)
        }
    }

    pub fn allow_all(
        &self,
    ) -> impl Fn(&EgressRequest<'_>) -> Result<(), WebError> + Send + Sync + 'static + use<> {
        self.check(|_| Ok(()))
    }
}

/// Records which secrets were asked for and releases `value` for each.
#[derive(Clone, Default)]
pub struct CredentialLog(pub Arc<Mutex<Vec<String>>>);

impl CredentialLog {
    pub fn requests(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    pub fn release(
        &self,
        value: Result<String, WebError>,
    ) -> impl Fn(&CredentialRequest<'_>) -> Result<Secret, WebError> + Send + Sync + 'static + use<>
    {
        let log = Arc::clone(&self.0);
        move |request: &CredentialRequest<'_>| {
            log.lock()
                .unwrap()
                .push(format!("{}:{}", request.provider, request.secret_name));
            value.clone().map(Secret::new)
        }
    }
}

/// A DNS resolver that must never be consulted.
pub fn no_dns(used: Arc<AtomicUsize>) -> DnsResolver {
    Arc::new(move |_, _| {
        used.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("DNS must not be used in this test"))
    })
}

/// A DNS resolver answering every name with `addresses`.
pub fn fixed_dns(addresses: &'static [&'static str]) -> DnsResolver {
    Arc::new(move |_, port| {
        Ok(addresses
            .iter()
            .map(|ip| SocketAddr::new(ip.parse().unwrap(), port))
            .collect())
    })
}

/// A client that releases no keys and allows every connection.
pub fn open_client(settings: WebSettings) -> WebClient {
    WebClient::new(
        settings,
        |_: &CredentialRequest<'_>| Err(WebError::NotConfigured("no key in this test".into())),
        |_: &EgressRequest<'_>| Ok(()),
    )
    .unwrap()
}
