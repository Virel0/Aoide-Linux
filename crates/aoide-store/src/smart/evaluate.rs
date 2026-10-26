//! Running a rule set against the store, and reading one off a playlist.

use rusqlite::params_from_iter;

use super::error::SmartPlaylistError;
use super::interval::Calendar;
use super::rule_set::SmartRules;
use super::sql::SmartQuery;
use crate::Millis;
use crate::error::Result;
use crate::models::Playlist;
use crate::store::Store;

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
