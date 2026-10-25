//! The rules behind a mix that are neither the model's nor Jellyfin's: which
//! library genres a described genre means, which candidates go first,
//! whether the library as a whole is worth asking, and the one line that
//! says what was understood. That line is the diagnostic: a mix that comes
//! back empty is explained by it or by nothing.
//!
//! Ported from the Electron app's `src/renderer/aoide/features/mix/mix-plan.ts`;
//! the genre rule is the phone's `MixComposer.libraryGenres(matching:in:)`,
//! so a description resolves to the same library genres on every app.

use std::collections::HashSet;

use super::smart_rules::{RuleField, RuleOperator, RuleValue, SmartRule, SmartRules};

/// The words of a name: lower-cased, split on anything that is not a letter
/// or a digit.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Library genres that mean what the described genres mean.
///
/// Whole-word containment, either way: every word of one side must be a
/// word of the other. A model's "rock" takes "Rock", "Hard Rock" and
/// "Alternative Rock" but not "Rockabilly"; "alternative rock" takes "Rock";
/// "soundtrack" takes "Video Game Soundtrack". In the library's order, each
/// once.
pub fn match_genres(described: &[String], library: &[String]) -> Vec<String> {
    let wanted: Vec<Vec<String>> = described
        .iter()
        .map(|d| words(d))
        .filter(|w| !w.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    library
        .iter()
        .filter(|candidate| {
            let have = words(candidate);
            !have.is_empty()
                && wanted.iter().any(|want| {
                    want.iter().all(|w| have.contains(w)) || have.iter().all(|h| want.contains(h))
                })
        })
        .cloned()
        .collect()
}

/// The genre names a rule set asks for (`genre is` and `genre contains`), as
/// the model spelled them.
pub fn described_genres(rules: Option<&SmartRules>) -> Vec<String> {
    rules
        .map(|r| r.rules.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|rule| {
            rule.field == RuleField::Genre
                && matches!(rule.op, RuleOperator::Is | RuleOperator::Contains)
        })
        .map(SmartRule::value_text)
        .collect()
}

/// Name hits first, then the rule candidates, each id once. The limit is
/// counted from the front, so this keeps the name hits in a mix that also
/// asked for hundreds of candidates by genre.
pub fn order_candidates(name_ids: &[String], rule_ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    name_ids
        .iter()
        .chain(rule_ids)
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// Puts the name hits back in front after the history has narrowed (which
/// sorts by last played), keeping the order within each half.
pub fn names_first(chosen: &[String], name_ids: &HashSet<String>) -> Vec<String> {
    let (named, rest): (Vec<String>, Vec<String>) =
        chosen.iter().cloned().partition(|id| name_ids.contains(id));
    named.into_iter().chain(rest).collect()
}

/// Whether to ask Jellyfin for candidates by the rules at all. "All of
/// nothing" is the whole library, a real mix when the description said
/// nothing more specific, and noise beside a name.
pub fn wants_rule_candidates(rules: Option<&SmartRules>, names: &[String]) -> bool {
    rules.is_some_and(|r| !r.rules.is_empty() || names.is_empty())
}

/// What a mix resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MixPlan {
    /// The library genres the description resolved to, the library's way.
    pub genres: Vec<String>,
    pub names: Vec<String>,
    pub rules: Option<SmartRules>,
}

/// `-6m` as "6 months", `-1y` as "a year"; anything else as itself.
fn span_words(value: &RuleValue) -> String {
    let text = match value {
        RuleValue::Text(t) => t.as_str(),
        other => return other_text(other),
    };
    let digits = text.strip_prefix('-').unwrap_or(text);
    let unit = digits.chars().last();
    let amount = unit.and_then(|u| digits[..digits.len() - u.len_utf8()].parse::<u64>().ok());
    let (one, many) = match unit {
        Some('d') => ("day", "days"),
        Some('w') => ("week", "weeks"),
        Some('m') => ("month", "months"),
        Some('y') => ("year", "years"),
        _ => return text.to_owned(),
    };
    match amount {
        Some(1) => format!("a {one}"),
        Some(n) => format!("{n} {many}"),
        None => text.to_owned(),
    }
}

fn other_text(value: &RuleValue) -> String {
    match value {
        RuleValue::Flag(b) => b.to_string(),
        RuleValue::Number(n) => n.to_string(),
        RuleValue::Text(t) => t.clone(),
    }
}

/// One rule as a person would say it, or nothing for a rule the line
/// already covers.
fn rule_phrase(rule: &SmartRule) -> Option<String> {
    use RuleField as F;
    use RuleOperator as O;
    let value = &rule.value;
    match (rule.field, rule.op) {
        (F::DurationMs | F::Genre, _) => None,
        (F::FirstPlayed, O::InTheLast) => Some(format!("new in the last {}", span_words(value))),
        (F::FirstPlayed, _) => None,
        (F::LastPlayed, O::NotInTheLast) => Some(format!("not played in {}", span_words(value))),
        (F::LastPlayed, O::InTheLast) => Some(format!("played in the last {}", span_words(value))),
        (F::LastPlayed, _) => None,
        (F::Liked, _) => Some(
            if *value == RuleValue::Flag(true) {
                "favourites"
            } else {
                "not favourites"
            }
            .to_owned(),
        ),
        (F::PlayCount, O::Is) if *value == RuleValue::Number(0) => Some("never played".into()),
        (F::PlayCount, O::GreaterThan) => {
            Some(format!("played more than {} times", other_text(value)))
        }
        (F::PlayCount, O::LessThan) => {
            Some(format!("played fewer than {} times", other_text(value)))
        }
        (F::PlayCount, _) => None,
        (F::SkipCount, O::Is) if *value == RuleValue::Number(0) => Some("never skipped".into()),
        (F::SkipCount, _) => None,
        (F::Year, O::GreaterThan) => Some(format!("after {}", other_text(value))),
        (F::Year, O::LessThan) => Some(format!("before {}", other_text(value))),
        (F::Year, O::Is) => Some(format!("from {}", other_text(value))),
        (F::Year, _) => None,
        // title, artist, album, album_artist: "artist contains Muse".
        (field, op) => Some(format!(
            "{} {} {}",
            field.as_str().replacen('_', " ", 1),
            op.as_str(),
            other_text(value)
        )),
    }
}

/// What the model understood, as one line: the names, the library genres,
/// the rest of the rules in words, and the length.
///
/// `Helldivers 2 · Rock, Metal, Electronic · not played in 6 months · 40 tracks`
pub fn describe_plan(plan: &MixPlan) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !plan.names.is_empty() {
        parts.push(plan.names.join(", "));
    }
    if !plan.genres.is_empty() {
        parts.push(plan.genres.join(", "));
    }
    let rules = plan
        .rules
        .as_ref()
        .map(|r| r.rules.as_slice())
        .unwrap_or_default();
    for phrase in rules.iter().filter_map(rule_phrase) {
        if !parts.contains(&phrase) {
            parts.push(phrase);
        }
    }
    if let Some(limit) = plan.rules.as_ref().and_then(|r| r.limit) {
        parts.push(format!("{limit} tracks"));
    }
    if parts.is_empty() {
        "anything".into()
    } else {
        parts.join(" · ")
    }
}

/// The sentence for names the library has nothing for.
pub fn missed_names_message(missed: &[String]) -> String {
    format!("Nothing in your library mentions {}.", missed.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openrouter::smart_rules::Match;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn library() -> Vec<String> {
        strings(&[
            "Alternative Rock",
            "Ambient",
            "Hard Rock",
            "Lo-Fi",
            "Metal",
            "Rock",
            "Rockabilly",
            "Video Game Soundtrack",
        ])
    }

    fn rule(field: RuleField, op: RuleOperator, value: RuleValue) -> SmartRule {
        SmartRule { field, op, value }
    }

    fn set(rules: Vec<SmartRule>, limit: Option<u32>) -> SmartRules {
        SmartRules {
            limit,
            match_: Match::All,
            rules,
            sort: None,
        }
    }

    fn text(t: &str) -> RuleValue {
        RuleValue::Text(t.into())
    }

    // match_genres

    #[test]
    fn matches_exactly_ignoring_case() {
        assert_eq!(match_genres(&strings(&["metal"]), &library()), ["Metal"]);
        assert_eq!(match_genres(&strings(&["METAL"]), &library()), ["Metal"]);
    }

    #[test]
    fn takes_every_library_genre_containing_the_described_one_as_a_whole_word() {
        assert_eq!(
            match_genres(&strings(&["rock"]), &library()),
            ["Alternative Rock", "Hard Rock", "Rock"]
        );
    }

    #[test]
    fn takes_a_library_genre_whose_words_are_all_in_the_described_one() {
        assert_eq!(
            match_genres(&strings(&["alternative rock"]), &library()),
            ["Alternative Rock", "Rock"]
        );
    }

    #[test]
    fn takes_video_game_soundtrack_for_soundtrack() {
        assert_eq!(
            match_genres(&strings(&["soundtrack"]), &library()),
            ["Video Game Soundtrack"]
        );
    }

    #[test]
    fn does_not_take_rockabilly_for_rock() {
        assert!(!match_genres(&strings(&["rock"]), &library()).contains(&"Rockabilly".into()));
    }

    #[test]
    fn ignores_punctuation() {
        assert_eq!(match_genres(&strings(&["lo-fi"]), &library()), ["Lo-Fi"]);
        assert_eq!(match_genres(&strings(&["lo fi"]), &library()), ["Lo-Fi"]);
        assert!(match_genres(&strings(&["lofi"]), &library()).is_empty());
    }

    #[test]
    fn returns_each_library_genre_once_in_the_librarys_order() {
        assert_eq!(
            match_genres(&strings(&["rock", "hard rock"]), &library()),
            ["Alternative Rock", "Hard Rock", "Rock"]
        );
    }

    #[test]
    fn matches_nothing_for_an_invented_genre_or_for_nothing() {
        assert!(match_genres(&strings(&["Chill"]), &library()).is_empty());
        assert!(match_genres(&[], &library()).is_empty());
        assert!(match_genres(&strings(&[""]), &library()).is_empty());
        assert!(match_genres(&strings(&["rock"]), &[]).is_empty());
    }

    // described_genres

    #[test]
    fn reads_the_genre_rules_as_the_model_spelled_them() {
        let rules = set(
            vec![
                rule(RuleField::Genre, RuleOperator::Is, text("rock")),
                rule(RuleField::Genre, RuleOperator::Contains, text("metal")),
                rule(
                    RuleField::Year,
                    RuleOperator::GreaterThan,
                    RuleValue::Number(1990),
                ),
            ],
            None,
        );
        assert_eq!(described_genres(Some(&rules)), ["rock", "metal"]);
        assert!(described_genres(None).is_empty());
    }

    // order_candidates and names_first

    #[test]
    fn puts_the_name_hits_first_and_drops_repeats() {
        assert_eq!(
            order_candidates(&strings(&["a", "b"]), &strings(&["c", "a", "d", "b"])),
            ["a", "b", "c", "d"]
        );
        assert_eq!(order_candidates(&strings(&["a", "a"]), &[]), ["a"]);
    }

    #[test]
    fn moves_the_name_hits_to_the_front_keeping_each_halfs_order() {
        let names: HashSet<String> = ["a", "b"].map(String::from).into();
        assert_eq!(
            names_first(&strings(&["c", "a", "d", "b"]), &names),
            ["a", "b", "c", "d"]
        );
        assert_eq!(
            names_first(&strings(&["c", "a"]), &HashSet::new()),
            ["c", "a"]
        );
    }

    // wants_rule_candidates

    #[test]
    fn asks_the_library_when_there_are_rules_or_nothing_else() {
        let empty = set(vec![], None);
        let genre = set(
            vec![rule(RuleField::Genre, RuleOperator::Is, text("Rock"))],
            None,
        );
        let named = strings(&["Helldivers 2"]);
        assert!(wants_rule_candidates(Some(&genre), &[]));
        assert!(wants_rule_candidates(Some(&genre), &named));
        assert!(wants_rule_candidates(Some(&empty), &[]));
        assert!(!wants_rule_candidates(Some(&empty), &named));
        assert!(!wants_rule_candidates(None, &[]));
        assert!(!wants_rule_candidates(None, &named));
    }

    // describe_plan

    #[test]
    fn says_the_names_the_genres_the_rules_and_the_length() {
        let plan = MixPlan {
            genres: strings(&["Rock", "Metal", "Electronic"]),
            names: strings(&["Helldivers 2"]),
            rules: Some(set(
                vec![
                    rule(RuleField::Genre, RuleOperator::Is, text("rock")),
                    rule(
                        RuleField::LastPlayed,
                        RuleOperator::NotInTheLast,
                        text("-6m"),
                    ),
                ],
                Some(40),
            )),
        };
        assert_eq!(
            describe_plan(&plan),
            "Helldivers 2 · Rock, Metal, Electronic · not played in 6 months · 40 tracks"
        );
    }

    #[test]
    fn says_genres_the_librarys_way_never_the_models() {
        let plan = MixPlan {
            genres: strings(&["Hard Rock"]),
            names: vec![],
            rules: Some(set(
                vec![rule(RuleField::Genre, RuleOperator::Is, text("rock"))],
                None,
            )),
        };
        assert_eq!(describe_plan(&plan), "Hard Rock");
    }

    #[test]
    fn puts_the_rest_of_the_rules_into_words() {
        let plan = MixPlan {
            genres: vec![],
            names: vec![],
            rules: Some(set(
                vec![
                    rule(RuleField::PlayCount, RuleOperator::Is, RuleValue::Number(0)),
                    rule(RuleField::Liked, RuleOperator::Is, RuleValue::Flag(true)),
                    rule(
                        RuleField::Year,
                        RuleOperator::GreaterThan,
                        RuleValue::Number(1989),
                    ),
                    rule(
                        RuleField::Year,
                        RuleOperator::LessThan,
                        RuleValue::Number(2000),
                    ),
                    rule(
                        RuleField::LastPlayed,
                        RuleOperator::NotInTheLast,
                        text("-1y"),
                    ),
                    rule(RuleField::Artist, RuleOperator::Contains, text("Muse")),
                ],
                None,
            )),
        };
        assert_eq!(
            describe_plan(&plan),
            "never played · favourites · after 1989 · before 2000 · not played in a year · artist contains Muse"
        );
    }

    #[test]
    fn says_anything_when_nothing_was_understood() {
        assert_eq!(describe_plan(&MixPlan::default()), "anything");
        let plan = MixPlan {
            rules: Some(set(vec![], None)),
            ..MixPlan::default()
        };
        assert_eq!(describe_plan(&plan), "anything");
    }

    #[test]
    fn describes_names_alone() {
        let plan = MixPlan {
            names: strings(&["Muse", "Deftones"]),
            ..MixPlan::default()
        };
        assert_eq!(describe_plan(&plan), "Muse, Deftones");
    }

    #[test]
    fn album_artist_reads_as_two_words() {
        let plan = MixPlan {
            rules: Some(set(
                vec![rule(RuleField::AlbumArtist, RuleOperator::Is, text("Muse"))],
                None,
            )),
            ..MixPlan::default()
        };
        assert_eq!(describe_plan(&plan), "album artist is Muse");
    }

    #[test]
    fn names_what_the_library_had_nothing_for() {
        assert_eq!(
            missed_names_message(&strings(&["Helldivers 2", "Muse"])),
            "Nothing in your library mentions Helldivers 2, Muse."
        );
    }
}
