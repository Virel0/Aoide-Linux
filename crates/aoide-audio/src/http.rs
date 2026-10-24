//! A seekable reader over an HTTP stream.
//!
//! The body is downloaded progressively into an unnamed temporary file, and
//! reads are served from that file. A read of bytes that have not arrived
//! blocks briefly until they do. A read far past what has arrived (a seek to
//! the end for trailing tags, or the listener scrubbing ahead) restarts the
//! download there with a `Range` request; once that run reaches the end, the
//! downloader goes back and fills whatever it skipped, so the file ends up
//! whole and any later seek is served locally.
//!
//! A download lives as long as some reader is open on it: once the last one
//! closes, the transfer stops and the temporary file is deleted, so a queue
//! holding hundreds of played tracks holds no downloads. Opening again starts
//! afresh and fetches from wherever the new reader asks.
//!
//! The URL carries its own authentication as a query parameter, so it is never
//! written into an error message or a log line.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once, Weak};
use std::thread;
use std::time::{Duration, Instant};

/// How far ahead of the download a read may land before the download is
/// restarted there instead of waited for.
const RESTART_DISTANCE: u64 = 512 * 1024;
/// How long a read waits for bytes before it gives up.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK: usize = 64 * 1024;

/// One remote file. Readers open at the same time share one download;
/// nothing is fetched until the first [`HttpSource::open`], and the download
/// stops when the last reader closes. Cloning shares all of this.
#[derive(Clone)]
pub struct HttpSource {
    url: Arc<str>,
    live: Arc<Mutex<Weak<Shared>>>,
}

impl std::fmt::Debug for HttpSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the URL: it carries a token.
        f.debug_struct("HttpSource").finish_non_exhaustive()
    }
}

struct Shared {
    url: String,
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Default)]
struct State {
    file: Option<Arc<File>>,
    started: bool,
    /// The response headers of the first request have been read.
    responded: bool,
    total: Option<u64>,
    content_type: Option<String>,
    /// The server answered a `Range` request with the whole body, so seeking
    /// ahead means waiting for the download rather than restarting it.
    ignores_ranges: bool,
    /// Downloaded byte ranges, sorted and merged, end exclusive.
    have: Vec<(u64, u64)>,
    /// A position a reader is waiting on.
    wanted: Option<u64>,
    /// A stream with no announced length has been read to its end.
    ended_unsized: bool,
    /// A fatal error, already stripped of the URL.
    error: Option<String>,
    /// Requests sent, for tests and diagnostics.
    requests: usize,
}

impl State {
    /// The end of the downloaded range holding `pos`, if there is one.
    fn available_at(&self, pos: u64) -> Option<u64> {
        self.have
            .iter()
            .find(|&&(start, end)| start <= pos && pos < end)
            .map(|&(_, end)| end)
    }

    fn add(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        self.have.push((start, end));
        self.have.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.have.len());
        for &(start, end) in &self.have {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        self.have = merged;
    }

    fn complete(&self) -> bool {
        match self.total {
            Some(0) => true,
            Some(total) => self.have.first() == Some(&(0, total)),
            None => self.ended_unsized,
        }
    }

    /// The first byte not yet downloaded, counting from the start.
    fn first_gap(&self) -> u64 {
        match self.have.first() {
            Some(&(0, end)) => end,
            _ => 0,
        }
    }
}

impl HttpSource {
    /// A remote file at `url`. The URL already carries its authentication.
    pub fn new(url: impl Into<String>) -> Self {
        let url: String = url.into();
        Self {
            url: url.into(),
            live: Arc::new(Mutex::new(Weak::new())),
        }
    }

    /// The download in progress, if a reader is open.
    fn current(&self) -> Option<Arc<Shared>> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade()
    }

    /// A reader positioned at the start. Starts a download unless one is
    /// running for another open reader, and waits for the response headers.
    pub fn open(&self) -> io::Result<HttpReader> {
        let shared = {
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            match live.upgrade() {
                Some(shared) => shared,
                None => {
                    let shared = Arc::new(Shared {
                        url: self.url.to_string(),
                        state: Mutex::new(State::default()),
                        changed: Condvar::new(),
                    });
                    *live = Arc::downgrade(&shared);
                    shared
                }
            }
        };
        let mut state = shared.lock();
        if !state.started {
            state.started = true;
            state.file = Some(Arc::new(tempfile::tempfile()?));
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("aoide-http".into())
                .spawn(move || download(shared))?;
        }

        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            if let Some(error) = &state.error {
                return Err(io::Error::other(error.clone()));
            }
            if state.responded {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no response from the server",
                ));
            }
            state = shared
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let file = state
            .file
            .clone()
            .ok_or_else(|| io::Error::other("download not started"))?;
        drop(state);
        Ok(HttpReader {
            shared: Arc::clone(&shared),
            file,
            pos: 0,
        })
    }

    /// The `Content-Type` the server sent, while a reader is open.
    pub fn content_type(&self) -> Option<String> {
        self.current()?.lock().content_type.clone()
    }

    /// The length the server announced, while a reader is open.
    pub fn len(&self) -> Option<u64> {
        self.current()?.lock().total
    }

    /// Whether the server announced an empty body.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// Bytes downloaded so far by the open download, as merged
    /// `(start, end)` ranges; empty when no reader is open.
    pub fn downloaded(&self) -> Vec<(u64, u64)> {
        self.current()
            .map(|shared| shared.lock().have.clone())
            .unwrap_or_default()
    }

    /// How many requests the open download has sent.
    pub fn requests_sent(&self) -> usize {
        self.current().map_or(0, |shared| shared.lock().requests)
    }

    /// Whether a download is running (some reader is open).
    pub fn is_active(&self) -> bool {
        self.current().is_some()
    }
}

/// A `Read + Seek` view of an [`HttpSource`].
pub struct HttpReader {
    shared: Arc<Shared>,
    file: Arc<File>,
    pos: u64,
}

impl Read for HttpReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + READ_TIMEOUT;
        let mut state = self.shared.lock();
        let end = loop {
            if let Some(end) = state.available_at(self.pos) {
                if state.wanted == Some(self.pos) {
                    state.wanted = None;
                }
                break end;
            }
            let past_end = match state.total {
                Some(total) => self.pos >= total,
                None => state.ended_unsized,
            };
            if past_end {
                return Ok(0);
            }
            if let Some(error) = &state.error {
                return Err(io::Error::other(error.clone()));
            }
            state.wanted = Some(self.pos);
            self.shared.changed.notify_all();
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the stream stalled",
                ));
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        drop(state);
        let len = ((end - self.pos) as usize).min(buf.len());
        let read = self.file.read_at(&mut buf[..len], self.pos)?;
        self.pos += read as u64;
        Ok(read)
    }
}

impl Seek for HttpReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            SeekFrom::End(delta) => match self.shared.lock().total {
                Some(total) => total.checked_add_signed(delta),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "the stream has no length",
                    ));
                }
            },
        };
        self.pos = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for HttpReader {
    fn is_seekable(&self) -> bool {
        self.shared.lock().total.is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.shared.lock().total
    }
}

/// rustls with the ring provider, so no OpenSSL-licensed code is linked. An
/// application that installed its own provider first keeps it.
fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A reqwest error without its URL, which carries the token.
fn describe(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

fn download(shared: Arc<Shared>) {
    install_crypto();
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(None)
        .build()
    {
        Ok(client) => client,
        Err(error) => return fail(&shared, describe(error)),
    };

    let mut from: u64 = 0;
    loop {
        if abandoned(&shared) {
            return;
        }
        match run(&shared, &client, from) {
            Ok(Next::Restart(at)) => from = at,
            Ok(Next::ReachedEnd) => {
                let state = shared.lock();
                if state.complete() || state.total.is_none() {
                    drop(state);
                    shared.changed.notify_all();
                    return;
                }
                from = state
                    .wanted
                    .filter(|&at| state.available_at(at).is_none())
                    .unwrap_or_else(|| state.first_gap());
            }
            Ok(Next::Abandoned) => return,
            Err(message) => return fail(&shared, message),
        }
    }
}

/// Nobody but the downloader holds the source any more.
fn abandoned(shared: &Arc<Shared>) -> bool {
    Arc::strong_count(shared) == 1
}

fn fail(shared: &Shared, message: String) {
    shared.lock().error = Some(message);
    shared.changed.notify_all();
}

enum Next {
    Restart(u64),
    ReachedEnd,
    Abandoned,
}

/// One request from `from` to the end, or until a reader wants bytes too far
/// from the download to wait for.
fn run(
    shared: &Arc<Shared>,
    client: &reqwest::blocking::Client,
    from: u64,
) -> Result<Next, String> {
    let mut request = client.get(&shared.url);
    if from > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={from}-"));
    }
    shared.lock().requests += 1;
    let mut response = request.send().map_err(describe)?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("the server answered {}", status.as_u16()));
    }
    let partial = status == reqwest::StatusCode::PARTIAL_CONTENT;

    let mut at = if partial { from } else { 0 };
    let file = {
        let mut state = shared.lock();
        if from > 0 && !partial {
            state.ignores_ranges = true;
        }
        if state.total.is_none() {
            state.total = if partial {
                response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.rsplit('/').next())
                    .and_then(|total| total.trim().parse().ok())
            } else {
                response.content_length()
            };
        }
        if state.content_type.is_none() {
            state.content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
        }
        state.responded = true;
        match state.file.clone() {
            Some(file) => file,
            None => return Ok(Next::Abandoned),
        }
    };
    shared.changed.notify_all();

    let mut buf = vec![0u8; CHUNK];
    loop {
        let read = match response.read(&mut buf) {
            Ok(0) => {
                let mut state = shared.lock();
                if state.total.is_none() {
                    state.ended_unsized = true;
                }
                return Ok(Next::ReachedEnd);
            }
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("the download broke off: {}", error.kind())),
        };
        file.write_all_at(&buf[..read], at)
            .map_err(|error| error.to_string())?;
        let mut state = shared.lock();
        state.add(at, at + read as u64);
        at += read as u64;
        let restart = state.wanted.filter(|&wanted| {
            !state.ignores_ranges
                && state.available_at(wanted).is_none()
                && (wanted < at || wanted > at + RESTART_DISTANCE)
        });
        drop(state);
        shared.changed.notify_all();
        if abandoned(shared) {
            return Ok(Next::Abandoned);
        }
        if let Some(wanted) = restart {
            return Ok(Next::Restart(wanted));
        }
    }
}
