//! Jellyfin's own playlists: create, edit, delete, entries, and the cover.
//!
//! These are the server's playlists, not Aoide playlists (those live in
//! aoide-store and sync through the sidecar). Listing them is
//! [`JellyfinClient::playlists`].

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::{Body, JellyfinClient};
use crate::library::{MAX_IDS_PER_REQUEST, STANDARD_FIELDS};
use crate::{Item, Page, Result};

/// One entry of a playlist: the item, and the entry's own id.
///
/// Removing and moving go by `entry_id`, never by the item id, because the same
/// song can be in a playlist twice. Current servers (checked against Jellyfin's
/// source, 10.11) hand out the item id as the entry id, so for them the two are
/// equal and removing one copy of a doubled song removes both; older servers
/// give each entry its own id. Either way, pass back what this returned.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    /// The entry's id within the playlist, for removing and moving it.
    pub entry_id: String,
    /// The track the entry plays.
    pub item: Item,
}

/// What to change about a playlist. A `None` field is left as it is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaylistEdit {
    /// A new name.
    pub name: Option<String>,
    /// Whether every user on the server can see the playlist.
    pub is_public: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Created {
    id: String,
}

#[derive(Deserialize)]
struct WireEntry {
    #[serde(rename = "PlaylistItemId", default)]
    entry_id: Option<String>,
    #[serde(flatten)]
    item: Item,
}

impl JellyfinClient {
    /// Makes a playlist owned by the signed-in user, holding `item_ids` in order
    /// (none is fine), and returns its id.
    ///
    /// The ids go in the JSON body rather than the URL, so a long first selection
    /// can't overrun a proxy's URL limit.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn create_playlist<S: AsRef<str>>(
        &self,
        name: &str,
        item_ids: &[S],
    ) -> Result<String> {
        let ids: Vec<&str> = item_ids.iter().map(AsRef::as_ref).collect();
        let body = json!({
            "Name": name,
            "Ids": ids,
            "UserId": self.user_id()?,
            "MediaType": "Audio",
        });
        let created: Created = self
            .send_decoded(Method::POST, &["Playlists"], &[], Some(&body))
            .await?;
        Ok(created.id)
    }

    /// Renames a playlist or changes who can see it. Only the fields set in
    /// `edit` are sent; an empty edit sends nothing.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials, even
    /// for an empty edit; otherwise the [request errors](JellyfinClient#errors).
    pub async fn edit_playlist(&self, playlist_id: &str, edit: &PlaylistEdit) -> Result<()> {
        self.require_credentials()?;
        let mut body = Map::new();
        if let Some(name) = &edit.name {
            body.insert("Name".into(), Value::from(name.as_str()));
        }
        if let Some(is_public) = edit.is_public {
            body.insert("IsPublic".into(), Value::from(is_public));
        }
        if body.is_empty() {
            return Ok(());
        }
        self.post_json(&["Playlists", playlist_id], &Value::Object(body))
            .await
    }

    /// Deletes a playlist from the server, file and all.
    ///
    /// Jellyfin deletes playlists through its general item delete, which deletes
    /// a song's file just as readily. So the id is first looked up as a playlist
    /// (`GET /Playlists/{id}`), and anything that isn't one stops here as
    /// [`Error::NotFound`](crate::Error::NotFound) with nothing deleted.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; [`Error::NotFound`](crate::Error::NotFound) when
    /// `playlist_id` isn't a playlist, with nothing deleted; otherwise the [request
    /// errors](JellyfinClient#errors).
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<()> {
        self.require_credentials()?;
        let _: Value = self.get(&["Playlists", playlist_id], &[]).await?;
        self.send_empty(Method::DELETE, &["Items", playlist_id], &[])
            .await
    }

    /// The playlist's entries in playlist order, each with its entry id.
    ///
    /// [`playlist_items`](Self::playlist_items) is the same list without entry
    /// ids, for playing.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn playlist_entries(&self, playlist_id: &str) -> Result<Vec<PlaylistEntry>> {
        let query = [
            ("userId", self.user_id()?.to_owned()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        let page: Page<WireEntry> = self
            .get(&["Playlists", playlist_id, "Items"], &query)
            .await?;
        Ok(page
            .items
            .into_iter()
            .map(|wire| PlaylistEntry {
                entry_id: wire.entry_id.unwrap_or_else(|| wire.item.id.clone()),
                item: wire.item,
            })
            .collect())
    }

    /// Adds items to a playlist, in order, at the end or from `position` (0-based).
    ///
    /// The ids travel in the URL, so they go 100 at a time, each chunk placed
    /// after the one before, so a long selection keeps its order.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors)
    /// from the first chunk that fails. Chunks sent before it stay added.
    pub async fn add_to_playlist<S: AsRef<str>>(
        &self,
        playlist_id: &str,
        item_ids: &[S],
        position: Option<u32>,
    ) -> Result<()> {
        let user_id = self.user_id()?.to_owned();
        for (n, chunk) in item_ids.chunks(MAX_IDS_PER_REQUEST).enumerate() {
            let mut query = vec![("ids", join(chunk)), ("userId", user_id.clone())];
            if let Some(position) = position {
                let offset = (n * MAX_IDS_PER_REQUEST) as u32;
                query.push(("position", position.saturating_add(offset).to_string()));
            }
            self.send_empty(Method::POST, &["Playlists", playlist_id, "Items"], &query)
                .await?;
        }
        Ok(())
    }

    /// Removes entries from a playlist by their entry ids (from
    /// [`playlist_entries`](Self::playlist_entries)). No ids sends nothing.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors)
    /// from the first chunk that fails. Chunks sent before it stay removed.
    pub async fn remove_from_playlist<S: AsRef<str>>(
        &self,
        playlist_id: &str,
        entry_ids: &[S],
    ) -> Result<()> {
        self.require_credentials()?;
        for chunk in entry_ids.chunks(MAX_IDS_PER_REQUEST) {
            let query = [("entryIds", join(chunk))];
            self.send_empty(Method::DELETE, &["Playlists", playlist_id, "Items"], &query)
                .await?;
        }
        Ok(())
    }

    /// Moves one entry to `new_index` (0-based, counted among the entries the user
    /// can see). An index past the end moves it to the end.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn move_playlist_entry(
        &self,
        playlist_id: &str,
        entry_id: &str,
        new_index: u32,
    ) -> Result<()> {
        self.require_credentials()?;
        let index = new_index.to_string();
        self.send_empty(
            Method::POST,
            &["Playlists", playlist_id, "Items", entry_id, "Move", &index],
            &[],
        )
        .await
    }

    /// Replaces a playlist's cover (its primary image) with `image`, the file's
    /// bytes, of type `content_type` (`image/jpeg`, `image/png`, `image/webp`).
    ///
    /// Jellyfin reads this body as base64 text, whatever the content type says,
    /// so the bytes are encoded here. The server only lets administrators change
    /// images: anyone else gets HTTP 403 as [`Error::Http`](crate::Error::Http).
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; [`Error::Http`](crate::Error::Http) with status 403 for
    /// a user who isn't an administrator; otherwise the [request
    /// errors](JellyfinClient#errors).
    pub async fn set_playlist_cover(
        &self,
        playlist_id: &str,
        image: &[u8],
        content_type: &str,
    ) -> Result<()> {
        self.require_credentials()?;
        let body = Body::Raw {
            content_type,
            bytes: BASE64.encode(image).into_bytes(),
        };
        self.send_body(
            Method::POST,
            &["Items", playlist_id, "Images", "Primary"],
            &[],
            body,
        )
        .await
        .map(drop)
    }

    /// Removes a playlist's own cover. Administrators only, as for
    /// [`set_playlist_cover`](Self::set_playlist_cover).
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn delete_playlist_cover(&self, playlist_id: &str) -> Result<()> {
        self.require_credentials()?;
        self.send_empty(
            Method::DELETE,
            &["Items", playlist_id, "Images", "Primary"],
            &[],
        )
        .await
    }
}

fn join<S: AsRef<str>>(ids: &[S]) -> String {
    ids.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(",")
}
