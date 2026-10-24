use crate::smart::{Field, Match, Operator, SmartRule, SmartRules, Sort, SortDirection, Value};

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
