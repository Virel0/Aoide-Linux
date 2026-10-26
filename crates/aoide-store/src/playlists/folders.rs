//! Folders: the tree playlists are filed in.

use std::collections::HashSet;

use super::{PlaylistError, check_gap, last_key};
use crate::Millis;
use crate::error::Result;
use crate::fractional_index::FractionalIndex;
use crate::models::Folder;
use crate::record::Record;
use crate::store::Store;

impl Store {
    /// Live folders in display order, across every level.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        self.read(|conn| Folder::fetch_where(conn, "deleted = 0 ORDER BY sortIndex, rowid", []))
    }

    /// Live folders directly under `parent_id`; `None` is the top level.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn folders_in_parent(&self, parent_id: Option<&str>) -> Result<Vec<Folder>> {
        self.read(|conn| {
            Folder::fetch_where(
                conn,
                "deleted = 0 AND parentId IS ? ORDER BY sortIndex, rowid",
                [parent_id],
            )
        })
    }

    /// A live folder; a deleted one is `None`.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn folder(&self, id: &str) -> Result<Option<Folder>> {
        Ok(self
            .read(|conn| Folder::fetch(conn, id))?
            .filter(|folder| !folder.deleted))
    }

    /// Creates a folder under `parent_id` (or at the top level), at the end of the list.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchFolder`] when the parent is missing, or a database error.
    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
        now: Millis,
    ) -> Result<Folder> {
        if let Some(parent_id) = parent_id {
            self.require_folder(parent_id)?;
        }
        let mut folder = Folder::new(name, self.next_folder_sort_index()?, now, self.device_id());
        folder.parent_id = parent_id.map(str::to_owned);
        self.record(folder, false, now)
    }

    /// Renames a folder in place.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchFolder`] when there is no live folder `id`, or a database
    /// error.
    pub fn rename_folder(&self, id: &str, name: &str, now: Millis) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        folder.name = name.to_owned();
        self.record(folder, false, now)
    }

    /// Re-parents a folder, refusing any move that would make it its own ancestor.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchFolder`] when either folder is missing,
    /// [`PlaylistError::FolderCycle`] when `parent_id` is inside `id`, or a database
    /// error.
    pub fn move_folder(&self, id: &str, parent_id: Option<&str>, now: Millis) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        if let Some(parent_id) = parent_id {
            self.require_folder(parent_id)?;
            if self.ancestry_contains(id, parent_id)? {
                return Err(PlaylistError::FolderCycle {
                    folder: id.to_owned(),
                    under: parent_id.to_owned(),
                }
                .into());
            }
        }
        folder.parent_id = parent_id.map(str::to_owned);
        folder.sort_index = self.next_folder_sort_index()?;
        self.record(folder, false, now)
    }

    /// Soft-deletes a folder and promotes everything inside it one level up — a folder
    /// is filing, not content, and removing one must not destroy playlists.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchFolder`] when no row has the id, deleted or not, or a
    /// database error.
    pub fn delete_folder(&self, id: &str, now: Millis) -> Result<()> {
        let folder = self
            .read(|conn| Folder::fetch(conn, id))?
            .ok_or_else(|| PlaylistError::NoSuchFolder(id.to_owned()))?;
        if folder.deleted {
            return Ok(());
        }
        for mut child in self.folders_in_parent(Some(id))? {
            child.parent_id = folder.parent_id.clone();
            self.record(child, false, now)?;
        }
        for mut list in self.playlists_in_folder(Some(id))? {
            list.folder_id = folder.parent_id.clone();
            self.record(list, false, now)?;
        }
        self.record(folder, true, now)?;
        Ok(())
    }

    /// Repositions a folder between two siblings; `None` on either side is the end.
    /// Passing neither leaves it where it is.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchFolder`] when it or a neighbour is missing,
    /// [`PlaylistError::BadNeighbours`] when the neighbours leave no gap, or a database
    /// error.
    pub fn reorder_folder(
        &self,
        id: &str,
        after: Option<&str>,
        before: Option<&str>,
        now: Millis,
    ) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        if after.is_none() && before.is_none() {
            return Ok(folder);
        }
        if after == Some(id) || before == Some(id) {
            return Err(PlaylistError::BadNeighbours.into());
        }
        let lower = after
            .map(|n| self.require_folder(n).map(|f| f.sort_index))
            .transpose()?;
        let upper = before
            .map(|n| self.require_folder(n).map(|f| f.sort_index))
            .transpose()?;
        check_gap(lower.as_deref(), upper.as_deref())?;
        folder.sort_index = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        self.record(folder, false, now)
    }
}

impl Store {
    /// Whether `folder_id` lies on the parent chain from `start_id`. Terminates on a
    /// cycle it did not build: two offline moves can merge into one, and a repeat
    /// visit is reported as the cycle it is.
    fn ancestry_contains(&self, folder_id: &str, start_id: &str) -> Result<bool> {
        self.read(|conn| {
            let mut cursor = Some(start_id.to_owned());
            let mut seen = HashSet::new();
            while let Some(id) = cursor {
                if id == folder_id || !seen.insert(id.clone()) {
                    return Ok(true);
                }
                cursor = Folder::fetch(conn, &id)?.and_then(|folder| folder.parent_id);
            }
            Ok(false)
        })
    }

    fn next_folder_sort_index(&self) -> Result<String> {
        let last = self.read(|conn| {
            last_key(
                conn,
                "SELECT sortIndex FROM folder WHERE deleted = 0 ORDER BY sortIndex DESC LIMIT 1",
                None,
            )
        })?;
        Ok(FractionalIndex::between(last.as_deref(), None))
    }
}
