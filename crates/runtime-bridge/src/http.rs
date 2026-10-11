use crate::BridgeError;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};
pub(crate) const LIMIT: usize = 1024 * 1024;
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}
pub(crate) fn read(stream: &mut TcpStream) -> Result<Request, BridgeError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    read_with_deadline(stream, deadline)
}
fn read_with_deadline(
    stream: &mut TcpStream,
    deadline: std::time::Instant,
) -> Result<Request, BridgeError> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| BridgeError::new("transport", "cannot set read deadline"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| BridgeError::new("transport", "cannot set write deadline"))?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let split = loop {
        let count = read_until(stream, &mut chunk, deadline)?;
        if count == 0 {
            return Err(BridgeError::new("bad_request", "request ended early"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index;
        }
        if bytes.len() > 16384 {
            return Err(BridgeError::new("too_large", "headers exceed limit"));
        }
    };
    if split > 16384 {
        return Err(BridgeError::new("too_large", "headers exceed limit"));
    }
    let head = std::str::from_utf8(&bytes[..split])
        .map_err(|_| BridgeError::new("bad_request", "headers must be UTF-8"))?;
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split_whitespace();
    let method = start.next().unwrap_or_default().to_owned();
    let path = start.next().unwrap_or_default().to_owned();
    if start.next() != Some("HTTP/1.1") || start.next().is_some() {
        return Err(BridgeError::new("bad_request", "invalid HTTP request line"));
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| BridgeError::new("bad_request", "invalid header"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(BridgeError::new("bad_request", "invalid header name"));
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err(BridgeError::new("bad_request", "duplicate header"));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(BridgeError::new(
            "bad_request",
            "chunked requests are unsupported",
        ));
    }
    let length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| BridgeError::new("bad_request", "invalid content length"))?
        .unwrap_or(0);
    if length > LIMIT {
        return Err(BridgeError::new("too_large", "request exceeds 1 MiB"));
    }
    let mut body = bytes[split + 4..].to_vec();
    if body.len() > length {
        return Err(BridgeError::new(
            "bad_request",
            "pipelined requests are unsupported",
        ));
    }
    while body.len() < length {
        let left = (length - body.len()).min(chunk.len());
        let count = read_until(stream, &mut chunk[..left], deadline)?;
        if count == 0 {
            return Err(BridgeError::new("bad_request", "request ended early"));
        }
        body.extend_from_slice(&chunk[..count]);
    }
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}
fn read_until(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: std::time::Instant,
) -> Result<usize, BridgeError> {
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    if left.is_zero() {
        return Err(BridgeError::new(
            "bad_request",
            "absolute request deadline expired",
        ));
    }
    stream
        .set_read_timeout(Some(left.min(Duration::from_secs(5))))
        .map_err(|_| BridgeError::new("transport", "cannot set request deadline"))?;
    stream
        .read(buffer)
        .map_err(|_| BridgeError::new("bad_request", "request read failed"))
}
pub(crate) fn write(stream: &mut TcpStream, status: u16, body: Option<&serde_json::Value>) {
    let text = body.map(ToString::to_string).unwrap_or_default();
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Service Unavailable",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn periodic_bytes_do_not_reset_absolute_header_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let sender = std::thread::spawn(move || {
            let mut client = TcpStream::connect(address).unwrap();
            for _ in 0..100 {
                if client.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let (mut stream, _) = listener.accept().unwrap();
        let start = std::time::Instant::now();
        assert!(read_with_deadline(&mut stream, start + Duration::from_millis(50)).is_err());
        assert!(start.elapsed() < Duration::from_millis(500));
        drop(stream);
        sender.join().unwrap();
    }
}
