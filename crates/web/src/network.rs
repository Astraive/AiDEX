use crate::{Result, WebError};
use aidex_tools::CancelToken;
use reqwest::{Url, blocking::Client, redirect::Policy};
use std::{
    io::Read,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// Backend-only request. Intentionally has no Debug/Serialize implementation.
pub struct HttpRequest {
    pub url: String,
    pub json: Option<serde_json::Value>,
    pub authorization: Option<(String, Zeroizing<String>)>,
    pub network_allowed: Arc<dyn Fn() -> bool + Send + Sync>,
}
pub struct HttpReply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub final_url: String,
}
pub trait HttpTransport: Send + Sync {
    fn send(&self, request: HttpRequest, cancel: &CancelToken) -> Result<HttpReply>;
}
#[derive(Default)]
pub struct PublicHttpsTransport {
    active: Arc<AtomicUsize>,
}
struct Admission(Arc<AtomicUsize>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub fn validate_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| WebError::Invalid)?;
    if value.len() > 8192
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.port_or_known_default() != Some(443)
    {
        return Err(WebError::Invalid);
    }
    if let Some(host) = url.host_str() {
        let host = host.trim_matches(['[', ']']);
        if host.eq_ignore_ascii_case("localhost")
            || host.ends_with(".localhost")
            || host.ends_with(".local")
        {
            return Err(WebError::Invalid);
        }
        if let Ok(ip) = host.parse::<IpAddr>()
            && !public_address(ip)
        {
            return Err(WebError::Invalid);
        }
    }
    Ok(url)
}
pub fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_address(IpAddr::V4(mapped));
            }
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] == 0x0db8 || s[1] < 0x0200))
        }
    }
}
fn admit(request: &HttpRequest, cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(WebError::Cancelled);
    }
    if !(request.network_allowed)() {
        return Err(WebError::Offline);
    }
    Ok(())
}
fn request(mut request: HttpRequest, cancel: CancelToken) -> Result<HttpReply> {
    for redirect in 0..=3 {
        admit(&request, &cancel)?;
        let url = validate_url(&request.url)?;
        let host = url
            .host_str()
            .ok_or(WebError::Invalid)?
            .trim_matches(['[', ']']);
        let addresses: Vec<SocketAddr> = (host, 443)
            .to_socket_addrs()
            .map_err(|_| WebError::Transport)?
            .collect();
        if addresses.is_empty()
            || addresses.len() > 32
            || addresses.iter().any(|ip| !public_address(ip.ip()))
        {
            return Err(WebError::Invalid);
        }
        admit(&request, &cancel)?;
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| WebError::Transport)?;
        let mut builder = if let Some(json) = &request.json {
            client.post(url.clone()).json(json)
        } else {
            client.get(url.clone())
        };
        builder = builder
            .header("Accept", "application/json,text/plain,text/html")
            .header("User-Agent", "AiDEX/0.1");
        if let Some((name, value)) = &request.authorization {
            builder = builder.header(name.as_str(), value.as_str());
        }
        admit(&request, &cancel)?;
        let mut response = builder.send().map_err(|_| WebError::Transport)?;
        let status = response.status().as_u16();
        if response.status().is_redirection() {
            // Search authentication is never forwarded. Public fetches validate and pin every hop.
            if request.authorization.is_some() || request.json.is_some() || redirect == 3 {
                return Err(WebError::Response);
            }
            let next = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or(WebError::Response)?;
            let destination = url.join(next).map_err(|_| WebError::Response)?;
            if destination.origin() != url.origin() {
                return Err(WebError::Invalid);
            }
            request.url = destination.to_string();
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(WebError::Http(status));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if response.content_length().is_some_and(|n| n > 1024 * 1024) {
            return Err(WebError::TooLarge);
        }
        let mut body = vec![];
        let mut chunk = [0u8; 8192];
        loop {
            admit(&request, &cancel)?;
            let read = response.read(&mut chunk).map_err(|_| WebError::Transport)?;
            if read == 0 {
                break;
            }
            if body.len() + read > 1024 * 1024 {
                return Err(WebError::TooLarge);
            }
            body.extend_from_slice(&chunk[..read]);
        }
        if body.len() > 1024 * 1024 {
            return Err(WebError::TooLarge);
        }
        return Ok(HttpReply {
            status,
            content_type,
            body,
            final_url: url.to_string(),
        });
    }
    Err(WebError::Response)
}
impl HttpTransport for PublicHttpsTransport {
    fn send(&self, request_data: HttpRequest, cancel: &CancelToken) -> Result<HttpReply> {
        if cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }
        if self.active.fetch_add(1, Ordering::AcqRel) >= 8 {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(WebError::Transport);
        }
        let lease = Admission(self.active.clone());
        let (tx, rx) = mpsc::sync_channel(1);
        let admission = CancelToken::new();
        let worker_cancel = admission.clone();
        std::thread::Builder::new()
            .name("aidex-public-web".into())
            .spawn(move || {
                let _lease = lease;
                let result = if worker_cancel.is_cancelled() {
                    Err(WebError::Cancelled)
                } else {
                    request(request_data, worker_cancel)
                };
                let _ = tx.send(result);
            })
            .map_err(|_| WebError::Transport)?;
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if cancel.is_cancelled() {
                admission.cancel();
                return Err(WebError::Cancelled);
            }
            if Instant::now() >= deadline {
                admission.cancel();
                return Err(WebError::Transport);
            }
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(WebError::Transport),
            }
        }
    }
}
pub(crate) fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(PublicHttpsTransport::default())
}
