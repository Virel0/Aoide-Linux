//! The HTTP reader against local servers, never the network: wiremock with a
//! Range-aware responder, and for the seek-ahead case a small server that
//! trickles its body out so there is a download to get ahead of.

mod common;

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aoide_audio::http::HttpSource;
use aoide_audio::{EngineEvent, EntryId, MediaSource, TrackSpec};
use common::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Not a real key: the reader must carry the query string as given.
const QUERY: &str = "?api_key=not-a-real-key&static=true";

/// Serves `body`, honouring `Range: bytes=N-` with 206 and Content-Range.
struct Ranged {
    body: Vec<u8>,
    honour_ranges: bool,
}

impl Respond for Ranged {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let from = request
            .headers
            .get("range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes="))
            .and_then(|v| v.trim_end_matches('-').parse::<usize>().ok());
        match from {
            Some(from) if self.honour_ranges && from < self.body.len() => {
                ResponseTemplate::new(206)
                    .insert_header("content-type", "audio/flac")
                    .insert_header(
                        "content-range",
                        format!("bytes {from}-{}/{}", self.body.len() - 1, self.body.len())
                            .as_str(),
                    )
                    .set_body_bytes(self.body[from..].to_vec())
            }
            _ => ResponseTemplate::new(200)
                .insert_header("content-type", "audio/flac")
                .set_body_bytes(self.body.clone()),
        }
    }
}

struct Server {
    _runtime: tokio::runtime::Runtime,
    server: MockServer,
    runtime: tokio::runtime::Handle,
}

impl Server {
    fn start(body: Vec<u8>, honour_ranges: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/audio"))
                .respond_with(Ranged {
                    body,
                    honour_ranges,
                })
                .mount(&server)
                .await;
            server
        });
        let handle = runtime.handle().clone();
        Self {
            _runtime: runtime,
            server,
            runtime: handle,
        }
    }

    fn url(&self) -> String {
        format!("{}/audio{QUERY}", self.server.uri())
    }

    fn ranges(&self) -> Vec<Option<String>> {
        let requests = self
            .runtime
            .block_on(self.server.received_requests())
            .unwrap_or_default();
        requests
            .iter()
            .map(|r| {
                r.headers
                    .get("range")
                    .map(|v| v.to_str().unwrap().to_owned())
            })
            .collect()
    }
}

fn noise_flac(seconds: usize) -> (Vec<i32>, Vec<u8>) {
    let samples = noise(7, 0.5, seconds * RATE as usize, 2);
    let bytes = flac_bytes(RATE, 2, &samples);
    (samples, bytes)
}

#[test]
fn a_stream_decodes_to_the_same_frames_as_the_file() {
    let (samples, bytes) = noise_flac(2);
    let server = Server::start(bytes, true);
    let (mut engine, handle, events) = inline_engine(2);
    let id = EntryId::fresh();
    handle
        .load(TrackSpec::new(id, MediaSource::http(server.url())), 0, true)
        .unwrap();
    let out = render(&mut engine, samples.len() / 2, 1024);
    assert_eq!(out, to_f32(&samples));
    assert!(drain(&events).contains(&EngineEvent::TrackStarted { id }));
}

#[test]
fn a_seek_in_a_stream_lands_on_its_frame() {
    let (samples, bytes) = noise_flac(3);
    println!("flac fixture: {} bytes", bytes.len());
    let server = Server::start(bytes, true);
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::http(server.url())),
            2_500,
            true,
        )
        .unwrap();
    let out = render(&mut engine, 4_800, 480);
    let start = 2_500 * 48;
    assert_eq!(out, to_f32(&samples[start * 2..(start + 4_800) * 2]));
    println!("requests: {:?}", server.ranges());
}

#[test]
fn a_server_that_ignores_ranges_still_plays_and_seeks() {
    let (samples, bytes) = noise_flac(2);
    let server = Server::start(bytes, false);
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::http(server.url())),
            1_000,
            true,
        )
        .unwrap();
    let out = render(&mut engine, 4_800, 480);
    let start = 1_000 * 48;
    assert_eq!(out, to_f32(&samples[start * 2..(start + 4_800) * 2]));
}

#[test]
fn a_failed_request_is_an_error_that_does_not_carry_the_url() {
    let server = Server::start(Vec::new(), true);
    let missing = format!("{}/missing{QUERY}", server.server.uri());
    let (mut engine, handle, events) = inline_engine(2);
    let id = EntryId::fresh();
    handle
        .load(TrackSpec::new(id, MediaSource::http(missing)), 0, true)
        .unwrap();
    render(&mut engine, 480, 480);
    let events = drain(&events);
    let message = events
        .iter()
        .find_map(|e| match e {
            EngineEvent::Error {
                id: failed,
                message,
            } if *failed == id => Some(message.clone()),
            _ => None,
        })
        .expect("an error event");
    println!("error: {message}");
    assert!(message.contains("404"));
    assert!(
        !message.contains("api_key") && !message.contains(&server.server.address().to_string()),
        "the URL leaked: {message}"
    );
    assert!(events.contains(&EngineEvent::Finished));
}

/// A server that sends its body in small pieces with a pause between them,
/// honouring `Range: bytes=N-`, and records the range of every request.
fn trickling_server(body: Arc<Vec<u8>>) -> (String, Arc<Mutex<Vec<Option<u64>>>>) {
    let listener = TcpListener::bind("localhost:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = Arc::clone(&body);
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut from = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = value.trim().trim_end_matches('-').parse::<u64>().ok();
                    }
                }
                log.lock().unwrap().push(from);
                let start = from.unwrap_or(0) as usize;
                let head = match from {
                    Some(_) => format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                        body.len() - start,
                        body.len() - 1,
                        body.len()
                    ),
                    None => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ),
                };
                if stream.write_all(head.as_bytes()).is_err() {
                    return;
                }
                for piece in body[start..].chunks(16 * 1024) {
                    if stream.write_all(piece).is_err() {
                        return; // the client went elsewhere
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
        }
    });
    (format!("http://{address}/audio{QUERY}"), seen)
}

#[test]
fn a_read_past_the_download_restarts_it_there_with_a_range_request() {
    // Four megabytes at about 1.6 MB/s: roughly 2.5 s to arrive in order.
    let body: Vec<u8> = (0..4 * 1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let body = Arc::new(body);
    let (url, seen) = trickling_server(Arc::clone(&body));
    let source = HttpSource::new(url);
    let mut reader = source.open().unwrap();

    let mut head = [0u8; 1024];
    reader.read_exact(&mut head).unwrap();
    assert_eq!(&head[..], &body[..1024]);

    let far = 3 * 1024 * 1024 + 17;
    let started = Instant::now();
    reader.seek(SeekFrom::Start(far)).unwrap();
    let mut piece = [0u8; 4096];
    reader.read_exact(&mut piece).unwrap();
    let waited = started.elapsed();
    assert_eq!(&piece[..], &body[far as usize..far as usize + 4096]);
    println!(
        "read 3 MB ahead after {waited:?}; requests {:?}",
        seen.lock().unwrap()
    );
    assert!(
        waited < Duration::from_millis(1_000),
        "waited {waited:?}, as long as the download itself"
    );
    assert!(
        seen.lock().unwrap().contains(&Some(far)),
        "no Range request from the seek point: {:?}",
        seen.lock().unwrap()
    );

    // Back at the start, the gap is filled and every byte is right.
    reader.seek(SeekFrom::Start(0)).unwrap();
    let mut all = Vec::new();
    reader.read_to_end(&mut all).unwrap();
    assert_eq!(all.len(), body.len());
    assert!(
        all == *body,
        "the file put together from several requests differs from the body"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while source.downloaded() != vec![(0, body.len() as u64)] && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(source.downloaded(), vec![(0, body.len() as u64)]);

    // The last reader closing ends the download and its file.
    drop(reader);
    assert!(!source.is_active());
    assert!(source.downloaded().is_empty());
}

#[test]
fn bytes_not_yet_arrived_are_waited_for_briefly() {
    let body: Vec<u8> = (0..256 * 1024u32).map(|i| (i % 251) as u8).collect();
    let body = Arc::new(body);
    let (url, seen) = trickling_server(Arc::clone(&body));
    let source = HttpSource::new(url);
    let mut reader = source.open().unwrap();
    // Within the restart distance: waited for, not re-requested.
    let near = 200 * 1024;
    reader.seek(SeekFrom::Start(near)).unwrap();
    let mut piece = [0u8; 1024];
    reader.read_exact(&mut piece).unwrap();
    assert_eq!(&piece[..], &body[near as usize..near as usize + 1024]);
    assert_eq!(*seen.lock().unwrap(), vec![None], "one request, no range");
}

#[test]
fn the_last_reader_closing_ends_the_download_at_once() {
    // A body the trickling server takes seconds to send, so the download
    // thread is certainly still running when the reader goes. The source
    // must count as closed the moment the reader is dropped, not when the
    // thread next notices.
    let body = Arc::new(
        (0..2 * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<u8>>(),
    );
    let (url, _) = trickling_server(Arc::clone(&body));
    let source = HttpSource::new(url);
    let mut reader = source.open().unwrap();
    let mut head = [0u8; 16];
    reader.read_exact(&mut head).unwrap();
    assert!(source.is_active());
    assert!(source.downloaded().iter().map(|(a, b)| b - a).sum::<u64>() < body.len() as u64);

    drop(reader);
    assert!(!source.is_active());
    assert!(source.downloaded().is_empty());
}
