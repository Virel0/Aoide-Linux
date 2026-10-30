//! Following synced lyrics: which line is being sung, with the listener's
//! timing nudge applied, and where a click on a line seeks to.

use std::time::Duration;

use aoide_melete::{LyricLine, Lyrics};

/// The time in the lyrics that matches `position_ms` in the track. A positive
/// offset shows the words later, as the phone's slider does: the lyrics read
/// `position - offset`, never before zero.
pub fn lyric_time(position_ms: u64, offset_ms: i32) -> Duration {
    let ms = i64::try_from(position_ms).unwrap_or(i64::MAX) - i64::from(offset_ms);
    Duration::from_millis(u64::try_from(ms.max(0)).unwrap_or(0))
}

/// The line being sung, or `None` before the first and for unsynced lyrics.
pub fn current_line(lyrics: &Lyrics, position_ms: u64, offset_ms: i32) -> Option<usize> {
    if !lyrics.is_synced() {
        return None;
    }
    lyrics.line_at(lyric_time(position_ms, offset_ms))
}

/// The track position a click on `line` seeks to: the moment that line is
/// highlighted, under the same offset.
pub fn seek_target(line: &LyricLine, offset_ms: i32) -> Option<u64> {
    let start = i64::try_from(line.start?.as_millis()).unwrap_or(i64::MAX);
    u64::try_from((start + i64::from(offset_ms)).max(0)).ok()
}

/// The setting's label, as on the phone.
pub fn offset_label(offset_ms: i32) -> String {
    match offset_ms {
        0 => "In sync".to_owned(),
        ms if ms > 0 => format!("{ms} ms later"),
        ms => format!("{} ms earlier", -ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, start_ms: Option<u64>) -> LyricLine {
        LyricLine {
            text: text.to_owned(),
            start: start_ms.map(Duration::from_millis),
            words: Vec::new(),
        }
    }

    fn synced() -> Lyrics {
        Lyrics {
            lines: vec![
                line("one", Some(1_000)),
                line("two", Some(5_000)),
                line("three", Some(9_000)),
            ],
        }
    }

    #[test]
    fn the_line_follows_the_position() {
        let lyrics = synced();
        assert_eq!(current_line(&lyrics, 500, 0), None);
        assert_eq!(current_line(&lyrics, 1_000, 0), Some(0));
        assert_eq!(current_line(&lyrics, 4_999, 0), Some(0));
        assert_eq!(current_line(&lyrics, 5_000, 0), Some(1));
        assert_eq!(current_line(&lyrics, 60_000, 0), Some(2));
    }

    #[test]
    fn a_positive_offset_shows_the_words_later() {
        let lyrics = synced();
        // At 5.2 s with the words 500 ms later, the lyrics read 4.7 s: still line one.
        assert_eq!(current_line(&lyrics, 5_200, 500), Some(0));
        assert_eq!(current_line(&lyrics, 5_500, 500), Some(1));
    }

    #[test]
    fn a_negative_offset_shows_them_earlier() {
        let lyrics = synced();
        assert_eq!(current_line(&lyrics, 4_600, -500), Some(1));
        assert_eq!(current_line(&lyrics, 600, -500), Some(0));
    }

    #[test]
    fn the_offset_never_reads_before_the_start() {
        assert_eq!(lyric_time(200, 1_500), Duration::ZERO);
    }

    #[test]
    fn unsynced_lyrics_have_no_current_line() {
        let lyrics = Lyrics {
            lines: vec![line("just text", None), line("more", None)],
        };
        assert_eq!(current_line(&lyrics, 10_000, 0), None);
    }

    #[test]
    fn a_click_seeks_to_where_the_line_lights_up() {
        let lyrics = synced();
        let target = seek_target(&lyrics.lines[1], 500).unwrap();
        assert_eq!(target, 5_500);
        assert_eq!(current_line(&lyrics, target, 500), Some(1));
        assert_eq!(seek_target(&line("x", Some(200)), -500), Some(0));
        assert_eq!(seek_target(&line("x", None), 0), None);
    }

    #[test]
    fn the_label_reads_like_the_phone() {
        assert_eq!(offset_label(0), "In sync");
        assert_eq!(offset_label(250), "250 ms later");
        assert_eq!(offset_label(-100), "100 ms earlier");
    }
}
