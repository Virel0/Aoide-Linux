//! A store belongs to one account.

use rusqlite::Transaction;

use crate::error::Result;
use crate::store::{Store, get_state, set_state};

/// What claiming the store on behalf of an account resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountClaim {
    /// Already this account's, or empty — nothing happened.
    Unchanged,
    /// It belonged to a different account and has been erased.
    Erased,
    /// It predates the account claim and holds data, so whose it is cannot be
    /// determined from inside the app. Only a person can answer: keep or erase.
    UnknownOwner,
}

const ACCOUNT_KEY: &str = "account";

impl Store {
    /// The Jellyfin user whose library this store holds, if claimed.
    pub fn account_id(&self) -> Result<Option<String>> {
        self.state(ACCOUNT_KEY)
    }

    /// Claims the store for `account_id`, erasing it if it demonstrably belonged to
    /// somebody else — on the way **in**, never on sign-out.
    ///
    /// The pull cursor is the sharp edge: it is a position in one user's log, and
    /// carried into a second account it skips that account's first ops permanently.
    /// Everything else goes too: the previous user's playlists would be visible to the
    /// next, and their unpushed ops would be pushed into another person's log.
    ///
    /// Signing out and back in as the same person is the ordinary case, and the local
    /// store is the only copy of anything not yet pushed, so sign-out erases nothing.
    pub fn adopt_account(&self, account_id: &str) -> Result<AccountClaim> {
        // The common path opens no write transaction; the decision is made again
        // inside the transaction, where it cannot race.
        if self.account_id()?.as_deref() == Some(account_id) {
            return Ok(AccountClaim::Unchanged);
        }
        self.write_local(|tx| {
            let previous = get_state(tx, ACCOUNT_KEY)?;
            // An unclaimed store that holds nothing is nobody's.
            if previous.is_none() && holds_data(tx)? {
                return Ok(AccountClaim::UnknownOwner);
            }
            let must_erase = previous
                .as_deref()
                .is_some_and(|previous| previous != account_id);
            if must_erase {
                erase_everything(tx)?;
            }
            if previous.as_deref() == Some(account_id) {
                return Ok(AccountClaim::Unchanged);
            }
            set_state(tx, ACCOUNT_KEY, account_id)?;
            Ok(if must_erase {
                AccountClaim::Erased
            } else {
                AccountClaim::Unchanged
            })
        })
    }

    /// Erases everything and claims the store: the answer to `UnknownOwner` when the
    /// person says the data is not theirs.
    pub fn erase_and_claim(&self, account_id: &str) -> Result<()> {
        self.write_local(|tx| {
            erase_everything(tx)?;
            set_state(tx, ACCOUNT_KEY, account_id)
        })
    }

    /// Claims the store without erasing: the answer when the person says it is theirs.
    pub fn keep_and_claim(&self, account_id: &str) -> Result<()> {
        self.write_local(|tx| set_state(tx, ACCOUNT_KEY, account_id))
    }
}

/// Empties every table the account owns, children before parents.
fn erase_everything(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    for table in [
        "playlistItem",
        "playlist",
        "folder",
        "like",
        "trackFlags",
        "playEvent",
        "queueState",
        "op",
        "quarantinedOp",
        "imageBlob",
        "track",
    ] {
        tx.execute(&format!("DELETE FROM \"{table}\""), [])?;
    }
    // The whole table, not the cursor row: every key here describes the account or
    // its data, and naming them one by one lets the next key survive a change of
    // account by default — which is how an index mark outlived the rows it vouched for.
    tx.execute("DELETE FROM \"syncState\"", [])?;
    Ok(())
}

/// Whether there is anything worth asking about. The track cache is excluded: it is a
/// rebuildable copy of what Jellyfin knows and says nothing about anyone's listening.
fn holds_data(tx: &Transaction<'_>) -> rusqlite::Result<bool> {
    for table in [
        "playlist",
        "playlistItem",
        "folder",
        "like",
        "trackFlags",
        "playEvent",
        "queueState",
    ] {
        let exists: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM \"{table}\")"),
            [],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(true);
        }
    }
    Ok(false)
}
