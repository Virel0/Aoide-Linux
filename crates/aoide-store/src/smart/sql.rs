//! Compiling a rule set to one SQL statement. Every rule value is a bound parameter;
//! fields and operators reach the SQL only through `match`es over closed enums.

use rusqlite::types::Value as SqlValue;

use super::error::SmartPlaylistError;
use super::interval::{Calendar, RelativeInterval};
use super::rule::{Field, Kind, Operator, SmartRule, Value};
use super::rule_set::{Match, SmartRules, Sort, SortDirection};
use crate::Millis;
use crate::play_definition::PlayDefinition;

pub(super) struct SmartQuery {
    pub(super) sql: String,
    pub(super) arguments: Vec<SqlValue>,
}

impl SmartQuery {
    pub(super) fn new(
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
    let bound = match (rule.field.kind(), &rule.value) {
        (Kind::Text, Value::Text(text)) => {
            return Ok(if rule.field == Field::Genre {
                genre(rule.op, text)
            } else {
                text_predicate(rule.op, column, text)
            });
        }
        (Kind::Number, Value::Number(bound)) => *bound,
        (Kind::Date, Value::Text(written)) => RelativeInterval::parse(written)?.ago(now, calendar),
        (Kind::Date, Value::Number(absolute)) => *absolute,
        (Kind::Flag, Value::Flag(flag)) => i64::from(*flag),
        _ => {
            return Err(SmartPlaylistError::ValueTypeMismatch {
                field: rule.field,
                op: rule.op,
            });
        }
    };
    let fragment = comparison(rule.field.kind(), rule.op, column).ok_or(
        SmartPlaylistError::OperatorNotApplicable {
            field: rule.field,
            op: rule.op,
        },
    )?;
    Ok((fragment, vec![SqlValue::Integer(bound)]))
}

/// A number, date or flag column against one bound integer; `None` for a pair `check`
/// would have refused.
fn comparison(kind: Kind, op: Operator, column: &str) -> Option<String> {
    Some(match (kind, op) {
        (Kind::Number | Kind::Flag, Operator::Is) => format!("{column} = ?"),
        // `NOT (NULL = 1999)` is NULL: without the first arm, "year is not
        // 1999" would drop the tracks whose year nobody knows.
        (Kind::Number, Operator::IsNot) => format!("({column} IS NULL OR {column} <> ?)"),
        (Kind::Number, Operator::GreaterThan) => format!("{column} > ?"),
        (Kind::Number, Operator::LessThan) => format!("{column} < ?"),
        // NULL fails both: `before` and `after` assert that a play happened.
        (Kind::Date, Operator::Before) => format!("{column} < ?"),
        (Kind::Date, Operator::After) => format!("{column} > ?"),
        (Kind::Date, Operator::InTheLast) => format!("{column} >= ?"),
        // "Not played in the last month" includes never played at all.
        (Kind::Date, Operator::NotInTheLast) => format!("({column} IS NULL OR {column} < ?)"),
        (Kind::Flag, Operator::IsNot) => format!("{column} <> ?"),
        _ => return None,
    })
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
