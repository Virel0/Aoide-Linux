//! Server-Sent Events, as Melete sends them: `event:` and `data:` lines, an
//! empty line ending each event, and `:` comments between to keep the
//! connection alive. Melete ends a stream after 10 minutes; open another.

use crate::error::transport;
use crate::{Error, Result};

/// The longest event kept: Melete's are a page of JSON at most.
const MAX_EVENT: usize = 4 << 20;

/// One event: its name (`message` when the server gave none) and its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The `event:` line.
    pub name: String,
    /// The `data:` lines, joined by newlines.
    pub data: String,
}

/// An open stream of events.
pub struct EventStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
}

impl EventStream {
    pub(crate) fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            buffer: Vec::new(),
        }
    }

    /// The next event; `None` once the server ended the stream.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`] when the connection fails or goes silent;
    /// [`Error::Decoding`] for an event past any Melete sends.
    pub async fn next(&mut self) -> Result<Option<Event>> {
        loop {
            if let Some(event) = take_event(&mut self.buffer) {
                return Ok(Some(event));
            }
            match self.response.chunk().await.map_err(transport)? {
                Some(chunk) => self.buffer.extend_from_slice(&chunk),
                None => return Ok(None),
            }
            if self.buffer.len() > MAX_EVENT {
                return Err(Error::Decoding {
                    path: "events".to_owned(),
                    message: "an event larger than any Melete sends".to_owned(),
                });
            }
        }
    }
}

/// Where the first blank line is: the end of an event and the gap's length.
fn blank_line(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|at| (at, 2));
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| (at, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// Takes the first whole event out of `buffer`, skipping comments and
/// anything without data. Split on bytes, so a character cut between two
/// chunks is decoded whole.
fn take_event(buffer: &mut Vec<u8>) -> Option<Event> {
    loop {
        let (end, gap) = blank_line(buffer)?;
        let block: Vec<u8> = buffer.drain(..end + gap).take(end).collect();
        let text = String::from_utf8_lossy(&block);
        let mut name = String::from("message");
        let mut data: Vec<&str> = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("event:") {
                name = value.trim().to_owned();
            } else if let Some(value) = line.strip_prefix("data:") {
                data.push(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        if !data.is_empty() {
            return Some(Event {
                name,
                data: data.join("\n"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_read_whole_and_comments_skipped() {
        let mut buffer = b": keep-alive\n\nevent: requests\ndata: [1,\ndata: 2]\n\nevent: sync\r\ndata: {}\r\n\r\npartial".to_vec();
        assert_eq!(
            take_event(&mut buffer),
            Some(Event {
                name: "requests".into(),
                data: "[1,\n2]".into()
            })
        );
        assert_eq!(take_event(&mut buffer).map(|e| e.name), Some("sync".into()));
        assert_eq!(take_event(&mut buffer), None);
        assert_eq!(buffer, b"partial");
    }

    #[test]
    fn a_character_cut_between_chunks_is_decoded_whole() {
        let whole = "event: requests\ndata: [\"Björk\"]\n\n".as_bytes();
        let cut = whole.iter().position(|b| *b == 0xC3).unwrap() + 1;
        let mut buffer = whole[..cut].to_vec();
        assert_eq!(take_event(&mut buffer), None);
        buffer.extend_from_slice(&whole[cut..]);
        assert_eq!(take_event(&mut buffer).unwrap().data, "[\"Björk\"]");
    }
}
