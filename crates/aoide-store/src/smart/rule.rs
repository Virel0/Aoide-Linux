//! One rule: the field it talks about, the comparison, and the value, and which
//! combinations of the three are legal.

use serde_json::{Map, Value as Json};

use super::error::SmartPlaylistError;
use super::interval::RelativeInterval;

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
pub(super) enum Kind {
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

    pub(super) fn kind(self) -> Kind {
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

    pub(super) fn to_json(&self) -> Json {
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

    pub(super) fn from_json(value: &Json) -> std::result::Result<Self, SmartPlaylistError> {
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
pub(super) fn required_str<'a>(
    object: &'a Map<String, Json>,
    key: &str,
) -> std::result::Result<&'a str, SmartPlaylistError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| malformed(&format!("'{key}' must be a string")))
}

pub(super) fn malformed(detail: &str) -> SmartPlaylistError {
    SmartPlaylistError::MalformedJson(detail.to_owned())
}
