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
    pub fn folders(&self) -> Result<Vec<Folder>> {
        self.read(|conn| Folder::fetch_where(conn, "deleted = 0 ORDER BY sortIndex, rowid", []))
    }

    pub fn folders_in_parent(&self, parent_id: Option<&str>) -> Result<Vec<Folder>> {
        self.read(|conn| {
            Folder::fetch_where(
                conn,
                "deleted = 0 AND parentId IS ? ORDER BY sortIndex, rowid",
                [parent_id],
            )
        })
    }

    pub fn folder(&self, id: &str) -> Result<Option<Folder>> {
        Ok(self
            .read(|conn| Folder::fetch(conn, id))?
            .filter(|folder| !folder.deleted))
    }

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

    pub fn rename_folder(&self, id: &str, name: &str, now: Millis) -> Result<Folder> {
        let mut folder = self.require_folder(id)?;
        folder.name = name.to_owned();
        self.record(folder, false, now)
    }

    /// Re-parents a folder, refusing any move that would make it its own ancestor.
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
