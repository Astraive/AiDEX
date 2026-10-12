//! A bounded `text/event-stream` reader for Streamable HTTP responses.

use std::io::{self, BufRead};

/// Read events from `reader`, calling `on_data` with the data of every
/// `message` event (the default type). `on_data` returns `false` to stop.
/// Lines and event payloads larger than `max` bytes are refused.
pub(crate) fn read_events(
    reader: &mut impl BufRead,
    max: usize,
    mut on_data: impl FnMut(String) -> bool,
) -> io::Result<()> {
    let mut data = String::new();
    let mut event = String::new();
    while let Some(line) = read_line_bounded(reader, max)? {
        if line.is_empty() {
            let payload = std::mem::take(&mut data);
            let kind = std::mem::take(&mut event);
            if !payload.is_empty()
                && (kind.is_empty() || kind == "message")
                && !on_data(payload.trim_end_matches('\n').to_owned())
            {
                return Ok(());
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        match field {
            "data" => {
                if data.len() + value.len() + 1 > max {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "an event exceeds the size limit",
                    ));
                }
                data.push_str(value);
                data.push('\n');
            }
            "event" => event = value.to_owned(),
            _ => {}
        }
    }
    if !data.is_empty() && (event.is_empty() || event == "message") {
        on_data(data.trim_end_matches('\n').to_owned());
    }
    Ok(())
}

/// One `\n`-terminated line without its `\r\n`, or `None` at end of stream.
pub(crate) fn read_line_bounded(
    reader: &mut impl BufRead,
    max: usize,
) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(position) = available.iter().position(|&byte| byte == b'\n') {
            line.extend_from_slice(&available[..position]);
            reader.consume(position + 1);
            break;
        }
        let length = available.len();
        line.extend_from_slice(available);
        reader.consume(length);
        if line.len() > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a stream line exceeds the size limit",
            ));
        }
    }
    if line.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a stream line exceeds the size limit",
        ));
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the stream is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_split_and_joined() {
        let text = ": comment\nevent: message\ndata: {\"a\":\ndata: 1}\n\nevent: ping\ndata: x\n\nid: 7\ndata: last";
        let mut reader = io::Cursor::new(text.as_bytes().to_vec());
        let mut seen = Vec::new();
        read_events(&mut reader, 1024, |data| {
            seen.push(data);
            true
        })
        .unwrap();
        assert_eq!(seen, ["{\"a\":\n1}", "last"]);
    }

    #[test]
    fn oversized_events_are_refused() {
        let text = format!("data: {}\n\n", "x".repeat(100));
        let mut reader = io::Cursor::new(text.into_bytes());
        assert!(read_events(&mut reader, 16, |_| true).is_err());
    }
}
