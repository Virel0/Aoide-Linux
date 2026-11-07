use crate::Millis;
use crate::Playlist;
use crate::error::Result;
use crate::smart::{Field, Match, Operator, SmartRule, SmartRules, Sort, SortDirection, Value};
use crate::store::Store;

/// Smart playlists every library gets.
///
/// Each user's are made under an id worked out from the built-in and the user
/// ([`BuiltInPlaylist::id_for`]), not minted, so the phone, the desk and this device
/// each create them on first run and end up with one row, not three. Per user,
/// because Melete gives each playlist id one owner: under one id for everybody
/// ([`BuiltInPlaylist::legacy_id`], as before), the first user to sync owned it and
/// every other user's were refused (Melete#324). The iOS `BuiltInPlaylists.swift`
/// makes the same ids.
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

    /// The id every user's devices once shared. A row under it is moved to the
    /// user's own id ([`Store::ensure_built_in_playlists`]).
    pub fn legacy_id(self) -> &'static str {
        match self {
            Self::Rediscover => "builtin:rediscover",
            Self::HeavyRotation => "builtin:heavy-rotation",
        }
    }

    /// The id this built-in has for the Melete user `user_id`.
    pub fn id_for(self, user_id: &str) -> String {
        format!("{}:{user_id}", self.legacy_id())
    }

    /// The built-in a playlist is, under its user's id or the old shared one.
    pub fn of_playlist(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|built| {
            let stem = built.legacy_id();
            id == stem
                || id
                    .strip_prefix(stem)
                    .is_some_and(|rest| rest.starts_with(':'))
        })
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
    /// rules back on one that has lost them. Returns the ones it created or repaired.
    ///
    /// Call it only once this store has synced with the server: before that, a built-in
    /// the server already has isn't here yet, and creating it again would write a
    /// newer copy that wins on every device.
    ///
    /// Under `user_id`'s own ids for them ([`BuiltInPlaylist::id_for`]), so the phone
    /// and the desk creating the same ones end in one row each; and only where **no
    /// row has ever existed** — deleted included — so a built-in removed on any device
    /// stays removed on all of them. Each is two ops, the create and the rules, as on
    /// the other clients, the rules a millisecond later. Written at the same instant
    /// (as this device once did), the rules lose the merge's tie on every other device
    /// and leave a plain, empty playlist; such a row is repaired here with its rules.
    ///
    /// A row under the old shared id ([`BuiltInPlaylist::legacy_id`]) is moved: copied
    /// to the user's id, with whatever name, notes, folder and rules it has, then
    /// deleted. Melete refuses the delete from anyone but the user whose device made
    /// it first, which leaves the others' copy only on their own device, deleted
    /// there. One already deleted under the old id means the listener deleted it, and
    /// nothing is made.
    ///
    /// # Errors
    ///
    /// Fails when the database does; built-ins written before the failure stay.
    pub fn ensure_built_in_playlists(
        &self,
        user_id: &str,
        now: Millis,
    ) -> Result<Vec<BuiltInPlaylist>> {
        let mut written = Vec::new();
        // Each built-in's writes a millisecond apart, the rules after the row, and
        // each pair after the last, so the ops keep their order on every device.
        let mut at = now;
        for built in BuiltInPlaylist::ALL {
            let id = built.id_for(user_id);
            let legacy = self.playlist(built.legacy_id())?;
            if self.playlist_row_exists(&id)? {
                if self.repair(built, &id, at)? {
                    written.push(built);
                }
            } else if let Some(old) = &legacy {
                self.record(moved(old, built, &id), false, at)?;
                written.push(built);
            } else if !self.playlist_row_exists(built.legacy_id())? {
                let list =
                    self.create_playlist_with_id(&id, built.name(), None, Some(built.notes()), at)?;
                // A millisecond later than the row, so on every other device the
                // rules win the merge over the row's empty ones.
                self.record(list.setting_smart_rules(&built.rules()), false, at + 1)?;
                written.push(built);
            }
            if legacy.is_some() {
                self.delete_playlist(built.legacy_id(), at + 1)?;
            }
            at += 2;
        }
        Ok(written)
    }

    /// Gives the live row `id` its built-in's rules when it has lost them (or they
    /// can't be read); whether it did.
    fn repair(&self, built: BuiltInPlaylist, id: &str, at: Millis) -> Result<bool> {
        let Some(list) = self.playlist(id)? else {
            return Ok(false);
        };
        if matches!(list.smart_rule_set(), Ok(Some(_))) {
            return Ok(false);
        }
        self.record(list.setting_smart_rules(&built.rules()), false, at)?;
        Ok(true)
    }
}

/// The row under a built-in's old shared id, as the user's own under `id`: its name,
/// notes, folder and place kept, and its rules too unless they're gone.
fn moved(old: &Playlist, built: BuiltInPlaylist, id: &str) -> Playlist {
    let mut row = Playlist {
        id: id.to_owned(),
        field_updated_at: None,
        ..old.clone()
    };
    if !matches!(row.smart_rule_set(), Ok(Some(_))) {
        row = row.setting_smart_rules(&built.rules());
    }
    row
}
