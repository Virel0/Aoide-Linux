use crate::Millis;
use crate::error::Result;
use crate::smart::{Field, Match, Operator, SmartRule, SmartRules, Sort, SortDirection, Value};
use crate::store::Store;

/// Smart playlists every library gets. Their ids are fixed rather than minted, so the
/// phone, the desk and this device each create them on first run and end up with one
/// row, not three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltInPlaylist {
    /// Tracks played more than twice but not in the last six months; up to 40.
    Rediscover,
    /// Tracks played more than once and within the last month; up to 25.
    HeavyRotation,
}

impl BuiltInPlaylist {
    /// Both, in the order they are created.
    pub const ALL: [BuiltInPlaylist; 2] = [Self::Rediscover, Self::HeavyRotation];

    /// The fixed id every device creates it under.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rediscover => "builtin:rediscover",
            Self::HeavyRotation => "builtin:heavy-rotation",
        }
    }

    /// The name shown.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rediscover => "Rediscover Mix",
            Self::HeavyRotation => "Heavy Rotation",
        }
    }

    /// The notes it is created with.
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
    /// Creates the built-in smart playlists this library has never had, and puts the
    /// rules back on one that has lost them. Returns the ones it wrote.
    ///
    /// Call it only once this store has synced with the server: before that, a built-in
    /// the server already has isn't here yet, and creating it again would write a
    /// newer copy that wins on every device.
    ///
    /// Under their fixed ids, so the phone and the desk creating the same ones end in
    /// one row each; and only where **no row has ever existed** — deleted included —
    /// so a built-in removed on any device stays removed on all of them. Each is two
    /// ops, the create and the rules, as on the other clients, the rules a millisecond
    /// later. Written at the same instant (as this device once did), the rules lose
    /// the merge's tie on every other device and leave a plain, empty playlist; such
    /// a row is repaired here with its rules.
    ///
    /// # Errors
    ///
    /// Fails when the database does; built-ins written before the failure stay.
    pub fn ensure_built_in_playlists(&self, now: Millis) -> Result<Vec<BuiltInPlaylist>> {
        let mut written = Vec::new();
        // Each built-in's writes a millisecond apart, the rules after the row, and
        // each pair after the last, so the ops keep their order on every device.
        let mut at = now;
        for built in BuiltInPlaylist::ALL {
            let rules = built.rules();
            if self.playlist_row_exists(built.id())? {
                // A live row whose rules are gone (or unreadable) gets them back.
                let Some(list) = self.playlist(built.id())? else {
                    continue;
                };
                if matches!(list.smart_rule_set(), Ok(Some(_))) {
                    continue;
                }
                self.record(list.setting_smart_rules(&rules), false, at)?;
            } else {
                let list = self.create_playlist_with_id(
                    built.id(),
                    built.name(),
                    None,
                    Some(built.notes()),
                    at,
                )?;
                // A millisecond later than the row, so on every other device the
                // rules win the merge over the row's empty ones.
                self.record(list.setting_smart_rules(&rules), false, at + 1)?;
            }
            at += 2;
            written.push(built);
        }
        Ok(written)
    }
}
