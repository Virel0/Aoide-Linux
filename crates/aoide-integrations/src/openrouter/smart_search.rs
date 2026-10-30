//! Turning a phrase like "upbeat rock from the nineties" into filters.
//!
//! **The model translates; it never chooses tracks.** Carried over from the
//! iOS app, where it is a measurement rather than a preference: asked to pick
//! songs for a named genre, the on-device model was right 2–5 times out of 8,
//! worse than useless when the server can answer the same question exactly.
//! Mapping a phrase onto a genre and a year range is the part a model is good
//! at; Melete does the selecting afterwards.
//!
//! Ported from the Electron app's `src/shared/aoide/smart-search.ts`, with the
//! iOS app's sort directions (`SmartSearch.swift`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::json::extract_object;

/// The order a listener asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ordering {
    /// A to Z by name. The default.
    #[default]
    ByName,
    /// The most played first.
    MostPlayed,
    /// The newest release first.
    NewestFirst,
    /// Shuffled.
    Random,
    /// The most recently played first.
    RecentlyPlayed,
}

impl Ordering {
    /// The spellings the prompt offers and the reply uses.
    pub const ALL: [Ordering; 5] = [
        Self::ByName,
        Self::MostPlayed,
        Self::NewestFirst,
        Self::Random,
        Self::RecentlyPlayed,
    ];

    /// The camelCase spelling: `byName`, `mostPlayed`, ….
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ByName => "byName",
            Self::MostPlayed => "mostPlayed",
            Self::NewestFirst => "newestFirst",
            Self::Random => "random",
            Self::RecentlyPlayed => "recentlyPlayed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == text)
    }

    /// Whether the largest value comes first, as the iOS app sorts:
    /// "newest", "most played" and "recently played" all mean descending;
    /// by name and random ascend. Melete sorts by name; the rest are put
    /// in order in the app (year, this device's history, or shuffled).
    pub fn descending(self) -> bool {
        matches!(
            self,
            Self::NewestFirst | Self::MostPlayed | Self::RecentlyPlayed
        )
    }
}

/// What a phrase becomes. Every field is a filter the server understands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MusicQuery {
    /// Only the listener's favourites.
    pub favorites_only: bool,
    /// The earliest release year, inclusive; never after `to_year`.
    pub from_year: Option<i32>,
    /// Always a subset of the library's own genres, in the library's
    /// spelling. Never the model's invention.
    pub genres: Vec<String>,
    /// The order to list the results in.
    pub ordering: Ordering,
    /// Words to match names against, when the phrase named something.
    pub search_term: Option<String>,
    /// The latest release year, inclusive.
    pub to_year: Option<i32>,
}

impl MusicQuery {
    /// True when the query would filter nothing, and a plain search is the
    /// better answer: running it would return the library in alphabetical
    /// order dressed up as an answer.
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty()
            && self.from_year.is_none()
            && self.to_year.is_none()
            && self.search_term.is_none()
            && !self.favorites_only
    }
}

/// Below this many words a phrase is a name, which the plain search handles
/// better, and every hosted call costs money.
pub const MINIMUM_PHRASE_WORDS: usize = 3;

/// Whether `text` is worth sending to a model at all.
pub fn looks_like_a_phrase(text: &str) -> bool {
    text.split_whitespace().count() >= MINIMUM_PHRASE_WORDS
}

/// The instructions, carrying the library's own genre vocabulary.
///
/// The vocabulary is the single biggest quality lever: left to itself a model
/// invents "Chill" and "Workout", which match nothing. The genre names are
/// the only thing about the library that leaves the machine; titles, artists
/// and listening history never do.
pub fn build_prompt(phrase: &str, library_genres: &[String]) -> String {
    let vocabulary = if library_genres.is_empty() {
        "(this library reports no genres — leave genres empty)".to_owned()
    } else {
        library_genres.join(", ")
    };
    let orderings = Ordering::ALL
        .iter()
        .map(|o| format!("\"{}\"", o.as_str()))
        .collect::<Vec<_>>()
        .join("|");
    [
        "You translate a listener’s description of music into search filters.".to_owned(),
        "You never choose songs. You only produce filters; the music server selects.".to_owned(),
        String::new(),
        "Reply with JSON only, matching exactly this shape:".to_owned(),
        "{\"searchTerm\": string|null, \"genres\": string[], \"fromYear\": number|null,".to_owned(),
        " \"toYear\": number|null, \"favoritesOnly\": boolean,".to_owned(),
        format!(" \"ordering\": {orderings}}}"),
        String::new(),
        "Rules:".to_owned(),
        "- genres MUST be chosen from this library’s genres, spelled exactly as given.".to_owned(),
        "  If none apply, return an empty array. Never invent a genre.".to_owned(),
        "- fromYear/toYear express a period: \"the nineties\" is 1990 to 1999.".to_owned(),
        "- searchTerm is a song, album or artist name if the description names one.".to_owned(),
        "- favoritesOnly is true only for favourites, loved or starred music.".to_owned(),
        "- ordering is byName unless the listener asked for something else.".to_owned(),
        String::new(),
        format!("This library’s genres: {vocabulary}"),
        String::new(),
        format!("Description: {phrase}"),
    ]
    .join("\n")
}

/// Reads a model's reply, keeping only what can be believed: genres the
/// library has (matched case-insensitively, returned in the library's
/// spelling, each once), plausible years (a reversed range is put right),
/// a known ordering (else by name). `None` when there is no JSON object.
pub fn parse_query(reply: &str, library_genres: &[String]) -> Option<MusicQuery> {
    parse_query_in(reply, library_genres, current_year())
}

fn parse_query_in(reply: &str, library_genres: &[String], this_year: i32) -> Option<MusicQuery> {
    let parsed = extract_object(reply)?;

    let mut genres: Vec<String> = Vec::new();
    for wanted in parsed
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(wanted) = wanted.as_str().map(str::to_lowercase) else {
            continue;
        };
        // As the Electron app's lookup map: a later spelling of the same
        // genre wins.
        let found = library_genres
            .iter()
            .rev()
            .find(|g| g.to_lowercase() == wanted);
        if let Some(genre) = found
            && !genres.contains(genre)
        {
            genres.push(genre.clone());
        }
    }

    let from = parsed.get("fromYear").and_then(|v| as_year(v, this_year));
    let to = parsed.get("toYear").and_then(|v| as_year(v, this_year));
    // A reversed range describes nothing; the likeliest reading is that the
    // model filled the two fields the wrong way round.
    let (from_year, to_year) = match (from, to) {
        (Some(a), Some(b)) => (Some(a.min(b)), Some(a.max(b))),
        other => other,
    };

    Some(MusicQuery {
        favorites_only: parsed.get("favoritesOnly") == Some(&Value::Bool(true)),
        from_year,
        genres,
        ordering: parsed
            .get("ordering")
            .and_then(Value::as_str)
            .and_then(Ordering::parse)
            .unwrap_or_default(),
        search_term: parsed
            .get("searchTerm")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
        to_year,
    })
}

/// A year from a number, or from the string a model often returns instead
/// (read as JavaScript's `parseInt` would: "1990s" is 1990). Recorded music
/// does not predate 1860, and a year past next year is a hallucination.
fn as_year(value: &Value, this_year: i32) -> Option<i32> {
    let year = match value {
        Value::Number(n) => n.as_f64()?.trunc(),
        Value::String(s) => leading_integer(s)? as f64,
        _ => return None,
    };
    let range = 1860.0..=f64::from(this_year + 1);
    // In range and whole, so the cast is exact.
    range.contains(&year).then_some(year as i32)
}

fn leading_integer(text: &str) -> Option<i64> {
    let text = text.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|n| sign * n)
}

/// The current calendar year (UTC).
pub(crate) fn current_year() -> i32 {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0);
    year_of_day(i64::try_from(days).unwrap_or(0))
}

/// The proleptic Gregorian year of a day counted from 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn year_of_day(days: i64) -> i32 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    i32::try_from(year).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn genres() -> Vec<String> {
        ["Rock", "Jazz", "Electronic", "Hip-Hop", "Classical"]
            .map(String::from)
            .to_vec()
    }

    fn reply(over: Value) -> String {
        let mut base = json!({
            "favoritesOnly": false, "fromYear": null, "genres": [],
            "ordering": "byName", "searchTerm": null, "toYear": null,
        });
        for (k, v) in over.as_object().unwrap() {
            base[k] = v.clone();
        }
        base.to_string()
    }

    fn parse(over: Value) -> MusicQuery {
        parse_query(&reply(over), &genres()).unwrap()
    }

    // When to ask a model at all.

    #[test]
    fn leaves_one_or_two_words_to_the_plain_search() {
        assert!(!looks_like_a_phrase("Radiohead"));
        assert!(!looks_like_a_phrase("kid a"));
        assert_eq!(MINIMUM_PHRASE_WORDS, 3);
    }

    #[test]
    fn offers_to_translate_a_real_phrase() {
        assert!(looks_like_a_phrase("upbeat rock from the nineties"));
    }

    #[test]
    fn is_not_fooled_by_spacing() {
        assert!(!looks_like_a_phrase("   jazz   "));
        assert!(looks_like_a_phrase("  quiet   jazz   for   evening "));
    }

    // The prompt.

    #[test]
    fn the_prompt_carries_the_librarys_own_genres() {
        let prompt = build_prompt("something upbeat", &genres());
        for genre in genres() {
            assert!(prompt.contains(&genre), "{genre}");
        }
    }

    #[test]
    fn says_so_when_the_library_reports_no_genres() {
        assert!(build_prompt("something upbeat", &[]).contains("leave genres empty"));
    }

    // The genre list is the only thing about the library that leaves the
    // machine: the prompt varies with the vocabulary and nothing else.
    #[test]
    fn varies_with_the_vocabulary_and_with_nothing_else_about_the_library() {
        let phrase = "quiet music for reading";
        let small = build_prompt(phrase, &["Jazz".into()]);
        let large = build_prompt(
            phrase,
            &["Jazz".into(), "Ambient".into(), "Classical".into()],
        );
        assert!(small.contains(phrase));
        let up_to = |p: &str| p[..p.find("genres: ").unwrap()].to_owned();
        assert_eq!(up_to(&small), up_to(&large));
        assert_eq!(large.replace("Jazz, Ambient, Classical", "Jazz"), small);
    }

    #[test]
    fn the_prompt_offers_every_ordering() {
        let prompt = build_prompt("x y z", &[]);
        assert!(prompt.contains(
            r#""ordering": "byName"|"mostPlayed"|"newestFirst"|"random"|"recentlyPlayed"}"#
        ));
    }

    // Reading the reply.

    #[test]
    fn keeps_a_genre_the_library_actually_has() {
        assert_eq!(parse(json!({"genres": ["Jazz"]})).genres, ["Jazz"]);
    }

    #[test]
    fn discards_a_genre_the_library_does_not_have() {
        assert!(
            parse(json!({"genres": ["Chill", "Workout"]}))
                .genres
                .is_empty()
        );
    }

    #[test]
    fn matches_case_insensitively_but_answers_in_the_librarys_spelling() {
        assert_eq!(
            parse(json!({"genres": ["jazz", "HIP-HOP"]})).genres,
            ["Jazz", "Hip-Hop"]
        );
    }

    #[test]
    fn does_not_repeat_a_genre_named_twice() {
        assert_eq!(parse(json!({"genres": ["Jazz", "jazz"]})).genres, ["Jazz"]);
    }

    #[test]
    fn reads_a_year_returned_as_a_string() {
        let q = parse(json!({"fromYear": "1990", "toYear": "1999"}));
        assert_eq!((q.from_year, q.to_year), (Some(1990), Some(1999)));
    }

    #[test]
    fn rights_a_range_the_model_filled_backwards() {
        let q = parse(json!({"fromYear": 1999, "toYear": 1990}));
        assert_eq!((q.from_year, q.to_year), (Some(1990), Some(1999)));
    }

    #[test]
    fn rejects_a_year_that_cannot_be_a_release_date() {
        assert_eq!(parse(json!({"fromYear": 3050})).from_year, None);
        assert_eq!(parse(json!({"fromYear": 1200})).from_year, None);
    }

    #[test]
    fn next_year_is_allowed_and_the_one_after_is_not() {
        let year = |y: i32| {
            parse_query_in(&reply(json!({ "fromYear": y })), &genres(), 2026)
                .unwrap()
                .from_year
        };
        assert_eq!(year(2027), Some(2027));
        assert_eq!(year(2028), None);
        assert_eq!(year(1860), Some(1860));
    }

    #[test]
    fn falls_back_to_by_name_for_an_ordering_it_does_not_recognise() {
        assert_eq!(
            parse(json!({"ordering": "vibes"})).ordering,
            Ordering::ByName
        );
        assert_eq!(
            parse(json!({"ordering": "newestFirst"})).ordering,
            Ordering::NewestFirst
        );
    }

    #[test]
    fn treats_favourites_only_as_false_unless_it_is_exactly_true() {
        assert!(!parse(json!({"favoritesOnly": "yes"})).favorites_only);
        assert!(parse(json!({"favoritesOnly": true})).favorites_only);
    }

    #[test]
    fn ignores_an_empty_search_term_rather_than_searching_for_nothing() {
        assert_eq!(parse(json!({"searchTerm": "   "})).search_term, None);
        assert_eq!(
            parse(json!({"searchTerm": " Muse "}))
                .search_term
                .as_deref(),
            Some("Muse")
        );
    }

    #[test]
    fn finds_json_inside_a_fenced_block() {
        let wrapped = format!("```json\n{}\n```", reply(json!({"genres": ["Rock"]})));
        assert_eq!(parse_query(&wrapped, &genres()).unwrap().genres, ["Rock"]);
    }

    #[test]
    fn finds_json_after_a_sentence() {
        let chatty = format!(
            "Sure! Here are the filters:\n{}",
            reply(json!({"genres": ["Rock"]}))
        );
        assert_eq!(parse_query(&chatty, &genres()).unwrap().genres, ["Rock"]);
    }

    #[test]
    fn gives_up_rather_than_guessing_when_there_is_no_json() {
        assert_eq!(
            parse_query("I am afraid I cannot help with that.", &genres()),
            None
        );
    }

    #[test]
    fn gives_up_on_a_json_array_which_is_not_a_query() {
        assert_eq!(parse_query(r#"["Rock"]"#, &genres()), None);
    }

    #[test]
    fn survives_a_reply_with_every_field_missing() {
        let q = parse_query("{}", &genres()).unwrap();
        assert!(q.genres.is_empty());
        assert_eq!(q.ordering, Ordering::ByName);
    }

    // is_empty.

    #[test]
    fn recognises_a_query_that_would_filter_nothing() {
        assert!(parse_query("{}", &genres()).unwrap().is_empty());
    }

    #[test]
    fn does_not_call_a_genre_only_or_favourites_only_query_empty() {
        assert!(!parse(json!({"genres": ["Jazz"]})).is_empty());
        assert!(!parse(json!({"favoritesOnly": true})).is_empty());
    }

    // The iOS app's sort directions.

    #[test]
    fn newest_most_and_recently_played_sort_largest_first() {
        assert!(Ordering::NewestFirst.descending());
        assert!(Ordering::MostPlayed.descending());
        assert!(Ordering::RecentlyPlayed.descending());
        assert!(!Ordering::ByName.descending());
        assert!(!Ordering::Random.descending());
    }

    #[test]
    fn the_calendar_is_right() {
        assert_eq!(year_of_day(0), 1970);
        assert_eq!(year_of_day(-1), 1969);
        // 2026-10-24 is day 20 750.
        assert_eq!(year_of_day(20_750), 2026);
        // 2000-02-29 and 2000-12-31.
        assert_eq!(year_of_day(11_016), 2000);
        assert_eq!(year_of_day(11_322), 2000);
        assert_eq!(year_of_day(11_323), 2001);
        assert!(current_year() >= 2026);
    }
}
