//! Smart playlist rules in the phone's own format, and checking the ones the
//! server's AI made from a mood.
//!
//! A mix is a rule set: describing a mood produces *rules*, the library and
//! the listening history select the tracks, and the result can be kept as a
//! smart playlist that syncs and evaluates the same on every Aoide app,
//! because this is byte for byte the JSON `playlist.smartRules` holds.
//!
//! Ported from the Electron app's `src/shared/aoide/smart-rules.ts`, itself
//! transcribed from the phone's `CurationKit/SmartPlaylist.swift`, including
//! the rule that file is most careful about: **an illegal field/operator pair
//! is refused, never quietly accepted.** A predicate that matches nothing
//! gives a smart playlist that is silently empty, which looks exactly like one
//! whose rules simply have no hits.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::json::whole_number;

/// What a rule tests. Text fields take text, number fields whole numbers,
/// date fields a time, and `liked` true or false; each kind has its own
/// operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleField {
    /// The album title (text).
    Album,
    /// The album artist (text).
    AlbumArtist,
    /// The track's artists (text).
    Artist,
    /// The track's length in milliseconds (number).
    DurationMs,
    /// When the listener first played it (date, from the local history).
    FirstPlayed,
    /// The track's genres (text).
    Genre,
    /// When the listener last played it (date, from the local history).
    LastPlayed,
    /// Liked by the listener (true or false).
    Liked,
    /// How many times the listener played it (number, from the local history).
    PlayCount,
    /// How many times the listener skipped it (number, from the local history).
    SkipCount,
    /// The track's title (text).
    Title,
    /// The release year (number).
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Date,
    Flag,
    Number,
    Text,
}

impl RuleField {
    const ALL: [RuleField; 12] = [
        Self::Album,
        Self::AlbumArtist,
        Self::Artist,
        Self::DurationMs,
        Self::FirstPlayed,
        Self::Genre,
        Self::LastPlayed,
        Self::Liked,
        Self::PlayCount,
        Self::SkipCount,
        Self::Title,
        Self::Year,
    ];

    /// The document's snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::AlbumArtist => "album_artist",
            Self::Artist => "artist",
            Self::DurationMs => "duration_ms",
            Self::FirstPlayed => "first_played",
            Self::Genre => "genre",
            Self::LastPlayed => "last_played",
            Self::Liked => "liked",
            Self::PlayCount => "play_count",
            Self::SkipCount => "skip_count",
            Self::Title => "title",
            Self::Year => "year",
        }
    }

    /// From the document's snake_case, or the camelCase the phone also
    /// accepts inbound. Emitted as snake_case only, the document's own
    /// spelling.
    pub fn parse(text: &str) -> Option<Self> {
        let alias = match text {
            "albumArtist" => Some(Self::AlbumArtist),
            "durationMs" => Some(Self::DurationMs),
            "firstPlayed" => Some(Self::FirstPlayed),
            "lastPlayed" => Some(Self::LastPlayed),
            "playCount" => Some(Self::PlayCount),
            "skipCount" => Some(Self::SkipCount),
            _ => None,
        };
        alias.or_else(|| Self::ALL.into_iter().find(|f| f.as_str() == text))
    }

    fn kind(self) -> Kind {
        match self {
            Self::Album | Self::AlbumArtist | Self::Artist | Self::Genre | Self::Title => {
                Kind::Text
            }
            Self::DurationMs | Self::PlayCount | Self::SkipCount | Self::Year => Kind::Number,
            Self::FirstPlayed | Self::LastPlayed => Kind::Date,
            Self::Liked => Kind::Flag,
        }
    }

    /// Whether the field describes listening (only the local history knows
    /// it) rather than the library (Melete answers it).
    pub fn is_history(self) -> bool {
        matches!(
            self,
            Self::FirstPlayed | Self::LastPlayed | Self::PlayCount | Self::SkipCount
        )
    }
}

/// How a rule compares its field with its value. Which operators a field
/// takes depends on its kind; any other pair is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleOperator {
    /// Dates: later than the value.
    After,
    /// Dates: earlier than the value.
    Before,
    /// Text: starts with the value.
    BeginsWith,
    /// Text: has the value in it.
    Contains,
    /// Text: doesn't have the value in it.
    DoesNotContain,
    /// Numbers: more than the value.
    GreaterThan,
    /// Dates: within a span back from now, such as `-30d`.
    InTheLast,
    /// Text, numbers and `liked`: equal to the value.
    Is,
    /// Text, numbers and `liked`: not equal to the value.
    IsNot,
    /// Numbers: less than the value.
    LessThan,
    /// Dates: not within a span back from now.
    NotInTheLast,
}

impl RuleOperator {
    const ALL: [RuleOperator; 11] = [
        Self::After,
        Self::Before,
        Self::BeginsWith,
        Self::Contains,
        Self::DoesNotContain,
        Self::GreaterThan,
        Self::InTheLast,
        Self::Is,
        Self::IsNot,
        Self::LessThan,
        Self::NotInTheLast,
    ];

    /// The camelCase spelling the rule document uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::After => "after",
            Self::Before => "before",
            Self::BeginsWith => "beginsWith",
            Self::Contains => "contains",
            Self::DoesNotContain => "doesNotContain",
            Self::GreaterThan => "greaterThan",
            Self::InTheLast => "inTheLast",
            Self::Is => "is",
            Self::IsNot => "isNot",
            Self::LessThan => "lessThan",
            Self::NotInTheLast => "notInTheLast",
        }
    }

    /// The operator spelled `text`, exactly as [`as_str`](Self::as_str) spells
    /// it; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == text)
    }
}

/// The operators a kind of field takes, which is the whole of what decides
/// legality.
fn operators(kind: Kind) -> &'static [RuleOperator] {
    use RuleOperator::*;
    match kind {
        Kind::Date => &[Before, After, InTheLast, NotInTheLast],
        Kind::Flag => &[Is, IsNot],
        Kind::Number => &[Is, IsNot, GreaterThan, LessThan],
        Kind::Text => &[Is, IsNot, Contains, DoesNotContain, BeginsWith],
    }
}

/// What a rule compares with, as a bare JSON value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RuleValue {
    /// For `liked`.
    Flag(bool),
    /// For number fields, and for a date as milliseconds since the Unix epoch.
    Number(i64),
    /// For text fields, and for a date as a span back from now: `-30d`, `6m`,
    /// `2y` (days, weeks, months, years).
    Text(String),
}

impl RuleValue {
    fn to_json(&self) -> Value {
        match self {
            Self::Flag(b) => Value::Bool(*b),
            Self::Number(n) => Value::from(*n),
            Self::Text(t) => Value::String(t.clone()),
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Flag(b) => b.to_string(),
            Self::Number(n) => n.to_string(),
            Self::Text(t) => t.clone(),
        }
    }
}

/// One rule. Fields in alphabetical order so it serializes as the phone's
/// `JSONEncoder(.sortedKeys)` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmartRule {
    /// What is tested.
    pub field: RuleField,
    /// How it is compared.
    pub op: RuleOperator,
    /// What it is compared with.
    pub value: RuleValue,
}

impl SmartRule {
    /// A rule both apps can evaluate, or the reason it is not one.
    ///
    /// # Errors
    ///
    /// The reason, as [`rule_problem`] words it, when `op` doesn't apply to
    /// `field` or `value` is the wrong kind for it.
    pub fn new(field: RuleField, op: RuleOperator, value: RuleValue) -> Result<Self, String> {
        let rule = SmartRule { field, op, value };
        match rule_problem(&rule) {
            Some(problem) => Err(problem),
            None => Ok(rule),
        }
    }

    /// The value as text, as the Electron app prints it in a summary.
    pub(crate) fn value_text(&self) -> String {
        self.value.display()
    }
}

/// How a rule set combines its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Match {
    /// A track must pass every rule. The default.
    #[default]
    All,
    /// A track must pass at least one rule.
    Any,
}

/// Which way a [`RuleSort`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    /// Smallest, earliest or A first.
    Asc,
    /// Largest, latest or Z first.
    Desc,
}

/// The order a rule set's tracks are listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSort {
    /// Which way.
    pub dir: SortDirection,
    /// By what.
    pub field: RuleField,
}

/// A rule set: `playlist.smartRules`. Serializes with sorted keys, as the
/// phone writes it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SmartRules {
    /// The most tracks the playlist holds; `None` for no cap.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub limit: Option<u32>,
    /// Whether a track must pass every rule or any one.
    #[serde(rename = "match")]
    pub match_: Match,
    /// The rules. None at all, with [`Match::All`], is the whole library.
    pub rules: Vec<SmartRule>,
    /// The order to list them in; `None` when the rules give none.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sort: Option<RuleSort>,
}

/// Whether a rule is one both apps can evaluate. A reason rather than a
/// boolean, so a model that produced `duration_ms beginsWith 3` is told.
pub fn rule_problem(rule: &SmartRule) -> Option<String> {
    problem(rule.field, rule.op.as_str(), &rule.value.to_json())
}

/// A relative span, as `inTheLast` takes it: `-30d`, `6m`, `2y`.
fn is_span(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let Some(unit) = digits.chars().last() else {
        return false;
    };
    let number = &digits[..digits.len() - unit.len_utf8()];
    matches!(unit, 'd' | 'w' | 'm' | 'y')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

fn problem(field: RuleField, op: &str, value: &Value) -> Option<String> {
    let name = field.as_str();
    let kind = field.kind();
    let legal = RuleOperator::parse(op).filter(|op| operators(kind).contains(op));
    let Some(op) = legal else {
        return Some(format!("{op} does not apply to {name}"));
    };
    match kind {
        Kind::Text if !value.is_string() => Some(format!("{name} needs text")),
        // Integers only: every numeric field counts something or is a
        // millisecond, and 2.5 would mean deciding which way to round a bound.
        Kind::Number if whole_number(value).is_none() => {
            Some(format!("{name} needs a whole number"))
        }
        Kind::Flag if !value.is_boolean() => Some(format!("{name} needs true or false")),
        Kind::Date => {
            // `before`/`after` name an instant, relative or an absolute epoch
            // millisecond; `inTheLast` names a span, and a bare number would
            // be a span in unstated units.
            if let Some(text) = value.as_str() {
                return (!is_span(text)).then(|| format!("{text} is not a span like -30d"));
            }
            if matches!(op, RuleOperator::InTheLast | RuleOperator::NotInTheLast) {
                return Some(format!("{} needs a span like -30d", op.as_str()));
            }
            whole_number(value)
                .is_none()
                .then(|| format!("{name} needs a time"))
        }
        _ => None,
    }
}

/// A mix's rules, read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedRules {
    /// Artists, albums, bands, games, films or shows the description named,
    /// as the model spelled them: the half of a description a rule cannot
    /// express ("Helldivers 2 session music"). Empty, never absent.
    pub names: Vec<String>,
    /// Rules that were dropped, and why. Shown rather than swallowed.
    pub rejected: Vec<String>,
    /// The rule set, with only the legal rules; `None` when there was no
    /// rule set to read.
    pub rules: Option<SmartRules>,
}

/// A name shorter than this is punctuation or a stray letter.
const MIN_NAME_LENGTH: usize = 2;

/// Trimmed, at least two characters, each once.
fn read_names(names: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for name in names.iter().map(|name| name.trim()) {
        if name.chars().count() >= MIN_NAME_LENGTH && !kept.iter().any(|n| n == name) {
            kept.push(name.to_owned());
        }
    }
    kept
}

fn describe(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// A mix as the server sent it (`names`, the rule set in the phone's format,
/// and the rules it `rejected`), checked again here: the server's checks are
/// a server's, and these rules run on this computer. Only the rules both
/// apps evaluate are kept; each one left out joins the server's reasons.
pub fn read_mix(names: &[String], rule_set: Option<&Value>, rejected: &[String]) -> ParsedRules {
    let mut rejected = rejected.to_vec();
    let rules = rule_set.and_then(|set| read_rule_set(set, &mut rejected));
    ParsedRules {
        names: read_names(names),
        rejected,
        rules,
    }
}

/// The rule set with its legal rules, the reason for each other one added to
/// `rejected`.
///
/// `any` of nothing can only ever be empty, and an always-empty playlist is
/// indistinguishable from a broken one, so it is refused, as the phone
/// refuses it. `all` of nothing is every track, which is a real playlist.
fn read_rule_set(set: &Value, rejected: &mut Vec<String>) -> Option<SmartRules> {
    let Some(set) = set.as_object() else {
        rejected.push("The server's rules couldn't be read.".into());
        return None;
    };
    let rules: Vec<SmartRule> = set
        .get("rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| read_rule(candidate, rejected))
        .collect();
    let match_ = if set.get("match").and_then(Value::as_str) == Some("any") {
        Match::Any
    } else {
        Match::All
    };
    if match_ == Match::Any && rules.is_empty() {
        rejected.push("No usable rules.".into());
        return None;
    }
    // Null, the server's "no cap", is no limit too.
    let limit = set
        .get("limit")
        .and_then(whole_number)
        .filter(|&n| n > 0)
        .and_then(|n| u32::try_from(n).ok());
    Some(SmartRules {
        limit,
        match_,
        rules,
        sort: None,
    })
}

/// One rule, or `None` with the reason added to `rejected`.
fn read_rule(candidate: &Value, rejected: &mut Vec<String>) -> Option<SmartRule> {
    let raw_field = candidate.get("field");
    let Some(field) = raw_field.and_then(Value::as_str).and_then(RuleField::parse) else {
        rejected.push(format!("Unknown field {}", describe(raw_field)));
        return None;
    };
    let op_text = describe(candidate.get("op"));
    let value = candidate.get("value").cloned().unwrap_or(Value::Null);
    if let Some(reason) = problem(field, &op_text, &value) {
        rejected.push(reason);
        return None;
    }
    let op = RuleOperator::parse(&op_text)?;
    let value = match &value {
        Value::Bool(b) => RuleValue::Flag(*b),
        Value::String(s) => RuleValue::Text(s.clone()),
        other => RuleValue::Number(whole_number(other)?),
    };
    Some(SmartRule { field, op, value })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(field: RuleField, op: &str, value: Value) -> Option<String> {
        problem(field, op, &value)
    }

    // Which rules both apps can evaluate.

    #[test]
    fn refuses_an_operator_that_does_not_apply_to_the_field() {
        let p = check(RuleField::DurationMs, "beginsWith", json!(3)).unwrap();
        assert!(p.contains("does not apply"), "{p}");
        let p = check(RuleField::Title, "inTheLast", json!("-30d")).unwrap();
        assert!(p.contains("does not apply"), "{p}");
    }

    #[test]
    fn accepts_the_pairs_the_phone_accepts() {
        assert_eq!(check(RuleField::Genre, "contains", json!("Jaz")), None);
        assert_eq!(check(RuleField::PlayCount, "greaterThan", json!(3)), None);
        assert_eq!(
            check(RuleField::LastPlayed, "notInTheLast", json!("-6m")),
            None
        );
        assert_eq!(check(RuleField::Liked, "is", json!(true)), None);
    }

    #[test]
    fn insists_a_number_field_gets_a_whole_number() {
        let p = check(RuleField::PlayCount, "greaterThan", json!(2.5)).unwrap();
        assert!(p.contains("whole number"), "{p}");
    }

    #[test]
    fn insists_a_span_is_a_span() {
        let p = check(RuleField::LastPlayed, "inTheLast", json!(30)).unwrap();
        assert!(p.contains("span"), "{p}");
        let p = check(RuleField::LastPlayed, "inTheLast", json!("ages")).unwrap();
        assert!(p.contains("span"), "{p}");
    }

    #[test]
    fn lets_before_and_after_name_an_instant_as_well_as_a_span() {
        assert_eq!(
            check(RuleField::LastPlayed, "before", json!(1_700_000_000_000i64)),
            None
        );
        assert_eq!(check(RuleField::LastPlayed, "after", json!("-1y")), None);
    }

    #[test]
    fn insists_a_flag_is_a_flag() {
        let p = check(RuleField::Liked, "is", json!("yes")).unwrap();
        assert!(p.contains("true or false"), "{p}");
    }

    #[test]
    fn typed_rules_are_checked_too() {
        assert!(
            SmartRule::new(
                RuleField::Year,
                RuleOperator::Contains,
                RuleValue::Number(1990)
            )
            .is_err()
        );
        assert!(
            SmartRule::new(
                RuleField::Year,
                RuleOperator::GreaterThan,
                RuleValue::Number(1989)
            )
            .is_ok()
        );
    }

    #[test]
    fn spans_are_digits_and_a_unit() {
        for good in ["-30d", "6m", "2y", "-1w", "0d"] {
            assert!(is_span(good), "{good}");
        }
        for bad in [
            "", "-", "d", "-d", "30", "30x", "3.5d", "--3d", "3dd", "-30D",
        ] {
            assert!(!is_span(bad), "{bad}");
        }
    }

    // Reading the server's rules.

    fn read(set: Value) -> ParsedRules {
        read_mix(&[], Some(&set), &[])
    }

    fn named(names: &[&str], set: Value) -> ParsedRules {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        read_mix(&names, Some(&set), &[])
    }

    #[test]
    fn keeps_the_rules_that_work_and_says_why_it_dropped_the_rest() {
        let parsed = read(json!({"match": "all", "rules": [
            {"field": "genre", "op": "is", "value": "Jazz"},
            {"field": "title", "op": "inTheLast", "value": "-30d"},
        ]}));
        assert_eq!(parsed.rules.unwrap().rules.len(), 1);
        assert_eq!(parsed.rejected.len(), 1);
    }

    #[test]
    fn the_servers_reasons_come_first_then_the_apps_own() {
        let set = json!({"match": "all", "limit": null, "rules": [
            {"field": "duration_ms", "op": "beginsWith", "value": 3},
        ]});
        let parsed = read_mix(&[], Some(&set), &["mood is not a field".to_owned()]);
        assert_eq!(
            parsed.rejected,
            [
                "mood is not a field",
                "beginsWith does not apply to duration_ms"
            ]
        );
        assert!(parsed.rules.unwrap().rules.is_empty());
    }

    #[test]
    fn no_rule_set_keeps_the_servers_reasons() {
        let parsed = read_mix(
            &["Helldivers 2".to_owned()],
            None,
            &["No usable rules.".to_owned()],
        );
        assert_eq!(parsed.rules, None);
        assert_eq!(parsed.rejected, ["No usable rules."]);
        assert_eq!(parsed.names, ["Helldivers 2"]);
    }

    #[test]
    fn a_rule_set_that_is_not_an_object_is_refused() {
        let parsed = read(json!([{"field": "genre", "op": "is", "value": "Jazz"}]));
        assert_eq!(parsed.rules, None);
        assert_eq!(parsed.rejected, ["The server's rules couldn't be read."]);
    }

    #[test]
    fn accepts_the_camel_case_spellings_the_phone_also_accepts() {
        let parsed = read(
            json!({"match": "all", "rules": [{"field": "playCount", "op": "greaterThan", "value": 3}]}),
        );
        let rules = parsed.rules.unwrap();
        assert_eq!(rules.rules[0].field, RuleField::PlayCount);
        assert!(
            serde_json::to_string(&rules)
                .unwrap()
                .contains("\"play_count\"")
        );
    }

    #[test]
    fn refuses_any_with_no_usable_rules() {
        let parsed = read(json!({"match": "any", "rules": []}));
        assert_eq!(parsed.rules, None);
        assert_eq!(parsed.rejected, ["No usable rules."]);
    }

    #[test]
    fn allows_all_with_no_rules_which_is_the_whole_library() {
        let parsed = read(json!({"limit": 25, "match": "all", "rules": []}));
        assert_eq!(parsed.rules.unwrap().limit, Some(25));
    }

    #[test]
    fn ignores_a_limit_that_is_not_a_positive_whole_number() {
        for limit in [json!(null), json!(0), json!(2.5), json!(-3), json!("25")] {
            let parsed = read(json!({"limit": limit, "match": "all", "rules": []}));
            assert_eq!(parsed.rules.unwrap().limit, None, "{limit}");
        }
    }

    #[test]
    fn no_limit_is_left_out_of_the_stored_rules() {
        let rules = read(json!({"limit": null, "match": "all", "rules": []}))
            .rules
            .unwrap();
        assert_eq!(
            serde_json::to_string(&rules).unwrap(),
            r#"{"match":"all","rules":[]}"#
        );
    }

    #[test]
    fn unknown_and_missing_fields_are_named_in_the_refusal() {
        let parsed = read(
            json!({"match": "all", "rules": [{"field": "mood", "op": "is", "value": "x"}, {"op": "is"}, 7]}),
        );
        assert_eq!(
            parsed.rejected,
            [
                "Unknown field mood",
                "Unknown field undefined",
                "Unknown field undefined"
            ]
        );
    }

    // The things a description names.

    #[test]
    fn names_come_back_spelled_as_the_model_wrote_them() {
        let parsed = named(
            &["Helldivers 2", "Nine Inch Nails"],
            json!({"match": "all", "rules": []}),
        );
        assert_eq!(parsed.names, ["Helldivers 2", "Nine Inch Nails"]);
    }

    #[test]
    fn names_are_an_empty_list_when_there_are_none() {
        assert!(read(json!({"match": "all", "rules": []})).names.is_empty());
        assert!(read_mix(&[], None, &[]).names.is_empty());
    }

    #[test]
    fn names_survive_the_rules_being_refused() {
        let parsed = named(&["Helldivers 2"], json!({"match": "any", "rules": []}));
        assert_eq!(parsed.rules, None);
        assert_eq!(parsed.names, ["Helldivers 2"]);
    }

    #[test]
    fn names_drop_blanks_single_characters_and_repeats() {
        let parsed = named(
            &[" Muse ", "", "x", "Muse"],
            json!({"match": "all", "rules": []}),
        );
        assert_eq!(parsed.names, ["Muse"]);
    }

    // Rules the phone produces: the cross-app contract.

    /// The exact JSON `MixComposer` encodes, generated by running Swift over
    /// the rule set (as the Electron app's test has it).
    const FROM_THE_PHONE: &str = concat!(
        r#"{"limit":40,"match":"all","rules":["#,
        r#"{"field":"genre","op":"is","value":"Rock"},"#,
        r#"{"field":"year","op":"greaterThan","value":1989},"#,
        r#"{"field":"year","op":"lessThan","value":2000},"#,
        r#"{"field":"last_played","op":"notInTheLast","value":"-6m"},"#,
        r#"{"field":"liked","op":"is","value":true}]}"#
    );

    fn from_the_phone() -> ParsedRules {
        read(serde_json::from_str(FROM_THE_PHONE).unwrap())
    }

    #[test]
    fn parses_the_phones_rules_without_dropping_one() {
        let parsed = from_the_phone();
        assert!(parsed.rejected.is_empty());
        let rules = parsed.rules.unwrap();
        assert_eq!(rules.rules.len(), 5);
        assert_eq!(rules.limit, Some(40));
        assert_eq!(rules.match_, Match::All);
    }

    #[test]
    fn keeps_every_value_at_the_type_the_phone_encoded_it_as() {
        let rules = from_the_phone().rules.unwrap().rules;
        let value =
            |f: &dyn Fn(&SmartRule) -> bool| rules.iter().find(|r| f(r)).unwrap().value.clone();
        assert_eq!(
            value(&|r| r.field == RuleField::Genre),
            RuleValue::Text("Rock".into())
        );
        assert_eq!(
            value(&|r| r.field == RuleField::Liked),
            RuleValue::Flag(true)
        );
        assert_eq!(
            value(&|r| r.op == RuleOperator::GreaterThan),
            RuleValue::Number(1989)
        );
        assert_eq!(
            value(&|r| r.field == RuleField::LastPlayed),
            RuleValue::Text("-6m".into())
        );
    }

    #[test]
    fn writes_the_phones_rules_back_byte_for_byte() {
        let rules = from_the_phone().rules.unwrap();
        assert_eq!(serde_json::to_string(&rules).unwrap(), FROM_THE_PHONE);
        let back: SmartRules = serde_json::from_str(FROM_THE_PHONE).unwrap();
        assert_eq!(back, rules);
    }

    #[test]
    fn a_sort_round_trips() {
        let json = r#"{"match":"any","rules":[{"field":"liked","op":"is","value":true}],"sort":{"dir":"desc","field":"play_count"}}"#;
        let rules: SmartRules = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&rules).unwrap(), json);
    }

    #[test]
    fn history_fields_are_the_four_listening_ones() {
        let history: Vec<_> = RuleField::ALL
            .into_iter()
            .filter(|f| f.is_history())
            .collect();
        assert_eq!(
            history,
            [
                RuleField::FirstPlayed,
                RuleField::LastPlayed,
                RuleField::PlayCount,
                RuleField::SkipCount
            ]
        );
    }

    #[test]
    fn rule_values_display_as_the_electron_app_prints_them() {
        assert_eq!(RuleValue::Flag(true).display(), "true");
        assert_eq!(RuleValue::Number(3).display(), "3");
        assert_eq!(RuleValue::Text("Muse".into()).display(), "Muse");
    }
}
