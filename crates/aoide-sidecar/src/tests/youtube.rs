//! A YouTube playlist export read as songs: the iOS `YouTubeVideoTests.swift` and
//! the YouTube cases of its `ImportTests.swift`.

use crate::import::{parse_duration, youtube};
use crate::{ImportedTrack, parse_playlist_csv};

#[test]
fn a_youtube_export_reads_the_artist_from_the_title_or_channel_and_a_tracks_length() {
    // The columns and values a YouTube playlist exporter writes, with a
    // description over two lines.
    let csv = "\u{FEFF}Title,Description,Channel name,Duration in seconds,Duration in timestamp,Duration,Video url\n\
        Radiohead - Karma Police (Official Music Video),\"Taken from OK Computer\nOut now\",RadioheadVEVO,264,4:24,\"4 Minutes, 24 Seconds\",https://www.youtube.com/watch?v=1\n\
        Paranoid Android,,Radiohead - Topic,,6:27,,https://www.youtube.com/watch?v=2\n\
        Creep [Lyrics],,RadioheadOfficialVEVO,,,\"3 Minutes, 58 Seconds\",https://www.youtube.com/watch?v=3\n";
    let playlist = parse_playlist_csv(csv, "x").unwrap();
    assert_eq!(
        playlist.tracks,
        [
            // A video's length isn't the song's; a YouTube Music track's is.
            ImportedTrack::new("Karma Police", &["Radiohead"], None),
            ImportedTrack::new("Paranoid Android", &["Radiohead"], Some(387_000)),
            ImportedTrack::new("Creep", &["Radiohead"], None),
        ]
    );
}

#[test]
fn an_artist_column_wins_over_the_channel() {
    let csv = "Title,Artist,Channel name\nKarma Police - Live,Radiohead,Someone Else\n";
    let playlist = parse_playlist_csv(csv, "x").unwrap();
    assert_eq!(
        playlist.tracks,
        [ImportedTrack::new(
            "Karma Police - Live",
            &["Radiohead"],
            None
        )]
    );
}

#[test]
fn lengths_are_milliseconds_minutes_and_seconds_or_hours_too() {
    let cases = [
        ("236000", Some(236_000)),
        ("4:24", Some(264_000)),
        ("1:02:03", Some(3_723_000)),
        ("4:", None),
        ("4 Minutes", None),
    ];
    for (value, expected) in cases {
        assert_eq!(parse_duration(value), expected, "{value}");
    }
}

#[test]
fn a_video_title_and_channel_become_a_song_and_its_artists() {
    let cases: [(&str, &str, &str, &[&str]); 17] = [
        (
            "LISA - Tarantino (Official Music Video)",
            "LISAOfficialVEVO",
            "Tarantino",
            &["LISA"],
        ),
        (
            "Shakira, Burna Boy - Dai Dai (Official Video)",
            "shakiraVEVO",
            "Dai Dai",
            &["Shakira", "Burna Boy"],
        ),
        (
            "ATEEZ(에이티즈) - 'Drip' Official MV",
            "KQ ENTERTAINMENT",
            "Drip",
            &["ATEEZ"],
        ),
        (
            "Alfaaz | Hamza Malik x Zain Zohaib | Official Visualiser",
            "Hamza Malik",
            "Alfaaz",
            &["Hamza Malik"],
        ),
        (
            "Dj Goja x Jason Derulo - Mi Chico (Official Music Video)",
            "x",
            "Mi Chico",
            &["Dj Goja", "Jason Derulo"],
        ),
        (
            "Creep - Remastered",
            "Radiohead - Topic",
            "Creep - Remastered",
            &["Radiohead"],
        ),
        (
            "Crazy in Love (feat. Jay-Z)",
            "Beyoncé - Topic",
            "Crazy in Love (feat. Jay-Z)",
            &["Beyoncé"],
        ),
        ("Music", "Madonna", "Music", &["Madonna"]),
        ("Official Video", "Someone", "Official Video", &["Someone"]),
        ("Untitled", "", "Untitled", &[]),
        (
            "Barsaat - Banjaare (Official Video)",
            "Banjaare",
            "Barsaat",
            &["Banjaare"],
        ),
        (
            "Ala Bolelo - Lyric Video | Jailer 2 | Anirudh",
            "Sun TV",
            "Ala Bolelo",
            &["Sun TV"],
        ),
        (
            "KATSEYE (캣츠아이) 'Hootie Frutti' Official MV",
            "HYBE LABELS",
            "Hootie Frutti",
            &["KATSEYE"],
        ),
        (
            "ILLIT (아일릿) '체포하겠어 (You’re Under Arrest)’ Official MV",
            "HYBE LABELS",
            "체포하겠어 (You’re Under Arrest)",
            &["ILLIT"],
        ),
        (
            "Felipe & Rodrigo -  Pós (Ao Vivo) #NoSentimento",
            "Felipe e Rodrigo",
            "Pós (Ao Vivo)",
            &["Felipe & Rodrigo"],
        ),
        ("#1 Crush", "Garbage", "#1 Crush", &["Garbage"]),
        (
            "Don't Stop Believin'",
            "JourneyVEVO",
            "Don't Stop Believin'",
            &["Journey"],
        ),
    ];
    for (title, channel, song, artists) in cases {
        let found = youtube::song(title, channel);
        assert_eq!(found.title, song, "{title}");
        assert_eq!(found.artists, artists, "{title}");
        assert_eq!(
            found.is_music_track,
            channel.ends_with(" - Topic"),
            "{title}"
        );
    }
}
