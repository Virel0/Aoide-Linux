//! Track id <-> MPRIS track id.
//!
//! MPRIS track ids are D-Bus object paths, whose elements may only hold
//! `[A-Za-z0-9_]`. Melete ids are integers today, but they come from the
//! server, so any string is mapped safely: ASCII letters and digits are kept,
//! every other byte (including `_`, the escape character itself) becomes `_`
//! and two upper-case hex digits. A leading `t` keeps the element non-empty for
//! an empty id. Because the escape character is always escaped, the mapping is
//! injective and can be reversed.

/// Object path prefix for track ids. `/org/mpris` is reserved by the spec, so
/// tracks live under the app's own name.
pub const TRACK_PATH_PREFIX: &str = "/io/github/Virel0/Aoide/Track/";

/// The path MPRIS uses when no track is loaded.
pub const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// The MPRIS track id (a D-Bus object path) for a track id.
pub fn track_object_path(track_id: &str) -> String {
    let mut path = String::with_capacity(TRACK_PATH_PREFIX.len() + 1 + track_id.len());
    path.push_str(TRACK_PATH_PREFIX);
    path.push('t');
    for &byte in track_id.as_bytes() {
        if byte.is_ascii_alphanumeric() {
            path.push(char::from(byte));
        } else {
            path.push('_');
            path.push(char::from(HEX[usize::from(byte >> 4)]));
            path.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    path
}

/// The track id a track path was made from, or `None` if the path did
/// not come from [`track_object_path`].
pub fn id_from_track_path(path: &str) -> Option<String> {
    let encoded = path.strip_prefix(TRACK_PATH_PREFIX)?.strip_prefix('t')?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut input = encoded.bytes();
    while let Some(byte) = input.next() {
        if byte.is_ascii_alphanumeric() {
            bytes.push(byte);
        } else if byte == b'_' {
            let high = hex_value(input.next()?)?;
            let low = hex_value(input.next()?)?;
            let decoded = (high << 4) | low;
            // An escaped letter or digit was never produced by the encoder;
            // accepting it would give two paths for one id.
            if decoded.is_ascii_alphanumeric() {
                return None;
            }
            bytes.push(decoded);
        } else {
            return None;
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Whether `path` is a valid D-Bus object path, per the D-Bus specification:
/// `/`, or `/`-separated non-empty elements of `[A-Za-z0-9_]`, with no
/// trailing `/`. Used to check the encoder.
#[cfg(test)]
fn is_valid_object_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    rest.split('/').all(|element| {
        !element.is_empty()
            && element
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_normal_track_id_is_kept_readable() {
        let id = "5f1c0a3e9b2d4c8f8a6e1b7d3c9f0e2a";
        assert_eq!(track_object_path(id), format!("{TRACK_PATH_PREFIX}t{id}"));
    }

    #[test]
    fn the_mapping_is_stable() {
        let id = "a-b_c/d é";
        assert_eq!(track_object_path(id), track_object_path(id));
        assert_eq!(
            track_object_path(id),
            "/io/github/Virel0/Aoide/Track/ta_2Db_5Fc_2Fd_20_C3_A9"
        );
    }

    #[test]
    fn hostile_ids_give_valid_object_paths() {
        let ids = [
            "",
            "_",
            "/",
            "//",
            "..",
            "a/../b",
            "\0",
            "with space",
            "dash-and.dot",
            "ünïcödé",
            "🎵",
            "_5F",
            "t",
            "\u{FFFF}",
        ];
        for id in ids {
            let path = track_object_path(id);
            assert!(is_valid_object_path(&path), "{id:?} -> {path:?}");
            assert!(path.starts_with(TRACK_PATH_PREFIX));
        }
    }

    #[test]
    fn every_id_round_trips() {
        let ids = [
            "", "_", "abc", "ABC", "_5F", "a_b", "x/y", "é", "🎵", "\0\x7f",
        ];
        for id in ids {
            let path = track_object_path(id);
            assert_eq!(id_from_track_path(&path).as_deref(), Some(id), "{path}");
        }
    }

    #[test]
    fn distinct_ids_never_share_a_path() {
        // Every string of up to two characters from an alphabet that mixes
        // kept characters, the escape character, look-alikes of escapes and
        // multi-byte characters.
        let alphabet = ["", "a", "A", "0", "F", "_", "5", "-", "/", "é", "🎵", "t"];
        let mut seen: HashMap<String, String> = HashMap::new();
        for x in alphabet {
            for y in alphabet {
                for z in alphabet {
                    let id = format!("{x}{y}{z}");
                    let path = track_object_path(&id);
                    if let Some(other) = seen.insert(path.clone(), id.clone()) {
                        assert_eq!(other, id, "collision on {path}");
                    }
                    assert_eq!(id_from_track_path(&path).as_deref(), Some(id.as_str()));
                }
            }
        }
    }

    #[test]
    fn ids_that_differ_only_in_case_stay_distinct() {
        assert_ne!(track_object_path("abc"), track_object_path("ABC"));
    }

    #[test]
    fn foreign_paths_do_not_decode() {
        let paths = [
            NO_TRACK,
            "/io/github/Virel0/Aoide/Track/",
            "/io/github/Virel0/Aoide/Track/abc",
            "/io/github/Virel0/Aoide/Track/t_",
            "/io/github/Virel0/Aoide/Track/t_5",
            "/io/github/Virel0/Aoide/Track/t_zz",
            "/io/github/Virel0/Aoide/Track/t_5f",
            // An escaped letter is not what the encoder writes.
            "/io/github/Virel0/Aoide/Track/t_61",
            // Escaped bytes that are not UTF-8.
            "/io/github/Virel0/Aoide/Track/t_FF",
            "/io/github/Virel0/Aoide/Track/ta/b",
        ];
        for path in paths {
            assert_eq!(id_from_track_path(path), None, "{path}");
        }
    }

    #[test]
    fn object_path_validator_follows_the_spec() {
        for good in ["/", "/a", "/a/b", "/A_1/b2", NO_TRACK] {
            assert!(is_valid_object_path(good), "{good}");
        }
        for bad in ["", "a", "/a/", "//", "/a//b", "/a-b", "/a.b", "/é"] {
            assert!(!is_valid_object_path(bad), "{bad}");
        }
    }
}
