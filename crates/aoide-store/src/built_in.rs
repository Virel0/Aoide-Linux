use crate::Millis;
use crate::error::Result;
use crate::smart::{Field, Match, Operator, SmartRule, SmartRules, Sort, SortDirection, Value};
use crate::store::Store;

/// Smart playlists every library gets. Their ids are fixed rather than minted, so the
/// phone, the desk and this device each create them on first run and end up with one
/// row, not three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltInPlaylist {
    Rediscover,
    HeavyRotation,
}

impl BuiltInPlaylist {
    pub const ALL: [BuiltInPlaylist; 2] = [Self::Rediscover, Self::HeavyRotation];

    pub fn id(self) -> &'static str {
        match self {
            Self::Rediscover => "builtin:rediscover",
            Self::HeavyRotation => "builtin:heavy-rotation",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rediscover => "Rediscover Mix",
            Self::HeavyRotation => "Heavy Rotation",
        }
    }

    pub fn notes(self) -> &'static str {
        match self {
            Self::Rediscover => {
                "Songs you played a lot and haven't heard in six months. Changes as you listen."
            }
            Self::HeavyRotation => "What you've played most this month.",
        }
    }

    /// Every client must produce exactly these rules.
    pub fn rules(self) -> SmartRules {
        let (window, minimum, limit) = match self {
            Self::Rediscover => ((Operator::NotInTheLast, "-6m"), 2, 40),
            Self::HeavyRotation => ((Operator::InTheLast, "-1m"), 1, 25),
        };
        let rules = vec![
            SmartRule {
                field: Field::LastPlayed,
                op: window.0,
                value: Value::Text(window.1.to_owned()),
            },
            SmartRule {
                field: Field::PlayCount,
                op: Operator::GreaterThan,
                value: Value::Number(minimum),
            },
        ];
        SmartRules {
            match_mode: Match::All,
            rules,
            sort: Some(Sort::new(Field::PlayCount, SortDirection::Desc)),
            limit: Some(limit),
        }
    }
}

impl Store {
    /// Creates the built-in smart playlists this library has never had, and returns
    /// which it created. Safe to call at every launch.
    ///
    /// Under their fixed ids, so the phone and the desk creating the same ones end in
    /// one row each; and only where **no row has ever existed** — deleted included —
    /// so a built-in removed on any device stays removed on all of them. Each is two
    /// ops, the create and the rules, as on the other clients.
    pub fn ensure_built_in_playlists(&self, now: Millis) -> Result<Vec<BuiltInPlaylist>> {
        let mut created = Vec::new();
        for built in BuiltInPlaylist::ALL {
            if self.playlist_row_exists(built.id())? {
                continue;
            }
            let list = self.create_playlist_with_id(
                built.id(),
                built.name(),
                None,
                Some(built.notes()),
                now,
            )?;
            self.record(list.setting_smart_rules(&built.rules()), false, now)?;
            created.push(built);
        }
        Ok(created)
    }
}
