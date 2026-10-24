//! Album covers: fetched over HTTP, kept on disk in the cache directory and in
//! memory (bounded), and handed to iced as image handles.
//!
//! A cover's URL carries its image tag, which changes when the image does, so
//! a cached file never goes stale: a new cover is a new key.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use iced::widget::image;

/// How many covers are kept decoded-ready in memory.
pub const MEMORY_CAPACITY: usize = 600;
/// How many downloads run at once.
pub const CONCURRENT: usize = 6;
/// Every cover is asked for at this width, so one file serves every size it
/// is drawn at (a 56 px row and a 220 px header, on a HiDPI screen).
pub const WIDTH: u32 = 480;
/// The largest body accepted as a cover.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// One cover: the item that owns it and its tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtKey {
    pub item: String,
    pub tag: String,
}

impl ArtKey {
    pub fn new(item: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            item: item.into(),
            tag: tag.into(),
        }
    }

    /// A file name that cannot leave the cache directory, whatever the server
    /// sent as an id.
    pub fn file_name(&self) -> String {
        let clean = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(64)
                .collect()
        };
        format!("{}-{}-{WIDTH}.img", clean(&self.item), clean(&self.tag))
    }
}

/// The memory side of the cache, and the download queue.
#[derive(Debug, Default)]
pub struct Artwork {
    dir: Option<PathBuf>,
    handles: HashMap<ArtKey, (image::Handle, u64)>,
    clock: u64,
    /// Asked for and not yet started.
    waiting: VecDeque<ArtKey>,
    /// Asked for at some point: queued, downloading, done or failed. A failure
    /// is not retried this session; a missing cover is not worth a request per
    /// redraw.
    seen: HashSet<ArtKey>,
    in_flight: usize,
}

impl Artwork {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            ..Self::default()
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn get(&self, key: &ArtKey) -> Option<&image::Handle> {
        self.handles.get(key).map(|(handle, _)| handle)
    }

    /// Queues covers not yet asked for.
    pub fn want(&mut self, keys: impl IntoIterator<Item = ArtKey>) {
        for key in keys {
            if self.seen.insert(key.clone()) {
                self.waiting.push_back(key);
            }
        }
    }

    /// The covers to start downloading now, keeping at most [`CONCURRENT`] going.
    pub fn next_batch(&mut self) -> Vec<ArtKey> {
        let mut batch = Vec::new();
        while self.in_flight < CONCURRENT {
            let Some(key) = self.waiting.pop_front() else {
                break;
            };
            self.in_flight += 1;
            batch.push(key);
        }
        batch
    }

    /// A download finished. `None` is a cover that could not be had.
    pub fn arrived(&mut self, key: ArtKey, bytes: Option<Vec<u8>>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        let Some(bytes) = bytes else {
            return;
        };
        self.clock += 1;
        self.handles
            .insert(key, (image::Handle::from_bytes(bytes), self.clock));
        if self.handles.len() > MEMORY_CAPACITY {
            // Evict the least recently stored eighth in one pass rather than one
            // entry per arrival.
            let mut stamps: Vec<u64> = self.handles.values().map(|(_, s)| *s).collect();
            stamps.sort_unstable();
            let cut = stamps[MEMORY_CAPACITY / 8];
            let evicted: Vec<ArtKey> = self
                .handles
                .iter()
                .filter(|(_, (_, stamp))| *stamp < cut)
                .map(|(key, _)| key.clone())
                .collect();
            for key in evicted {
                self.handles.remove(&key);
                // Evicted from memory, still on disk: it may be asked for again.
                self.seen.remove(&key);
            }
        }
    }

    /// Signed out: another account's covers should not linger in memory.
    pub fn clear(&mut self) {
        self.handles.clear();
        self.waiting.clear();
        self.seen.clear();
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

static HTTP: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .ok()
});

/// Reads a cover from disk, or downloads it and writes it there.
pub async fn fetch(url: String, dir: Option<PathBuf>, key: ArtKey) -> Option<Vec<u8>> {
    let path = dir.as_ref().map(|dir| dir.join(key.file_name()));
    if let Some(path) = path.clone() {
        let cached = tokio::task::spawn_blocking(move || std::fs::read(path).ok())
            .await
            .ok()
            .flatten();
        if let Some(bytes) = cached.filter(|b| !b.is_empty()) {
            return Some(bytes);
        }
    }
    let client = HTTP.as_ref()?;
    let response = client.get(&url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let is_image = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v.starts_with("image/"));
    if !is_image {
        return None;
    }
    let bytes = response.bytes().await.ok()?.to_vec();
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return None;
    }
    if let (Some(path), Some(dir)) = (path, dir) {
        let copy = bytes.clone();
        let _ = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            let temporary = path.with_extension("part");
            std::fs::write(&temporary, copy)?;
            std::fs::rename(temporary, path)
        })
        .await;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn key(n: usize) -> ArtKey {
        ArtKey::new(format!("item{n}"), "tag")
    }

    #[test]
    fn a_file_name_cannot_escape_the_directory() {
        let name = ArtKey::new("../../etc/passwd", "a/b").file_name();
        assert!(!name.contains('/') && !name.contains(".."), "{name}");
    }

    #[test]
    fn downloads_are_queued_and_limited() {
        let mut artwork = Artwork::new(None);
        artwork.want((0..10).map(key));
        artwork.want([key(0)]);
        let first = artwork.next_batch();
        assert_eq!(first.len(), CONCURRENT);
        assert!(artwork.next_batch().is_empty());
        artwork.arrived(first[0].clone(), Some(vec![1, 2, 3]));
        assert_eq!(artwork.next_batch().len(), 1);
        assert!(artwork.get(&first[0]).is_some());
    }

    #[test]
    fn a_failed_cover_is_not_asked_for_again() {
        let mut artwork = Artwork::new(None);
        artwork.want([key(1)]);
        let batch = artwork.next_batch();
        artwork.arrived(batch[0].clone(), None);
        artwork.want([key(1)]);
        assert!(artwork.next_batch().is_empty());
        assert!(artwork.get(&key(1)).is_none());
    }

    #[test]
    fn memory_stays_bounded() {
        let mut artwork = Artwork::new(None);
        for n in 0..(MEMORY_CAPACITY + 50) {
            artwork.in_flight += 1;
            artwork.arrived(key(n), Some(vec![0]));
        }
        assert!(artwork.len() <= MEMORY_CAPACITY);
        // The newest survive.
        assert!(artwork.get(&key(MEMORY_CAPACITY + 49)).is_some());
        assert!(artwork.get(&key(0)).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cover_is_downloaded_once_then_read_from_disk() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/a1/Images/Primary"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(vec![0xff, 0xd8, 0xff]),
            )
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/Items/a1/Images/Primary?tag=t", server.uri());
        let key = ArtKey::new("a1", "t");
        let first = fetch(url.clone(), Some(dir.path().to_owned()), key.clone()).await;
        assert_eq!(first.as_deref(), Some(&[0xff, 0xd8, 0xff][..]));
        let second = fetch(url, Some(dir.path().to_owned()), key).await;
        assert_eq!(second, first);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_that_is_not_an_image_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<html>"),
            )
            .mount(&server)
            .await;
        let url = format!("{}/Items/a1/Images/Primary", server.uri());
        assert_eq!(fetch(url, None, ArtKey::new("a1", "t")).await, None);
    }
}
