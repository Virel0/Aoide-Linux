//! The MPRIS `Metadata` map, built as plain data so it can be tested without a
//! bus. The Linux side turns each [`Entry`] into a D-Bus variant.

use std::time::Duration;

use crate::track_id::track_object_path;
use crate::types::Metadata;

/// One value of the `a{sv}` metadata map.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// D-Bus type `o`.
    ObjectPath(String),
    /// D-Bus type `s`.
    Str(String),
    /// D-Bus type `as`.
    StrList(Vec<String>),
    /// D-Bus type `x`.
    I64(i64),
    /// D-Bus type `i`.
    I32(i32),
}

/// The metadata map for a track, keys as the MPRIS and xesam specs name them.
/// Empty fields are left out rather than sent empty.
pub fn entries(metadata: &Metadata) -> Vec<(&'static str, Entry)> {
    let mut out = vec![(
        "mpris:trackid",
        Entry::ObjectPath(track_object_path(&metadata.jellyfin_id)),
    )];
    if let Some(length) = metadata.length {
        out.push(("mpris:length", Entry::I64(micros(length))));
    }
    if let Some(url) = metadata.art_url.as_deref().and_then(safe_art_url) {
        out.push(("mpris:artUrl", Entry::Str(clean(url))));
    }
    if !metadata.title.is_empty() {
        out.push(("xesam:title", Entry::Str(clean(&metadata.title))));
    }
    if !metadata.artists.is_empty() {
        out.push(("xesam:artist", Entry::StrList(clean_all(&metadata.artists))));
    }
    if let Some(album) = metadata.album.as_deref().filter(|a| !a.is_empty()) {
        out.push(("xesam:album", Entry::Str(clean(album))));
    }
    if !metadata.album_artists.is_empty() {
        out.push((
            "xesam:albumArtist",
            Entry::StrList(clean_all(&metadata.album_artists)),
        ));
    }
    if let Some(number) = metadata.track_number.and_then(|n| i32::try_from(n).ok()) {
        out.push(("xesam:trackNumber", Entry::I32(number)));
    }
    out
}

/// A duration in MPRIS units (microseconds), saturating.
pub fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

/// D-Bus strings may not contain NUL; a message that has one is invalid and the
/// bus daemon drops the connection that sent it. Server-provided text is
/// cleaned rather than trusted.
fn clean(text: &str) -> String {
    text.replace('\0', "")
}

fn clean_all(texts: &[String]) -> Vec<String> {
    texts.iter().map(|t| clean(t)).collect()
}

/// Query parameter names that carry a Jellyfin (or Emby) credential.
const CREDENTIAL_PARAMS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "access_token",
    "x-emby-token",
    "x-mediabrowser-token",
];

/// The art URL if it may be published: `file://`, or `https://` without
/// credentials in it. Everything on the session bus can read MPRIS metadata.
pub fn safe_art_url(url: &str) -> Option<&str> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("file://") {
        return Some(url);
    }
    let rest = lower.strip_prefix("https://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..authority_end].contains('@') || rest[..authority_end].is_empty() {
        return None;
    }
    let query = rest[authority_end..]
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or(""))
        .unwrap_or("");
    let has_credential = query.split('&').any(|pair| {
        let name = pair.split('=').next().unwrap_or("");
        CREDENTIAL_PARAMS.contains(&name)
    });
    if has_credential { None } else { Some(url) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Metadata {
        Metadata {
            jellyfin_id: "0123abcd".into(),
            title: "Song".into(),
            artists: vec!["One".into(), "Two".into()],
            album: Some("Album".into()),
            album_artists: vec!["One".into()],
            length: Some(Duration::from_millis(215_250)),
            art_url: Some("https://jellyfin.example.org/Items/0123abcd/Images/Primary".into()),
            track_number: Some(7),
        }
    }

    fn get<'a>(entries: &'a [(&'static str, Entry)], key: &str) -> Option<&'a Entry> {
        entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    #[test]
    fn a_full_track_fills_every_key() {
        let e = entries(&track());
        assert_eq!(
            get(&e, "mpris:trackid"),
            Some(&Entry::ObjectPath(
                "/io/github/Virel0/Aoide/Track/t0123abcd".into()
            ))
        );
        assert_eq!(get(&e, "mpris:length"), Some(&Entry::I64(215_250_000)));
        assert_eq!(
            get(&e, "mpris:artUrl"),
            Some(&Entry::Str(
                "https://jellyfin.example.org/Items/0123abcd/Images/Primary".into()
            ))
        );
        assert_eq!(get(&e, "xesam:title"), Some(&Entry::Str("Song".into())));
        assert_eq!(
            get(&e, "xesam:artist"),
            Some(&Entry::StrList(vec!["One".into(), "Two".into()]))
        );
        assert_eq!(get(&e, "xesam:album"), Some(&Entry::Str("Album".into())));
        assert_eq!(
            get(&e, "xesam:albumArtist"),
            Some(&Entry::StrList(vec!["One".into()]))
        );
        assert_eq!(get(&e, "xesam:trackNumber"), Some(&Entry::I32(7)));
    }

    #[test]
    fn empty_fields_are_left_out_but_the_track_id_never_is() {
        let e = entries(&Metadata {
            jellyfin_id: "x".into(),
            ..Metadata::default()
        });
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, "mpris:trackid");
    }

    #[test]
    fn nul_characters_are_removed() {
        let mut t = track();
        t.title = "So\0ng".into();
        t.artists = vec!["O\0ne".into()];
        let e = entries(&t);
        assert_eq!(get(&e, "xesam:title"), Some(&Entry::Str("Song".into())));
        assert_eq!(
            get(&e, "xesam:artist"),
            Some(&Entry::StrList(vec!["One".into()]))
        );
    }

    #[test]
    fn a_track_number_too_big_for_i32_is_dropped() {
        let mut t = track();
        t.track_number = Some(u32::MAX);
        assert_eq!(get(&entries(&t), "xesam:trackNumber"), None);
    }

    #[test]
    fn huge_lengths_saturate() {
        assert_eq!(micros(Duration::MAX), i64::MAX);
        assert_eq!(micros(Duration::from_secs(1)), 1_000_000);
    }

    #[test]
    fn art_urls_must_be_https_or_file() {
        assert!(safe_art_url("file:///var/cache/aoide/art/abc.jpg").is_some());
        assert!(safe_art_url("https://jellyfin.example.org/Items/a/Images/Primary").is_some());
        assert!(
            safe_art_url("HTTPS://jellyfin.example.org/Items/a/Images/Primary?tag=x&maxWidth=512")
                .is_some()
        );
        for bad in [
            "http://jellyfin.example.org/Items/a/Images/Primary",
            "ftp://example.org/a.jpg",
            "data:image/png;base64,AAAA",
            "/var/cache/a.jpg",
            "https://",
            "",
        ] {
            assert_eq!(safe_art_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn art_urls_with_credentials_are_not_published() {
        for bad in [
            "https://user:secret@jellyfin.example.org/Items/a/Images/Primary",
            "https://jellyfin.example.org/Items/a/Images/Primary?api_key=x",
            "https://jellyfin.example.org/Items/a/Images/Primary?tag=1&ApiKey=x",
            "https://jellyfin.example.org/Items/a/Images/Primary?X-Emby-Token=x#frag",
            "https://jellyfin.example.org/Items/a/Images/Primary?token=x",
        ] {
            assert_eq!(safe_art_url(bad), None, "{bad}");
        }
        // A parameter that merely contains the word is fine.
        assert!(safe_art_url("https://jellyfin.example.org/a?tokenized=1").is_some());
        // An `@` in the path is not user info.
        assert!(safe_art_url("https://jellyfin.example.org/a@2x.png").is_some());
    }

    #[test]
    fn disallowed_art_urls_are_left_out_of_the_map() {
        let mut t = track();
        t.art_url = Some("http://jellyfin.example.org/a.jpg".into());
        assert_eq!(get(&entries(&t), "mpris:artUrl"), None);
    }
}
