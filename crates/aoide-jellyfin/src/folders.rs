//! The library as folders on disk. Reading only.

use crate::client::JellyfinClient;
use crate::library::STANDARD_FIELDS;
use crate::{Item, Page, Result};

impl JellyfinClient {
    /// One page of what is directly inside `parent_id`, folders first, then by
    /// name. Start from a music library's id (from
    /// [`music_libraries`](Self::music_libraries)) and go down by the ids of the
    /// items with [`Item::is_folder`] set. Folders come back as
    /// [`ItemKind::Folder`](crate::ItemKind::Folder), or as
    /// [`ItemKind::MusicAlbum`](crate::ItemKind::MusicAlbum) where the server
    /// took a folder for an album; files as [`ItemKind::Audio`](crate::ItemKind::Audio).
    ///
    /// `limit: None` asks for everything in the folder.
    pub async fn folder_children(
        &self,
        parent_id: &str,
        start_index: u32,
        limit: Option<u32>,
    ) -> Result<Page<Item>> {
        // Not recursive: one level, as a folder view shows it.
        let mut query = vec![
            ("userId", self.user_id()?.to_owned()),
            ("parentId", parent_id.to_owned()),
            ("sortBy", "IsFolder,SortName".to_owned()),
            ("sortOrder", "Ascending".to_owned()),
            ("startIndex", start_index.to_string()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        self.get(&["Items"], &query).await
    }
}
