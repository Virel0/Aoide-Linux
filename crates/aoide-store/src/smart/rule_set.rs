//! A whole rule set: how its rules combine, its order and limit, and its JSON form.

use serde_json::{Map, Value as Json};

use super::error::SmartPlaylistError;
use super::rule::{Field, SmartRule, malformed, required_str};
use crate::json::canonical_json;

/// How a rule set's rules combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Match {
    /// A track matches every rule; `all` of no rules is every track.
    All,
    /// A track matches at least one rule.
    Any,
}

/// Which way a sort runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortDirection {
    /// Smallest, earliest or A first.
    Asc,
    /// Largest, latest or Z first.
    Desc,
}

/// What a rule set is ordered by. Ties are broken by track id, so a limit always
/// takes the same tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sort {
    /// The field sorted on; any but genre.
    pub field: Field,
    /// Which way.
    pub dir: SortDirection,
}

impl Sort {
    /// A sort on `field`, `dir` first. Whether the field can be sorted on is checked when
    /// the rule set is evaluated.
    pub fn new(field: Field, dir: SortDirection) -> Self {
        Self { field, dir }
    }
}

/// The rule set stored as JSON in `playlist.smartRules`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SmartRules {
    /// How the rules combine.
    pub match_mode: Match,
    /// The rules, in the order they were written.
    pub rules: Vec<SmartRule>,
    /// The order; by title when `None`.
    pub sort: Option<Sort>,
    /// At most this many tracks; always positive.
    pub limit: Option<i64>,
}

impl SmartRules {
    /// `all` of nothing is every track, a real playlist; `any` of nothing can only be
    /// empty, and an always-empty playlist looks broken, so it is refused.
    ///
    /// # Errors
    ///
    /// [`SmartPlaylistError::NoRules`] for `any` of no rules, and
    /// [`SmartPlaylistError::NonPositiveLimit`] for a limit below one.
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

    /// Reads the stored JSON form (see [`Self::to_json`]). Operators may also be written
    /// as `=`, `!=`, `>`, `<`, `>=` and `<=`; the last two become `greaterThan` and
    /// `lessThan` with the bound moved by one.
    ///
    /// # Errors
    ///
    /// [`SmartPlaylistError::MalformedJson`] when the document is not a rule set, and any
    /// error [`SmartRule::new`] or [`Self::new`] gives for what it holds.
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
