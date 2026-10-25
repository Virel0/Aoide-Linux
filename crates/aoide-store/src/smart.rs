//! Smart playlists: rules stored as JSON on the playlist, evaluated locally in SQL.
//!
//! An illegal field/operator pair cannot be constructed, from code or from JSON: the
//! cheap implementation of "unsupported" is a predicate that matches nothing, and a
//! smart playlist that is quietly empty looks exactly like one whose rules have no
//! hits. Every rule *value* is a bound parameter and every field and operator maps to
//! SQL through a `match` over a closed enum, so nothing typed reaches SQLite as source.

use chrono::{DateTime, Days, Local, Months, NaiveDateTime, TimeZone, Utc};
use rusqlite::params_from_iter;
use rusqlite::types::Value as SqlValue;
use serde_json::{Map, Value as Json};

use crate::Millis;
use crate::error::Result;
use crate::json::canonical_json;
use crate::models::Playlist;
use crate::play_definition::PlayDefinition;
use crate::store::Store;

// MARK: - Rule model

/// The columns a rule may talk about. The wire spellings are the design document's
/// snake_case; camelCase is accepted on the way in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    DurationMs,
    PlayCount,
    SkipCount,
    LastPlayed,
    FirstPlayed,
    Liked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Date,
    Flag,
}

impl Field {
    pub const ALL: [Field; 12] = [
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::AlbumArtist,
        Self::Genre,
        Self::Year,
        Self::DurationMs,
        Self::PlayCount,
        Self::SkipCount,
        Self::LastPlayed,
        Self::FirstPlayed,
        Self::Liked,
    ];

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::AlbumArtist => "album_artist",
            Self::Genre => "genre",
            Self::Year => "year",
            Self::DurationMs => "duration_ms",
            Self::PlayCount => "play_count",
            Self::SkipCount => "skip_count",
            Self::LastPlayed => "last_played",
            Self::FirstPlayed => "first_played",
            Self::Liked => "liked",
        }
    }

    pub fn parse(wire: &str) -> std::result::Result<Self, SmartPlaylistError> {
        let alias = match wire {
            "albumArtist" => Some(Self::AlbumArtist),
            "durationMs" => Some(Self::DurationMs),
            "playCount" => Some(Self::PlayCount),
            "skipCount" => Some(Self::SkipCount),
            "lastPlayed" => Some(Self::LastPlayed),
            "firstPlayed" => Some(Self::FirstPlayed),
            _ => None,
        };
        Self::ALL
            .into_iter()
            .find(|field| field.wire_name() == wire)
            .or(alias)
            .ok_or_else(|| SmartPlaylistError::UnknownField(wire.to_owned()))
    }

    fn kind(self) -> Kind {
        match self {
            Self::Title | Self::Artist | Self::Album | Self::AlbumArtist | Self::Genre => {
                Kind::Text
            }
            Self::Year | Self::DurationMs | Self::PlayCount | Self::SkipCount => Kind::Number,
            Self::LastPlayed | Self::FirstPlayed => Kind::Date,
            Self::Liked => Kind::Flag,
        }
    }
}

/// The closed set of comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operator {
    Is,
    IsNot,
    Contains,
    DoesNotContain,
    BeginsWith,
    GreaterThan,
    LessThan,
    Before,
    After,
    InTheLast,
    NotInTheLast,
}

impl Operator {
    pub const ALL: [Operator; 11] = [
        Self::Is,
        Self::IsNot,
        Self::Contains,
        Self::DoesNotContain,
        Self::BeginsWith,
        Self::GreaterThan,
        Self::LessThan,
        Self::Before,
        Self::After,
        Self::InTheLast,
        Self::NotInTheLast,
    ];

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Is => "is",
            Self::IsNot => "isNot",
            Self::Contains => "contains",
            Self::DoesNotContain => "doesNotContain",
            Self::BeginsWith => "beginsWith",
            Self::GreaterThan => "greaterThan",
            Self::LessThan => "lessThan",
            Self::Before => "before",
            Self::After => "after",
            Self::InTheLast => "inTheLast",
            Self::NotInTheLast => "notInTheLast",
        }
    }

    /// A name, or a symbol and how far it shifts the bound: `>=` and `<=` translate
    /// exactly, because every comparable field here is integral.
    fn resolve(wire: &str) -> std::result::Result<(Self, i64), SmartPlaylistError> {
        if let Some(op) = Self::ALL.into_iter().find(|op| op.wire_name() == wire) {
            return Ok((op, 0));
        }
        match wire {
            "=" | "==" => Ok((Self::Is, 0)),
            "!=" | "<>" => Ok((Self::IsNot, 0)),
            ">" => Ok((Self::GreaterThan, 0)),
            "<" => Ok((Self::LessThan, 0)),
            ">=" => Ok((Self::GreaterThan, -1)),
            "<=" => Ok((Self::LessThan, 1)),
            _ => Err(SmartPlaylistError::UnknownOperator(wire.to_owned())),
        }
    }
}

/// A rule value: text, a whole number, or a flag. Never a float — every numeric field
/// counts something or is a millisecond.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    Text(String),
    Number(i64),
    Flag(bool),
}

impl Value {
    fn to_json(&self) -> Json {
        match self {
            Self::Text(text) => Json::String(text.clone()),
            Self::Number(number) => Json::from(*number),
            Self::Flag(flag) => Json::Bool(*flag),
        }
    }

    fn from_json(value: &Json) -> std::result::Result<Self, SmartPlaylistError> {
        match value {
            Json::Bool(flag) => Ok(Self::Flag(*flag)),
            Json::Number(number) => number
                .as_i64()
                .map(Self::Number)
                .ok_or(SmartPlaylistError::MalformedValue),
            Json::String(text) => Ok(Self::Text(text.clone())),
            _ => Err(SmartPlaylistError::MalformedValue),
        }
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<i64> for Value {
    fn from(number: i64) -> Self {
        Self::Number(number)
    }
}

impl From<i32> for Value {
    fn from(number: i32) -> Self {
        Self::Number(i64::from(number))
    }
}

impl From<bool> for Value {
    fn from(flag: bool) -> Self {
        Self::Flag(flag)
    }
}

/// One clause: a field, an operator, and a value to compare against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SmartRule {
    pub field: Field,
    pub op: Operator,
    pub value: Value,
}

impl SmartRule {
    pub fn new(
        field: Field,
        op: Operator,
        value: impl Into<Value>,
    ) -> std::result::Result<Self, SmartPlaylistError> {
        let value = value.into();
        check(field, op, &value)?;
        Ok(Self { field, op, value })
    }

    fn to_json(&self) -> Json {
        let mut map = Map::new();
        map.insert(
            "field".to_owned(),
            Json::String(self.field.wire_name().to_owned()),
        );
        map.insert(
            "op".to_owned(),
            Json::String(self.op.wire_name().to_owned()),
        );
        map.insert("value".to_owned(), self.value.to_json());
        Json::Object(map)
    }

    fn from_json(value: &Json) -> std::result::Result<Self, SmartPlaylistError> {
        let object = value
            .as_object()
            .ok_or_else(|| malformed("a rule must be an object"))?;
        let field = Field::parse(required_str(object, "field")?)?;
        let (op, bias) = Operator::resolve(required_str(object, "op")?)?;
        let mut value = Value::from_json(
            object
                .get("value")
                .ok_or_else(|| malformed("a rule needs a value"))?,
        )?;
        if bias != 0 {
            let Value::Number(bound) = value else {
                return Err(SmartPlaylistError::ValueTypeMismatch { field, op });
            };
            value = Value::Number(bound + bias);
        }
        Self::new(field, op, value)
    }
}

fn check(field: Field, op: Operator, value: &Value) -> std::result::Result<(), SmartPlaylistError> {
    let require = |allowed: &[Operator]| {
        if allowed.contains(&op) {
            Ok(())
        } else {
            Err(SmartPlaylistError::OperatorNotApplicable { field, op })
        }
    };
    let mismatch = SmartPlaylistError::ValueTypeMismatch { field, op };
    match field.kind() {
        Kind::Text => {
            require(&[
                Operator::Is,
                Operator::IsNot,
                Operator::Contains,
                Operator::DoesNotContain,
                Operator::BeginsWith,
            ])?;
            if !matches!(value, Value::Text(_)) {
                return Err(mismatch);
            }
        }
        Kind::Number => {
            require(&[
                Operator::Is,
                Operator::IsNot,
                Operator::GreaterThan,
                Operator::LessThan,
            ])?;
            if !matches!(value, Value::Number(_)) {
                return Err(mismatch);
            }
        }
        Kind::Date => {
            require(&[
                Operator::Before,
                Operator::After,
                Operator::InTheLast,
                Operator::NotInTheLast,
            ])?;
            match (op, value) {
                // `before`/`after` name an instant, relative or as an epoch millisecond;
                // `inTheLast` names a span, and a bare number is a span in no units.
                (Operator::Before | Operator::After, Value::Number(_)) => {}
                (_, Value::Text(written)) => {
                    RelativeInterval::parse(written)?;
                }
                _ => return Err(mismatch),
            }
        }
        Kind::Flag => {
            require(&[Operator::Is, Operator::IsNot])?;
            if !matches!(value, Value::Flag(_)) {
                return Err(mismatch);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Match {
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sort {
    pub field: Field,
    pub dir: SortDirection,
}

impl Sort {
    pub fn new(field: Field, dir: SortDirection) -> Self {
        Self { field, dir }
    }
}

/// The rule set stored as JSON in `playlist.smartRules`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SmartRules {
    pub match_mode: Match,
    pub rules: Vec<SmartRule>,
    pub sort: Option<Sort>,
    pub limit: Option<i64>,
}

impl SmartRules {
    /// `all` of nothing is every track, a real playlist; `any` of nothing can only be
    /// empty, and an always-empty playlist looks broken, so it is refused.
    pub fn new(
        match_mode: Match,
        rules: Vec<SmartRule>,
        sort: Option<Sort>,
        limit: Option<i64>,
    ) -> std::result::Result<Self, SmartPlaylistError> {
        if match_mode == Match::Any && rules.is_empty() {
            return Err(SmartPlaylistError::NoRules);
        }
        if let Some(limit) = limit
            && limit <= 0
        {
            return Err(SmartPlaylistError::NonPositiveLimit(limit));
        }
        Ok(Self {
            match_mode,
            rules,
            sort,
            limit,
        })
    }

    /// Every track (`all` of no rules).
    pub fn everything() -> Self {
        Self {
            match_mode: Match::All,
            rules: Vec::new(),
            sort: None,
            limit: None,
        }
    }

    pub fn from_json(json: &str) -> std::result::Result<Self, SmartPlaylistError> {
        let parsed: Json =
            serde_json::from_str(json).map_err(|error| malformed(&error.to_string()))?;
        let object = parsed
            .as_object()
            .ok_or_else(|| malformed("a rule set must be an object"))?;
        let match_mode = match required_str(object, "match")? {
            "all" => Match::All,
            "any" => Match::Any,
            other => return Err(malformed(&format!("'{other}' is not a match mode"))),
        };
        let rules = object
            .get("rules")
            .and_then(Json::as_array)
            .ok_or_else(|| malformed("a rule set needs a rules array"))?
            .iter()
            .map(SmartRule::from_json)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let sort = match object.get("sort") {
            None | Some(Json::Null) => None,
            Some(sort) => {
                let sort = sort
                    .as_object()
                    .ok_or_else(|| malformed("sort must be an object"))?;
                let field = Field::parse(required_str(sort, "field")?)?;
                let dir = match required_str(sort, "dir")? {
                    "asc" => SortDirection::Asc,
                    "desc" => SortDirection::Desc,
                    other => return Err(malformed(&format!("'{other}' is not a sort direction"))),
                };
                Some(Sort { field, dir })
            }
        };
        let limit = match object.get("limit") {
            None | Some(Json::Null) => None,
            Some(limit) => Some(
                limit
                    .as_i64()
                    .ok_or_else(|| malformed("limit must be a whole number"))?,
            ),
        };
        Self::new(match_mode, rules, sort, limit)
    }

    /// Sorted keys, for the same reason op payloads have them: an unchanged rule set
    /// must not look edited.
    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        map.insert(
            "match".to_owned(),
            Json::String(
                if self.match_mode == Match::All {
                    "all"
                } else {
                    "any"
                }
                .to_owned(),
            ),
        );
        map.insert(
            "rules".to_owned(),
            Json::Array(self.rules.iter().map(SmartRule::to_json).collect()),
        );
        if let Some(sort) = self.sort {
            let mut sort_map = Map::new();
            sort_map.insert(
                "dir".to_owned(),
                Json::String(
                    if sort.dir == SortDirection::Asc {
                        "asc"
                    } else {
                        "desc"
                    }
                    .to_owned(),
                ),
            );
            sort_map.insert(
                "field".to_owned(),
                Json::String(sort.field.wire_name().to_owned()),
            );
            map.insert("sort".to_owned(), Json::Object(sort_map));
        }
        if let Some(limit) = self.limit {
            map.insert("limit".to_owned(), Json::from(limit));
        }
        canonical_json(&Json::Object(map))
    }
}

fn required_str<'a>(
    object: &'a Map<String, Json>,
    key: &str,
) -> std::result::Result<&'a str, SmartPlaylistError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| malformed(&format!("'{key}' must be a string")))
}

fn malformed(detail: &str) -> SmartPlaylistError {
    SmartPlaylistError::MalformedJson(detail.to_owned())
}

// MARK: - Relative dates

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeUnit {
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

/// A span written the way rules store it: `-30d`, `-6m`, `1y`. Kept relative until
/// evaluation, so "not played in a month" keeps moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelativeInterval {
    pub amount: u32,
    pub unit: TimeUnit,
}

/// Which calendar month and year arithmetic happens in. A parameter because the answer
/// depends on a time zone: a test using the machine's would pass in one city and fail
/// in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Calendar {
    Utc,
    Local,
}

impl RelativeInterval {
    /// Parses `-30d` or `30d`: the leading minus is decoration, since every operator
    /// already looks backwards.
    pub fn parse(written: &str) -> std::result::Result<Self, SmartPlaylistError> {
        let refuse = || SmartPlaylistError::MalformedInterval(written.to_owned());
        let lowered = written.trim_matches([' ', '\t']).to_lowercase();
        let body = lowered.strip_prefix('-').unwrap_or(&lowered);
        let unit = match body.chars().last() {
            Some('h') => TimeUnit::Hours,
            Some('d') => TimeUnit::Days,
            Some('w') => TimeUnit::Weeks,
            Some('m') => TimeUnit::Months,
            Some('y') => TimeUnit::Years,
            _ => return Err(refuse()),
        };
        let digits = &body[..body.len() - 1];
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(refuse());
        }
        let amount = digits.parse().map_err(|_| refuse())?;
        Ok(Self { amount, unit })
    }

    /// The instant this far before `now`.
    ///
    /// Calendar arithmetic rather than multiples of a day: a month before 31 March is
    /// the end of February. Hours are an absolute shift; days, weeks, months and years
    /// keep the wall-clock time in `calendar`.
    pub fn ago(&self, now: Millis, calendar: Calendar) -> Millis {
        if self.unit == TimeUnit::Hours {
            return now.saturating_sub(i64::from(self.amount) * 3_600_000);
        }
        let Some(instant) = DateTime::<Utc>::from_timestamp_millis(now) else {
            return now;
        };
        match calendar {
            Calendar::Utc => self
                .shift(instant.naive_utc())
                .map_or(now, |naive| naive.and_utc().timestamp_millis()),
            Calendar::Local => {
                let local = instant.with_timezone(&Local).naive_local();
                self.shift(local)
                    .and_then(resolve_local)
                    .map_or(now, |resolved| resolved.timestamp_millis())
            }
        }
    }

    fn shift(&self, naive: NaiveDateTime) -> Option<NaiveDateTime> {
        let amount = self.amount;
        match self.unit {
            TimeUnit::Hours => None,
            TimeUnit::Days => naive.checked_sub_days(Days::new(u64::from(amount))),
            TimeUnit::Weeks => naive.checked_sub_days(Days::new(u64::from(amount) * 7)),
            TimeUnit::Months => naive.checked_sub_months(Months::new(amount)),
            TimeUnit::Years => naive.checked_sub_months(Months::new(amount.checked_mul(12)?)),
        }
    }
}

/// A local wall-clock time to an instant: the earlier reading of an ambiguous hour,
/// and the first valid time after a skipped one.
pub(crate) fn resolve_local(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    if let Some(resolved) = Local.from_local_datetime(&naive).earliest() {
        return Some(resolved);
    }
    Local
        .from_local_datetime(&(naive + chrono::Duration::hours(1)))
        .earliest()
}

// MARK: - Evaluation

impl Store {
    /// The Jellyfin ids matching `rules`, in the rule set's own order, as of `now`.
    pub fn evaluate(
        &self,
        rules: &SmartRules,
        now: Millis,
        calendar: Calendar,
    ) -> Result<Vec<String>> {
        let query = SmartQuery::new(rules, now, calendar)?;
        self.read(|conn| {
            let mut statement = conn.prepare(&query.sql)?;
            let rows =
                statement.query_map(params_from_iter(query.arguments.iter()), |row| row.get(0))?;
            rows.collect()
        })
    }

    /// How many tracks `rules` match, from the same compiled statement — `LIMIT`
    /// included — so the count cannot drift from `evaluate`.
    pub fn match_count(&self, rules: &SmartRules, now: Millis, calendar: Calendar) -> Result<i64> {
        let query = SmartQuery::new(rules, now, calendar)?;
        self.read(|conn| {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM ({})", query.sql),
                params_from_iter(query.arguments.iter()),
                |row| row.get(0),
            )
        })
    }

    /// Evaluates a playlist's stored rules; a playlist with none is an error, not an
    /// empty list.
    pub fn resolve_smart(
        &self,
        playlist: &Playlist,
        now: Millis,
        calendar: Calendar,
    ) -> Result<Vec<String>> {
        self.evaluate(&rules_of(playlist)?, now, calendar)
    }

    pub fn match_count_of(
        &self,
        playlist: &Playlist,
        now: Millis,
        calendar: Calendar,
    ) -> Result<i64> {
        self.match_count(&rules_of(playlist)?, now, calendar)
    }
}

fn rules_of(playlist: &Playlist) -> Result<SmartRules> {
    match playlist.smart_rule_set()? {
        Some(rules) => Ok(rules),
        None => Err(SmartPlaylistError::NotSmart {
            playlist_id: playlist.id.clone(),
        }
        .into()),
    }
}

impl Playlist {
    /// A copy marked smart and carrying `rules`. Does not write: [`Store::record`] is
    /// the only path that also writes the op.
    pub fn setting_smart_rules(&self, rules: &SmartRules) -> Playlist {
        let mut copy = self.clone();
        copy.is_smart = true;
        copy.smart_rules = Some(rules.to_json());
        copy
    }

    pub fn smart_rule_set(&self) -> std::result::Result<Option<SmartRules>, SmartPlaylistError> {
        match self.smart_rules.as_deref() {
            Some(json) if self.is_smart && !json.is_empty() => {
                SmartRules::from_json(json).map(Some)
            }
            _ => Ok(None),
        }
    }
}

// MARK: - Compilation to SQL

struct SmartQuery {
    sql: String,
    arguments: Vec<SqlValue>,
}

impl SmartQuery {
    fn new(
        rules: &SmartRules,
        now: Millis,
        calendar: Calendar,
    ) -> std::result::Result<Self, SmartPlaylistError> {
        let mut predicates = Vec::new();
        let mut arguments = Vec::new();
        for rule in &rules.rules {
            let (fragment, bound) = predicate(rule, now, calendar)?;
            predicates.push(format!("({fragment})"));
            arguments.extend(bound);
        }
        let filter = if predicates.is_empty() {
            "1".to_owned()
        } else {
            predicates.join(if rules.match_mode == Match::All {
                " AND "
            } else {
                " OR "
            })
        };
        let mut tail = String::new();
        if let Some(limit) = rules.limit {
            tail = "LIMIT ?".to_owned();
            arguments.push(SqlValue::Integer(limit));
        }

        // The history aggregate joins the cache for durations; the outer joins are LEFT
        // because a track nobody has played has a play count of zero, not no row.
        let play = PlayDefinition::counts_as_play("e", "d");
        let skip = PlayDefinition::counts_as_skip("e", "d");
        let sql = format!(
            "WITH history AS ( \
                SELECT e.jellyfinId AS jellyfinId, \
                       SUM(CASE WHEN {play} THEN 1 ELSE 0 END) AS playCount, \
                       SUM(CASE WHEN {skip} THEN 1 ELSE 0 END) AS skipCount, \
                       MAX(CASE WHEN {play} THEN e.startedAt END) AS lastPlayed, \
                       MIN(CASE WHEN {play} THEN e.startedAt END) AS firstPlayed \
                FROM playEvent e LEFT JOIN track d ON d.jellyfinId = e.jellyfinId \
                GROUP BY e.jellyfinId \
             ) \
             SELECT t.jellyfinId FROM track t \
             LEFT JOIN history h ON h.jellyfinId = t.jellyfinId \
             LEFT JOIN \"like\" l ON l.jellyfinId = t.jellyfinId AND l.deleted = 0 \
             LEFT JOIN trackFlags f ON f.jellyfinId = t.jellyfinId AND f.deleted = 0 \
             WHERE COALESCE(f.notInterested, 0) = 0 AND ({filter}) \
             ORDER BY {order} {tail}",
            order = order(rules.sort)?,
        );
        Ok(Self { sql, arguments })
    }
}

fn column(field: Field) -> &'static str {
    match field {
        Field::Title => "t.title",
        Field::Artist => "t.artist",
        Field::Album => "t.album",
        // A comparison against NULL is NULL, so "album artist is not X" would drop the
        // tracks with none — the ones most obviously satisfying it.
        Field::AlbumArtist => "IFNULL(t.albumArtist, '')",
        Field::Genre => "t.genres",
        Field::Year => "t.year",
        Field::DurationMs => "t.durationMs",
        Field::PlayCount => "COALESCE(h.playCount, 0)",
        Field::SkipCount => "COALESCE(h.skipCount, 0)",
        // Not coalesced: never played is not played at the epoch.
        Field::LastPlayed => "h.lastPlayed",
        Field::FirstPlayed => "h.firstPlayed",
        Field::Liked => "COALESCE(l.liked, 0)",
    }
}

fn predicate(
    rule: &SmartRule,
    now: Millis,
    calendar: Calendar,
) -> std::result::Result<(String, Vec<SqlValue>), SmartPlaylistError> {
    let column = column(rule.field);
    let mismatch = SmartPlaylistError::ValueTypeMismatch {
        field: rule.field,
        op: rule.op,
    };
    let not_applicable = SmartPlaylistError::OperatorNotApplicable {
        field: rule.field,
        op: rule.op,
    };

    match rule.field.kind() {
        Kind::Text => {
            let Value::Text(text) = &rule.value else {
                return Err(mismatch);
            };
            Ok(if rule.field == Field::Genre {
                genre(rule.op, text)
            } else {
                text_predicate(rule.op, column, text)
            })
        }
        Kind::Number => {
            let Value::Number(bound) = rule.value else {
                return Err(mismatch);
            };
            let argument = vec![SqlValue::Integer(bound)];
            match rule.op {
                Operator::Is => Ok((format!("{column} = ?"), argument)),
                // `NOT (NULL = 1999)` is NULL: without the first arm, "year is not
                // 1999" would drop the tracks whose year nobody knows.
                Operator::IsNot => Ok((format!("({column} IS NULL OR {column} <> ?)"), argument)),
                Operator::GreaterThan => Ok((format!("{column} > ?"), argument)),
                Operator::LessThan => Ok((format!("{column} < ?"), argument)),
                _ => Err(not_applicable),
            }
        }
        Kind::Date => {
            let instant = match &rule.value {
                Value::Text(written) => RelativeInterval::parse(written)?.ago(now, calendar),
                Value::Number(absolute) => *absolute,
                Value::Flag(_) => return Err(mismatch),
            };
            let argument = vec![SqlValue::Integer(instant)];
            match rule.op {
                // NULL fails both: `before` and `after` assert that a play happened.
                Operator::Before => Ok((format!("{column} < ?"), argument)),
                Operator::After => Ok((format!("{column} > ?"), argument)),
                Operator::InTheLast => Ok((format!("{column} >= ?"), argument)),
                // "Not played in the last month" includes never played at all.
                Operator::NotInTheLast => {
                    Ok((format!("({column} IS NULL OR {column} < ?)"), argument))
                }
                _ => Err(not_applicable),
            }
        }
        Kind::Flag => {
            let Value::Flag(flag) = rule.value else {
                return Err(mismatch);
            };
            let argument = vec![SqlValue::Integer(i64::from(flag))];
            match rule.op {
                Operator::Is => Ok((format!("{column} = ?"), argument)),
                Operator::IsNot => Ok((format!("{column} <> ?"), argument)),
                _ => Err(not_applicable),
            }
        }
    }
}

/// NOCASE so that "artist is beatles" finds The Beatles. ASCII-only, as SQLite's is.
fn text_predicate(op: Operator, column: &str, text: &str) -> (String, Vec<SqlValue>) {
    match op {
        Operator::Is => (
            format!("{column} = ? COLLATE NOCASE"),
            vec![SqlValue::Text(text.to_owned())],
        ),
        Operator::IsNot => (
            format!("{column} <> ? COLLATE NOCASE"),
            vec![SqlValue::Text(text.to_owned())],
        ),
        Operator::Contains => (
            format!("{column} LIKE ? ESCAPE '\\'"),
            vec![wildcarded(text, true)],
        ),
        Operator::DoesNotContain => (
            format!("{column} NOT LIKE ? ESCAPE '\\'"),
            vec![wildcarded(text, true)],
        ),
        Operator::BeginsWith => (
            format!("{column} LIKE ? ESCAPE '\\'"),
            vec![wildcarded(text, false)],
        ),
        // Unreachable past `check`; a false predicate rather than a crash.
        _ => ("0".to_owned(), Vec::new()),
    }
}

/// Genres are a JSON array, so every operator asks about *some* genre, and the negative
/// ones sit outside the `EXISTS` — inside, "is not Jazz" would match the jazz too.
fn genre(op: Operator, text: &str) -> (String, Vec<SqlValue>) {
    let (test, argument) = match op {
        Operator::Is | Operator::IsNot => (
            "g.value = ? COLLATE NOCASE",
            vec![SqlValue::Text(text.to_owned())],
        ),
        Operator::Contains | Operator::DoesNotContain => {
            ("g.value LIKE ? ESCAPE '\\'", vec![wildcarded(text, true)])
        }
        Operator::BeginsWith => ("g.value LIKE ? ESCAPE '\\'", vec![wildcarded(text, false)]),
        _ => ("0", Vec::new()),
    };
    let negated = matches!(op, Operator::IsNot | Operator::DoesNotContain);
    (
        format!(
            "{}EXISTS (SELECT 1 FROM json_each(t.genres) AS g WHERE {test})",
            if negated { "NOT " } else { "" }
        ),
        argument,
    )
}

/// A LIKE pattern whose only wildcards are the ones added here: "100%" must not match
/// every title containing 100.
fn wildcarded(text: &str, prefix: bool) -> SqlValue {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    SqlValue::Text(format!("{}{escaped}%", if prefix { "%" } else { "" }))
}

/// There is always an order, with `jellyfinId` as the tiebreak, so a LIMIT takes the
/// same subset every time.
fn order(sort: Option<Sort>) -> std::result::Result<String, SmartPlaylistError> {
    let Some(sort) = sort else {
        return Ok("t.title COLLATE NOCASE, t.jellyfinId".to_owned());
    };
    // A track has any number of genres; "sorted by genre" has no answer.
    if sort.field == Field::Genre {
        return Err(SmartPlaylistError::FieldNotSortable(Field::Genre));
    }
    let expression = if sort.field.kind() == Kind::Text {
        format!("{} COLLATE NOCASE", column(sort.field))
    } else {
        column(sort.field).to_owned()
    };
    Ok(format!(
        "{expression} {}, t.jellyfinId",
        if sort.dir == SortDirection::Asc {
            "ASC"
        } else {
            "DESC"
        }
    ))
}

// MARK: - Errors

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmartPlaylistError {
    #[error("no such rule field: {0}")]
    UnknownField(String),
    #[error("no such rule operator: {0}")]
    UnknownOperator(String),
    #[error("{} cannot apply to {}", op.wire_name(), field.wire_name())]
    OperatorNotApplicable { field: Field, op: Operator },
    #[error("the value does not suit {} {}", field.wire_name(), op.wire_name())]
    ValueTypeMismatch { field: Field, op: Operator },
    #[error("{0} is not a relative interval; expected something like -30d, -6m or -1y")]
    MalformedInterval(String),
    #[error("a rule value must be a string, a whole number or a boolean")]
    MalformedValue,
    #[error("matching any of no rules can never match anything")]
    NoRules,
    #[error("a limit of {0} can never return anything")]
    NonPositiveLimit(i64),
    #[error("{} has no single value to sort by", .0.wire_name())]
    FieldNotSortable(Field),
    #[error("playlist {playlist_id} carries no smart rules")]
    NotSmart { playlist_id: String },
    /// The document is not shaped like a rule set at all.
    #[error("not a rule set: {0}")]
    MalformedJson(String),
}
