//! Motion covers on disk: each album's video, read once from the server with
//! the token and kept a week in the cache directory, named by the album.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aoide_melete::MeleteClient;

/// How long a video on disk is played before it is read again.
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What looking for an album's motion cover found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// A video on disk to play.
    File(PathBuf),
    /// The album has none, or none this app plays: not asked again this run.
    Nothing,
    /// The read failed (the network, the disk): asked again next time.
    Failed,
}

/// The containers GStreamer is given: MP4 (an `ftyp` box) and WebM (an
/// EBML header). Anything else, such as an error page, is never kept.
pub fn is_video(bytes: &[u8]) -> bool {
    let mp4 = bytes.get(4..8) == Some(b"ftyp".as_slice());
    let webm = bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]);
    mp4 || webm
}

/// The file an album's video is kept in: a name that cannot leave the
/// directory, whatever the server sent as an id. `None` for an id with
/// nothing to name a file by.
pub fn file_for(dir: &Path, album: &str) -> Option<PathBuf> {
    let name: String = album
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect();
    (!name.is_empty()).then(|| dir.join(format!("{name}.video")))
}

/// A file younger than [`KEEP_FOR`], with something in it.
fn fresh(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| {
        meta.len() > 0
            && meta
                .modified()
                .ok()
                .and_then(|made| made.elapsed().ok())
                .is_some_and(|age| age < KEEP_FOR)
    })
}

/// The album's motion cover as a file to play. `path` is the one its album
/// gave, when the caller has it; otherwise the album is asked for it, even
/// with a copy kept here, so a motion cover the server took away stops
/// playing (Melete 0.5.0 deleted the ones it made). A kept copy plays without
/// a new download, and when the server can't be reached.
pub async fn find(
    client: MeleteClient,
    dir: PathBuf,
    album: String,
    path: Option<String>,
) -> Found {
    let Some(file) = file_for(&dir, &album) else {
        return Found::Nothing;
    };
    let kept = file.clone();
    let is_kept = tokio::task::spawn_blocking(move || fresh(&kept))
        .await
        .unwrap_or(false);
    let path = match path {
        Some(path) => path,
        None => match client.album(&album).await {
            Ok((item, _)) => match item.motion_cover {
                Some(path) => path,
                None => {
                    if is_kept {
                        let gone = file.clone();
                        let _ =
                            tokio::task::spawn_blocking(move || std::fs::remove_file(gone)).await;
                    }
                    return Found::Nothing;
                }
            },
            Err(_) if is_kept => return Found::File(file),
            Err(_) => return Found::Failed,
        },
    };
    if is_kept {
        return Found::File(file);
    }
    let bytes = match client.motion_cover(&path).await {
        Ok(Some(bytes)) if is_video(&bytes) => bytes,
        Ok(_) => return Found::Nothing,
        Err(_) => return Found::Failed,
    };
    keep(dir, file, bytes).await
}

/// Writes the video through a temporary file, so a half-written one is
/// never played.
async fn keep(dir: PathBuf, file: PathBuf, bytes: Vec<u8>) -> Found {
    let written = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        let temporary = file.with_extension("part");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, &file).map(|()| file)
    })
    .await;
    match written {
        Ok(Ok(file)) => Found::File(file),
        _ => Found::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MP4: &[u8] = b"\0\0\0\x18ftypisom-a-loop";

    fn client(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "test-device"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    fn album_json(motion: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "id": 7, "title": "Low Tide", "artist_id": 1, "artist": "The Quiet Engines",
            "year": 2019, "tracks": 0, "duration_seconds": null,
            "cover": "/api/v1/music/albums/7/cover", "motion_cover": motion,
            "genres": [], "liked": false, "track_list": [],
        })
    }

    #[test]
    fn only_mp4_and_webm_are_videos() {
        assert!(is_video(MP4));
        assert!(is_video(&[0x1a, 0x45, 0xdf, 0xa3, 0x01]));
        assert!(!is_video(b"<html>not a video</html>"));
        assert!(!is_video(b"ftyp"));
    }

    #[test]
    fn a_file_name_cannot_escape_the_directory() {
        let dir = Path::new("/cache/motion");
        let file = file_for(dir, "../../etc/passwd").unwrap();
        assert_eq!(file, dir.join("etcpasswd.video"));
        assert_eq!(file_for(dir, "../"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_video_is_read_once_with_the_token_then_played_from_disk() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/7"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(album_json(Some("/api/v1/music/albums/7/motion"))),
            )
            // Asked each time: whether it still has one.
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/7/motion"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(MP4))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ask = || find(client(&server), dir.path().to_owned(), "7".into(), None);
        let file = dir.path().join("7.video");
        assert_eq!(ask().await, Found::File(file.clone()));
        assert_eq!(std::fs::read(&file).unwrap(), MP4);
        assert_eq!(ask().await, Found::File(file));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_video_or_one_that_isnt_a_video_is_nothing_and_a_failure_is_not() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(album_json(None)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/8/motion"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/9/motion"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ask = |album: &str, path: Option<&str>| {
            find(
                client(&server),
                dir.path().to_owned(),
                album.into(),
                path.map(str::to_owned),
            )
        };
        assert_eq!(ask("7", None).await, Found::Nothing);
        let html = ask("8", Some("/api/v1/music/albums/8/motion")).await;
        assert_eq!(html, Found::Nothing);
        let down = ask("9", Some("/api/v1/music/albums/9/motion")).await;
        assert_eq!(down, Found::Failed);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_kept_video_the_server_took_away_is_deleted_and_one_kept_plays_offline() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(album_json(None)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums/9"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("7.video");
        let offline = dir.path().join("9.video");
        std::fs::write(&gone, MP4).unwrap();
        std::fs::write(&offline, MP4).unwrap();
        let ask = |album: &str| find(client(&server), dir.path().to_owned(), album.into(), None);
        assert_eq!(ask("7").await, Found::Nothing);
        assert!(!gone.exists());
        assert_eq!(ask("9").await, Found::File(offline));
    }

    #[test]
    fn a_video_older_than_a_week_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("7.video");
        std::fs::write(&file, MP4).unwrap();
        assert!(fresh(&file));
        let old = std::time::SystemTime::now() - KEEP_FOR - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(!fresh(&file));
    }
}
