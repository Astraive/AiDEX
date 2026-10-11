use crate::{BridgeConnection, BridgeDispatcher, BridgeError, BridgeScope, http, protocol};
use aidex_core::CancelToken;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
pub(crate) struct Cached {
    pub fingerprint: [u8; 32],
    pub result: Option<Value>,
}
pub(crate) struct Record {
    pub scope: BridgeScope,
    pub dispatcher: Arc<dyn BridgeDispatcher>,
    pub cancel: CancelToken,
    pub expires: Instant,
    pub phase: AtomicU8,
    pub calls: Mutex<HashMap<String, Cached>>,
}
pub(crate) struct State {
    pub records: Mutex<HashMap<[u8; 32], Arc<Record>>>,
    pub closed: AtomicBool,
    pub authority: String,
}
pub struct BridgeServer {
    state: Arc<State>,
    endpoint: String,
}
pub struct BridgeLease {
    connection: BridgeConnection,
    key: [u8; 32],
    state: Weak<State>,
}
pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
impl BridgeServer {
    pub fn bind() -> Result<Arc<Self>, BridgeError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|_| BridgeError::new("unavailable", "cannot bind runtime MCP bridge"))?;
        listener
            .set_nonblocking(true)
            .map_err(|_| BridgeError::new("unavailable", "cannot configure listener"))?;
        let authority = listener
            .local_addr()
            .map_err(|_| BridgeError::new("unavailable", "cannot resolve listener"))?
            .to_string();
        let state = Arc::new(State {
            records: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            authority: authority.clone(),
        });
        let (sender, receiver) = mpsc::sync_channel::<TcpStream>(16);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..4 {
            let receiver = receiver.clone();
            let state = state.clone();
            std::thread::Builder::new()
                .name("aidex-mcp-worker".into())
                .spawn(move || {
                    loop {
                        let stream = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        match stream {
                            Ok(mut stream) => serve(&state, &mut stream),
                            Err(_) => break,
                        }
                    }
                })
                .map_err(|_| BridgeError::new("unavailable", "cannot start bridge worker"))?;
        }
        let accept_state = state.clone();
        std::thread::Builder::new()
            .name("aidex-mcp-listener".into())
            .spawn(move || {
                while !accept_state.closed.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, address)) => {
                            // Windows accepted sockets inherit the listener's nonblocking mode.
                            // Workers use blocking reads with bounded request deadlines.
                            if stream.set_nonblocking(false).is_err() {
                                continue;
                            }
                            if !address.ip().is_loopback() {
                                http::write(&mut stream, 403, None);
                                continue;
                            }
                            if let Err(mpsc::TrySendError::Full(mut stream)) =
                                sender.try_send(stream)
                            {
                                http::write(&mut stream, 429, None);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| BridgeError::new("unavailable", "cannot start bridge listener"))?;
        Ok(Arc::new(Self {
            state,
            endpoint: format!("http://{authority}/mcp"),
        }))
    }
    pub fn lease(
        &self,
        scope: BridgeScope,
        dispatcher: Arc<dyn BridgeDispatcher>,
        cancel: CancelToken,
        ttl: Duration,
    ) -> Result<BridgeLease, BridgeError> {
        if self.state.closed.load(Ordering::SeqCst)
            || cancel.is_stopped()
            || !dispatcher.active(&scope)
        {
            return Err(BridgeError::new(
                "invalid_scope",
                "bridge requires an active bound run",
            ));
        }
        let mut random = Zeroizing::new([0u8; 32]);
        getrandom::fill(random.as_mut())
            .map_err(|_| BridgeError::new("unavailable", "secure credential generation failed"))?;
        let token = Zeroizing::new(
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        );
        let key = digest(token.as_bytes());
        let record = Arc::new(Record {
            scope,
            dispatcher,
            cancel,
            expires: Instant::now() + ttl.min(Duration::from_secs(3600)),
            phase: AtomicU8::new(0),
            calls: Mutex::new(HashMap::new()),
        });
        let mut records = self.state.records.lock().unwrap_or_else(|e| e.into_inner());
        records.retain(|_, record| !record.cancel.is_stopped() && record.expires > Instant::now());
        if records.len() >= 32 {
            return Err(BridgeError::new(
                "capacity",
                "runtime bridge lease limit reached",
            ));
        }
        records.insert(key, record);
        Ok(BridgeLease {
            connection: BridgeConnection {
                endpoint: self.endpoint.clone(),
                token: Arc::new(token),
            },
            key,
            state: Arc::downgrade(&self.state),
        })
    }
    pub fn revoke_run(&self, session: &str, run: &str) {
        self.state
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, record| {
                let keep = record.scope.session_id != session || record.scope.run_id != run;
                if !keep {
                    record.cancel.cancel();
                }
                keep
            });
    }
    pub fn shutdown(&self) {
        self.state.closed.store(true, Ordering::SeqCst);
        for (_, record) in self
            .state
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
        {
            record.cancel.cancel();
        }
    }
}
impl Drop for BridgeServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl BridgeLease {
    pub fn connection(&self) -> BridgeConnection {
        self.connection.clone()
    }
    pub fn active(&self) -> bool {
        self.state.upgrade().is_some_and(|state| {
            state
                .records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&self.key)
                .is_some_and(|record| {
                    !record.cancel.is_stopped() && record.expires > Instant::now()
                })
        })
    }
}
impl Drop for BridgeLease {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade()
            && let Some(record) = state
                .records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.key)
        {
            record.cancel.cancel();
        }
    }
}
fn serve(state: &State, stream: &mut TcpStream) {
    let request = match http::read(stream) {
        Ok(request) => request,
        Err(error) => {
            http::write(
                stream,
                if error.code == "too_large" { 413 } else { 400 },
                None,
            );
            return;
        }
    };
    if request.headers.get("host") != Some(&state.authority)
        || request
            .headers
            .get("origin")
            .is_some_and(|origin| origin != &format!("http://{}", state.authority))
    {
        http::write(stream, 403, None);
        return;
    }
    if request.path != "/mcp" {
        http::write(stream, 404, None);
        return;
    }
    let Some(token) = request
        .headers
        .get("authorization")
        .and_then(|header| header.strip_prefix("Bearer "))
        .filter(|token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        http::write(stream, 401, None);
        return;
    };
    let record = state
        .records
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&digest(token.as_bytes()))
        .cloned();
    let Some(record) =
        record.filter(|record| !record.cancel.is_stopped() && record.expires > Instant::now())
    else {
        http::write(stream, 401, None);
        return;
    };
    if !record.dispatcher.active(&record.scope) {
        http::write(stream, 403, None);
        return;
    }
    if request.method != "POST" {
        http::write(stream, 405, None);
        return;
    }
    if !request
        .headers
        .get("content-type")
        .is_some_and(|value| value.starts_with("application/json"))
        || !request.headers.get("accept").is_some_and(|value| {
            value.contains("application/json") && value.contains("text/event-stream")
        })
    {
        http::write(stream, 400, None);
        return;
    }
    if request
        .headers
        .get("mcp-protocol-version")
        .is_some_and(|version| version != "2025-06-18" && version != "2025-03-26")
    {
        http::write(stream, 400, None);
        return;
    }
    let input = match serde_json::from_slice(&request.body) {
        Ok(input) => input,
        Err(_) => {
            http::write(stream, 400, None);
            return;
        }
    };
    let (status, body) = protocol::handle(&record, input);
    http::write(stream, status, body.as_ref());
}
