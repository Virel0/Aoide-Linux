//! A YouTube video read as a song: an export of a YouTube playlist has the video's
//! title and channel, not a song title and artist.
//!
//! - A YouTube Music track's channel is "Artist - Topic", and its title is the song.
//! - A music video's title is usually "Artist - Song (Official Video)", or
//!   "Song - Artist" on the artist's own channel.
//! - A K-pop video's is often "Artist 'Song' Official MV".
//! - Otherwise the channel stands for the artist, less "VEVO" and "Official".
//!
//! Only a YouTube Music track's length is the song's: a video's runs over with its
//! intro and ending, and would rule out the very recording it shows.
//!
//! What video sites add to a title (Official Video, Lyric Video, M/V, hashtags, a
//! " | " and what follows) is taken off first, so Melete looks the song up by its
//! own name. The phone's `YouTubeVideo.swift` follows the same rules.

const BRACKETS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('【', '】'), ('「', '」')];

/// Words that mark a bracketed part as the video's, not the song's.
const VIDEO_WORDS: [&str; 15] = [
    "official",
    "oficial",
    "video",
    "videoclip",
    "audio",
    "lyric",
    "lyrics",
    "lyrical",
    "visualizer",
    "visualiser",
    "visual",
    "mv",
    "hd",
    "4k",
    "hq",
];

/// What a title may end with that names the video, longest first.
const VIDEO_ENDINGS: [&str; 13] = [
    "official music video",
    "official lyric video",
    "official visualizer",
    "official visualiser",
    "official video",
    "official audio",
    "official mv",
    "official m/v",
    "music video",
    "lyric video",
    "video oficial",
    "m/v",
    "mv",
];

/// A video read as a song.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Song {
    pub title: String,
    pub artists: Vec<String>,
    /// A YouTube Music track, whose length is the song's.
    pub is_music_track: bool,
}

/// The song a video's `title` and `channel` stand for.
pub(crate) fn song(title: &str, channel: &str) -> Song {
    if let Some(artist) = topic_artist(channel) {
        return Song {
            title: song_title(title),
            artists: vec![artist],
            is_music_track: true,
        };
    }
    let (title, artists) = video_song(title, channel);
    Song {
        title,
        artists,
        is_music_track: false,
    }
}

fn video_song(title: &str, channel: &str) -> (String, Vec<String>) {
    let named = channel_artist(channel);
    let cleaned = song_title(title);
    if let Some((left, right)) = artist_dash(&cleaned) {
        let song = song_title(right);
        // "Song - Artist" on the artist's own channel.
        if !named.is_empty() && plain(&song) == plain(&named) && plain(left) != plain(&named) {
            return (song_title(left), vec![named]);
        }
        return (song, artists(&without_groups(left)));
    }
    if let Some((left, song)) = quoted_song(&cleaned) {
        return (song.to_owned(), artists(&without_groups(left)));
    }
    let artists = if named.is_empty() {
        Vec::new()
    } else {
        vec![named]
    };
    (cleaned, artists)
}

/// "Artist 'Song'": a quoted song at the end, after a space.
fn quoted_song(title: &str) -> Option<(&str, &str)> {
    const OPENERS: [char; 4] = ['\'', '‘', '"', '“'];
    const CLOSERS: [char; 4] = ['\'', '’', '"', '”'];
    let last = title.chars().next_back()?;
    if !CLOSERS.contains(&last) {
        return None;
    }
    let body = &title[..title.len() - last.len_utf8()];
    let (at, opener) = body
        .char_indices()
        .rev()
        .find(|&(at, c)| OPENERS.contains(&c) && body[..at].ends_with(' '))?;
    let left = body[..at].trim();
    let song = body[at + opener.len_utf8()..].trim();
    (!left.is_empty() && !song.is_empty()).then_some((left, song))
}

/// Letters and digits only, lower-cased: for telling two names apart.
fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// The artist of a YouTube Music "Artist - Topic" channel.
fn topic_artist(channel: &str) -> Option<String> {
    const SUFFIX: &str = " - topic";
    let lower = channel.to_lowercase();
    if !lower.ends_with(SUFFIX) || lower.len() != channel.len() {
        return None;
    }
    let artist = channel[..channel.len() - SUFFIX.len()].trim();
    (!artist.is_empty()).then(|| artist.to_owned())
}

/// "Artist - Song" split at its first dash, both sides there.
fn artist_dash(title: &str) -> Option<(&str, &str)> {
    [" - ", " – ", " — "].iter().find_map(|dash| {
        let (left, right) = title.split_once(dash)?;
        let (left, right) = (left.trim(), right.trim());
        (!left.is_empty() && !right.is_empty()).then_some((left, right))
    })
}

/// The channel's name without "VEVO" or "Official" stuck to its end.
fn channel_artist(channel: &str) -> String {
    let mut name = channel.trim();
    for suffix in ["VEVO", "Vevo", "Official"] {
        if let Some(rest) = name.strip_suffix(suffix).filter(|rest| !rest.is_empty()) {
            name = rest.trim();
        }
    }
    name.to_owned()
}

/// Several artists as a video names them: "A, B", "A x B", "A feat. B".
fn artists(text: &str) -> Vec<String> {
    let mut parts = vec![text.to_owned()];
    for separator in [",", " x ", " X ", " feat. ", " ft. ", " Feat. ", " Ft. "] {
        parts = parts
            .iter()
            .flat_map(|part| part.split(separator).map(str::to_owned))
            .collect();
    }
    parts
        .iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Bracketed parts taken out: "ILLIT (아일릿)" is ILLIT.
fn without_groups(text: &str) -> String {
    let mut result = text.to_owned();
    for (open, close) in BRACKETS {
        result = removing_groups(&result, open, close, |_| true);
    }
    let kept = tidy(&result);
    if kept.is_empty() {
        text.to_owned()
    } else {
        kept
    }
}

/// `text` without the bracketed parts whose inside (lower-cased) `drop` says to drop.
fn removing_groups(text: &str, open: char, close: char, drop: impl Fn(&str) -> bool) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let Some(end) = rest[start..].find(close).map(|offset| start + offset) else {
            break;
        };
        result.push_str(&rest[..start]);
        let inside = rest[start + open.len_utf8()..end].to_lowercase();
        if !drop(&inside) {
            result.push_str(&rest[start..end + close.len_utf8()]);
        }
        rest = &rest[end + close.len_utf8()..];
    }
    result + rest
}

/// Whether a bracketed part names the video: (Official Video), [M/V], (Lyrics).
fn is_video_part(inside: &str) -> bool {
    inside.contains("m/v")
        || inside
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| VIDEO_WORDS.contains(&word))
}

/// The song's title out of a video's: no " | " and what follows, no hashtags,
/// no bracketed video words, no "Official Video" at the end, no quotes around it.
pub(crate) fn song_title(title: &str) -> String {
    let first = title.split(" | ").next().unwrap_or(title);
    // "#NoSentimento" goes; "#1 Crush" stays.
    let mut song = first
        .split(' ')
        .filter(|word| {
            !word
                .strip_prefix('#')
                .is_some_and(|tag| tag.starts_with(char::is_alphabetic))
        })
        .collect::<Vec<_>>()
        .join(" ");
    for (open, close) in BRACKETS {
        song = removing_groups(&song, open, close, is_video_part);
    }
    song = without_video_ending(&song);
    let trimmed = song.trim_matches(|c| matches!(c, ' ' | '-' | ',' | ':'));
    let kept = tidy(unquoted(trimmed));
    if kept.is_empty() {
        title.trim().to_owned()
    } else {
        kept
    }
}

fn without_video_ending(text: &str) -> String {
    let mut result = text.trim().to_owned();
    loop {
        let lower = result.to_lowercase();
        let Some(ending) = VIDEO_ENDINGS
            .iter()
            .find(|ending| lower.ends_with(&format!(" {ending}")))
        else {
            return result;
        };
        // Lower-casing can change a string's length; only cut where it didn't.
        if lower.len() != result.len() {
            return result;
        }
        result = result[..result.len() - ending.len()].trim().to_owned();
    }
}

/// 'NEON' or “NEON” is NEON.
fn unquoted(text: &str) -> &str {
    const QUOTES: [char; 6] = ['\'', '"', '‘', '’', '“', '”'];
    let mut chars = text.chars();
    match (chars.next(), chars.next_back()) {
        (Some(first), Some(last))
            if text.chars().count() > 2 && QUOTES.contains(&first) && QUOTES.contains(&last) =>
        {
            text[first.len_utf8()..text.len() - last.len_utf8()].trim()
        }
        _ => text,
    }
}

/// Single spaces between words, none at the ends.
fn tidy(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
