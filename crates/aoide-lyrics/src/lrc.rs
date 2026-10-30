//! LRC and plain-text lyrics, parsed into aoide-melete's [`Lyrics`].
//!
//! LRC as found in the wild:
//!
//! - `[mm:ss.xx]text`, with one to three fraction digits, none, or a colon
//!   before them (`[mm:ss:xx]`); `[hh:mm:ss.xx]` too.
//! - Several times on one line (`[00:12.00][00:45.00]chorus`): the line is
//!   repeated at each.
//! - Word timings ("enhanced" LRC): `[00:12.00]<00:12.00>Word <00:12.50>by
//!   <00:13.00>word<00:13.40>`, a trailing time marking the last word's end.
//! - Header tags (`[ar:…]`, `[ti:…]`, `[length:…]`) are skipped, except
//!   `[offset:±ms]`: a positive offset makes the lyrics come sooner.
//!
//! A file with no timed line at all is read as plain text.

use std::time::Duration;

use aoide_melete::{LyricLine, LyricWord, Lyrics};

/// LRC (synced, with or without word timings) or, failing any timestamp, plain
/// text. Timed lines come out in time order; lines without a time in a synced
/// file (credits, stray text) are dropped. Empty timed lines are kept: they
/// mark instrumental breaks.
pub fn parse_lrc(text: &str) -> Lyrics {
    let mut offset_ms: i64 = 0;
    let mut timed: Vec<Timed> = Vec::new();

    for raw in text.trim_start_matches('\u{feff}').lines() {
        let (times, offset, body) = leading_tags(raw.trim());
        if let Some(offset) = offset {
            offset_ms = offset;
        }
        if times.is_empty() {
            continue;
        }
        let (text, words, end) = words_of(body);
        let first = times[0];
        for time in times {
            // A repeated line's words move with it.
            let shift = |t: Duration| t.saturating_add(time).saturating_sub(first);
            timed.push(Timed {
                text: text.clone(),
                start: time,
                words: words.iter().map(|(w, t)| (w.clone(), shift(*t))).collect(),
                end: end.map(shift),
            });
        }
    }

    if timed.is_empty() {
        return parse_plain(&strip_header_tags(text));
    }

    timed.sort_by_key(|line| line.start);
    let apply = |t: Duration| apply_offset(t, offset_ms);
    let starts: Vec<Duration> = timed.iter().map(|line| apply(line.start)).collect();

    let lines = timed
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let next_line = starts.get(i + 1).copied();
            let words = finish_words(&line.words, line.end, next_line, apply);
            LyricLine {
                text: line.text,
                start: Some(starts[i]),
                words,
            }
        })
        .collect();
    Lyrics { lines }
}

/// Plain text: one untimed line per line. Blank lines inside are kept (they
/// separate verses); leading and trailing ones are not.
pub fn parse_plain(text: &str) -> Lyrics {
    let all: Vec<&str> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim_end)
        .collect();
    let first = all.iter().position(|l| !l.trim().is_empty());
    let last = all.iter().rposition(|l| !l.trim().is_empty());
    let lines = match (first, last) {
        (Some(first), Some(last)) => all[first..=last]
            .iter()
            .map(|line| LyricLine {
                text: line.trim().to_owned(),
                start: None,
                words: Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    Lyrics { lines }
}

struct Timed {
    text: String,
    start: Duration,
    words: Vec<(String, Duration)>,
    /// A trailing word timestamp: when the last word ends.
    end: Option<Duration>,
}

/// The `[…]` tags at the start of a line: its times, an `[offset:…]` if there
/// is one, and the rest of the line.
fn leading_tags(mut line: &str) -> (Vec<Duration>, Option<i64>, &str) {
    let mut times = Vec::new();
    let mut offset = None;
    while let Some(rest) = line.strip_prefix('[') {
        let Some(close) = rest.find(']') else { break };
        let tag = &rest[..close];
        if let Some(time) = parse_time(tag) {
            times.push(time);
        } else if let Some((key, value)) = tag.split_once(':') {
            if key.trim().eq_ignore_ascii_case("offset") {
                offset = value.trim().trim_start_matches('+').parse::<i64>().ok();
            }
        } else {
            // `[chorus]` and the like: text, not a tag.
            break;
        }
        line = rest[close + 1..].trim_start();
    }
    (times, offset, line)
}

/// A line's body with any `<mm:ss.xx>` word times taken out: the text, the
/// timed words, and a trailing time that ends the last word.
fn words_of(body: &str) -> (String, Vec<(String, Duration)>, Option<Duration>) {
    let mut text = String::new();
    let mut words: Vec<(String, Duration)> = Vec::new();
    let mut pending: Option<Duration> = None;
    let mut segment = String::new();
    let mut end = None;
    let mut rest = body;
    let mut any_tag = false;

    loop {
        let tag = rest
            .find('<')
            .and_then(|open| rest[open..].find('>').map(|close| (open, open + close)))
            .and_then(|(open, close)| parse_time(&rest[open + 1..close]).map(|t| (open, close, t)));
        let Some((open, close, time)) = tag else {
            segment.push_str(rest);
            break;
        };
        any_tag = true;
        segment.push_str(&rest[..open]);
        flush(&mut text, &mut words, &mut segment, pending);
        pending = Some(time);
        rest = &rest[close + 1..];
    }
    // The text after the last tag: a word if there is any, else that tag was
    // the end of the line.
    if segment.trim().is_empty() {
        end = pending.filter(|_| any_tag);
        text.push_str(&segment);
    } else {
        flush(&mut text, &mut words, &mut segment, pending);
    }
    (text.trim().to_owned(), words, end)
}

/// Moves `segment` into the line text, and into the words when it has a time.
fn flush(
    text: &mut String,
    words: &mut Vec<(String, Duration)>,
    segment: &mut String,
    time: Option<Duration>,
) {
    let word = segment.trim();
    if let (Some(time), false) = (time, word.is_empty()) {
        words.push((word.to_owned(), time));
    }
    text.push_str(segment);
    segment.clear();
}

/// Word ends: the next word's start, else the line's trailing time, else the
/// next line's start, capped at [`LyricWord::MAX_SWEEP`] as every source caps
/// them, so both sources sweep alike.
fn finish_words(
    words: &[(String, Duration)],
    line_end: Option<Duration>,
    next_line: Option<Duration>,
    apply: impl Fn(Duration) -> Duration,
) -> Vec<LyricWord> {
    words
        .iter()
        .enumerate()
        .map(|(i, (text, start))| {
            let start = apply(*start);
            let natural = match words.get(i + 1) {
                Some((_, next)) => Some(apply(*next)),
                None => line_end.map(&apply).or(next_line),
            };
            let cap = start + LyricWord::MAX_SWEEP;
            let end = natural
                .filter(|end| *end >= start)
                .map_or(cap, |end| end.min(cap));
            LyricWord {
                text: text.clone(),
                start,
                end,
            }
        })
        .collect()
}

/// `[offset:+500]` moves every time 500 ms earlier; never before zero.
fn apply_offset(time: Duration, offset_ms: i64) -> Duration {
    let shift = Duration::from_millis(offset_ms.unsigned_abs());
    if offset_ms >= 0 {
        time.saturating_sub(shift)
    } else {
        time.saturating_add(shift)
    }
}

/// `mm:ss`, `mm:ss.f`, `mm:ss.ff`, `mm:ss.fff`, `mm:ss:ff`, `hh:mm:ss.ff`.
fn parse_time(tag: &str) -> Option<Duration> {
    let tag = tag.trim();
    if tag.is_empty()
        || !tag
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b':' || b == b'.')
    {
        return None;
    }
    let parts: Vec<&str> = tag.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [m, s] => ("0", *m, *s),
        // A third part without a dot is centiseconds after a colon, the common
        // misspelling; with a dot it is hours, minutes, seconds.
        [m, s, f] if !f.contains('.') => {
            return clock(0, number(m)?, number(s)?, fraction(f)?);
        }
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    let (whole, frac) = seconds.split_once('.').unwrap_or((seconds, ""));
    clock(
        number(hours)?,
        number(minutes)?,
        number(whole)?,
        fraction(frac)?,
    )
}

/// No song is a day long: a later time is a broken or hostile tag.
const LONGEST: u64 = 24 * 3600;

fn clock(hours: u64, minutes: u64, seconds: u64, millis: u64) -> Option<Duration> {
    if seconds >= 60 {
        return None;
    }
    let total = hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    (total < LONGEST).then(|| Duration::from_secs(total) + Duration::from_millis(millis))
}

fn number(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Fraction digits as milliseconds: `5` is 500, `05` is 50, `005` is 5; digits
/// past the third are dropped.
fn fraction(digits: &str) -> Option<u64> {
    if digits.is_empty() {
        return Some(0);
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let kept = &digits[..digits.len().min(3)];
    let value: u64 = kept.parse().ok()?;
    Some(value * 10u64.pow(3 - kept.len() as u32))
}

/// Plain text with any `[ar:…]`-style header lines taken out.
fn strip_header_tags(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let (_, _, body) = leading_tags(line.trim());
            let only_tags = body.is_empty() && line.trim_start().starts_with('[');
            !only_tags
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_times_neither_panic_nor_count() {
        // From the security audit: each of these panicked with "overflow when
        // adding durations".
        let huge = "5124095576030431";
        for text in [
            format!("[{huge}:00:00.00]<{huge}:00:00.00>a\n"),
            format!("[offset:-9223372036854775807]\n[{huge}:00:00.00]hi\n"),
            format!("[{huge}:00:00.00]a\n[{huge}:00:00.50]b\n"),
            "[offset:-9223372036854775807]\n[00:01.00]hi\n".to_owned(),
            "[00:01.00]<00:02.00>a [00:03.00]<99999999999999:00.00>b\n".to_owned(),
        ] {
            let lyrics = parse_lrc(&text);
            for line in &lyrics.lines {
                assert!(
                    line.start
                        .is_none_or(|s| s < Duration::from_secs(u64::MAX / 2))
                );
            }
        }
        // A day or more is no time at all.
        assert_eq!(parse_time("24:00:00.00"), None);
        assert_eq!(
            parse_time("23:59:59.99"),
            Some(Duration::from_millis(86_399_990))
        );
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn texts(lyrics: &Lyrics) -> Vec<&str> {
        lyrics.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn plain_lrc_lines_are_synced_without_words() {
        let lyrics = parse_lrc("[ar:Someone]\n[ti:Song]\n[00:01.00]First\n[00:02.50]Second\n");
        assert_eq!(texts(&lyrics), ["First", "Second"]);
        assert!(lyrics.is_synced());
        assert!(!lyrics.has_word_timing());
        assert_eq!(lyrics.lines[1].start, Some(ms(2_500)));
    }

    #[test]
    fn every_timestamp_shape_parses() {
        assert_eq!(parse_time("01:02"), Some(ms(62_000)));
        assert_eq!(parse_time("01:02.5"), Some(ms(62_500)));
        assert_eq!(parse_time("01:02.05"), Some(ms(62_050)));
        assert_eq!(parse_time("01:02.005"), Some(ms(62_005)));
        assert_eq!(parse_time("01:02.0059"), Some(ms(62_005)));
        assert_eq!(parse_time("01:02:50"), Some(ms(62_500)));
        assert_eq!(parse_time("1:01:02.50"), Some(ms(3_662_500)));
        assert_eq!(parse_time("125:00.00"), Some(ms(7_500_000)));
        for bad in [
            "", "ar:x", "01:75.00", "a1:02", "01:02.x", "1:2:3:4", "-1:00",
        ] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_line_with_several_times_repeats_in_order() {
        let lyrics = parse_lrc("[00:10.00][00:30.00]Chorus\n[00:20.00]Verse\n");
        assert_eq!(texts(&lyrics), ["Chorus", "Verse", "Chorus"]);
        let starts: Vec<_> = lyrics.lines.iter().map(|l| l.start.unwrap()).collect();
        assert_eq!(starts, [ms(10_000), ms(20_000), ms(30_000)]);
    }

    #[test]
    fn enhanced_lrc_has_word_timings_and_a_trailing_end() {
        let lyrics = parse_lrc(
            "[00:01.00]<00:01.00>Hello <00:01.50>big <00:01.80>world<00:02.00>\n[00:04.00]Next\n",
        );
        let line = &lyrics.lines[0];
        assert_eq!(line.text, "Hello big world");
        let words: Vec<_> = line
            .words
            .iter()
            .map(|w| (w.text.as_str(), w.start, w.end))
            .collect();
        assert_eq!(
            words,
            [
                ("Hello", ms(1_000), ms(1_500)),
                ("big", ms(1_500), ms(1_800)),
                ("world", ms(1_800), ms(2_000)),
            ]
        );
        assert!(lyrics.lines[1].words.is_empty());
    }

    #[test]
    fn the_last_word_without_an_end_runs_to_the_next_line_capped() {
        let lyrics = parse_lrc(
            "[00:01.00]<00:01.00>one <00:01.20>two\n[00:01.90]x\n[00:10.00]<00:10.00>long\n[01:00.00]y\n",
        );
        assert_eq!(lyrics.lines[0].words[1].end, ms(1_900));
        // Fifty seconds to the next line, capped at 1.2 s.
        assert_eq!(lyrics.lines[2].words[0].end, ms(11_200));
    }

    #[test]
    fn text_before_the_first_word_time_is_in_the_line_but_not_a_word() {
        let lyrics = parse_lrc("[00:05.00]Oh <00:05.40>yeah<00:06.00>\n");
        assert_eq!(lyrics.lines[0].text, "Oh yeah");
        assert_eq!(lyrics.lines[0].words.len(), 1);
        assert_eq!(lyrics.lines[0].words[0].text, "yeah");
    }

    #[test]
    fn an_offset_moves_lines_and_words_never_below_zero() {
        let lyrics = parse_lrc("[offset:+500]\n[00:00.20]a\n[00:02.00]<00:02.00>b <00:02.40>c\n");
        assert_eq!(lyrics.lines[0].start, Some(Duration::ZERO));
        assert_eq!(lyrics.lines[1].start, Some(ms(1_500)));
        assert_eq!(lyrics.lines[1].words[1].start, ms(1_900));
        let later = parse_lrc("[offset:-250]\n[00:01.00]a\n");
        assert_eq!(later.lines[0].start, Some(ms(1_250)));
    }

    #[test]
    fn empty_timed_lines_stay_as_breaks_and_untimed_text_goes() {
        let lyrics = parse_lrc("Lyrics by someone\n[00:01.00]Sing\n[00:05.00]\n[00:09.00]Again\n");
        assert_eq!(texts(&lyrics), ["Sing", "", "Again"]);
    }

    #[test]
    fn a_bracketed_word_is_text_not_a_tag() {
        let lyrics = parse_lrc("[00:01.00][Chorus] la la\n");
        assert_eq!(lyrics.lines[0].text, "[Chorus] la la");
    }

    #[test]
    fn no_timestamps_means_plain_text_without_the_header() {
        let lyrics = parse_lrc("[ar:Someone]\nJust words\n\nSecond verse\n");
        assert!(!lyrics.is_synced());
        assert_eq!(texts(&lyrics), ["Just words", "", "Second verse"]);
    }

    #[test]
    fn plain_text_trims_blank_edges_and_a_bom() {
        let lyrics = parse_plain("\u{feff}\n\n  First  \r\n\r\nSecond\n\n");
        assert_eq!(texts(&lyrics), ["First", "", "Second"]);
        assert!(parse_plain(" \n\n").is_empty());
    }

    #[test]
    fn crlf_and_a_bom_dont_break_lrc() {
        let lyrics = parse_lrc("\u{feff}[00:01.00]One\r\n[00:02.00]Two\r\n");
        assert_eq!(texts(&lyrics), ["One", "Two"]);
    }

    #[test]
    fn parsed_lyrics_follow_the_clock_like_the_servers() {
        let lyrics = parse_lrc("[00:01.00]a\n[00:05.00]b\n");
        assert_eq!(lyrics.line_at(ms(4_000)), Some(0));
        assert_eq!(lyrics.line_at(ms(5_000)), Some(1));
    }
}
