//! Library views' shared pieces: tiles, and the one listing aoide-jellyfin
//! does not have yet (a genre's albums).

use aoide_jellyfin::{Error, Item, JellyfinClient, Page};

use crate::artwork::ArtKey;

/// An album (or artist) as a tile in a grid or a shelf.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub art: Option<ArtKey>,
}

impl Tile {
    pub fn album(item: &Item) -> Self {
        let subtitle = item
            .album_artist
            .clone()
            .or_else(|| item.artists.first().cloned())
            .unwrap_or_default();
        Self {
            id: item.id.clone(),
            title: item.name.clone(),
            subtitle,
            art: item.artwork_source().map(|(id, tag)| ArtKey::new(id, tag)),
        }
    }

    pub fn artist(item: &Item) -> Self {
        let subtitle = match item.child_count {
            Some(1) => "1 album".to_owned(),
            Some(n) if n > 1 => format!("{n} albums"),
            _ => String::new(),
        };
        Self {
            id: item.id.clone(),
            title: item.name.clone(),
            subtitle,
            art: item.artwork_source().map(|(id, tag)| ArtKey::new(id, tag)),
        }
    }

    /// The album a track belongs to, as a tile: what Home's played shelves show,
    /// since Jellyfin keeps play statistics on tracks only.
    pub fn album_of_track(item: &Item) -> Option<Self> {
        let album_id = item.album_id.clone()?;
        Some(Self {
            id: album_id.clone(),
            title: item.album.clone().unwrap_or_else(|| item.name.clone()),
            subtitle: item
                .album_artist
                .clone()
                .or_else(|| item.artists.first().cloned())
                .unwrap_or_default(),
            art: item
                .album_primary_image_tag
                .clone()
                .map(|tag| ArtKey::new(album_id, tag)),
        })
    }
}

/// The albums of the tracks, each once, in the order first met.
pub fn albums_of_tracks(tracks: &[Item], limit: usize) -> Vec<Tile> {
    let mut seen = std::collections::HashSet::new();
    tracks
        .iter()
        .filter_map(Tile::album_of_track)
        .filter(|tile| seen.insert(tile.id.clone()))
        .take(limit)
        .collect()
}

/// A genre's albums, by name.
///
/// aoide-jellyfin's `ListQuery` has no genre filter yet, so this one request
/// is made here, with the client's own address, device header and token.
/// It belongs in the library crate as `ListQuery::genre_ids`.
pub async fn genre_albums(
    client: &JellyfinClient,
    genre_id: &str,
    parent_id: Option<&str>,
) -> Result<Vec<Item>, Error> {
    let credentials = client.credentials().ok_or(Error::NotSignedIn)?;
    let mut url = client.address().as_url().clone();
    if let Ok(mut segments) = url.path_segments_mut() {
        segments.pop_if_empty().push("Items");
    }
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("userId", &credentials.user_id)
            .append_pair("genreIds", genre_id)
            .append_pair("includeItemTypes", "MusicAlbum")
            .append_pair("recursive", "true")
            .append_pair("sortBy", "AlbumArtist,SortName")
            .append_pair("sortOrder", "Ascending")
            .append_pair("fields", "Genres,AlbumArtist,Artists,ParentId,ChildCount");
        if let Some(parent_id) = parent_id {
            query.append_pair("parentId", parent_id);
        }
    }
    let header = client
        .device()
        .authorization_header(Some(&credentials.access_token));
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| Error::Transport(e.to_string()))?;
    let response = http
        .get(url)
        .header(reqwest::header::AUTHORIZATION, header.as_str())
        .header("X-Emby-Authorization", header.as_str())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| Error::Transport(e.without_url().to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .bytes()
        .await
        .map_err(|e| Error::Transport(e.without_url().to_string()))?;
    match status {
        200..=299 => serde_json::from_slice::<Page<Item>>(&body)
            .map(|page| page.items)
            .map_err(|e| Error::Decoding {
                path: "Items".into(),
                message: e.to_string(),
            }),
        401 => Err(Error::Unauthorized),
        404 => Err(Error::NotFound {
            path: "Items".into(),
        }),
        status => Err(Error::Http {
            status,
            path: "Items".into(),
            detail: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{header_regex, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn item(json: serde_json::Value) -> Item {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn played_tracks_become_their_albums_once_each() {
        let tracks = [
            item(
                serde_json::json!({"Id": "t1", "Name": "a", "AlbumId": "A", "Album": "Alpha", "AlbumPrimaryImageTag": "x"}),
            ),
            item(serde_json::json!({"Id": "t2", "Name": "b", "AlbumId": "B", "Album": "Beta"})),
            item(serde_json::json!({"Id": "t3", "Name": "c", "AlbumId": "A", "Album": "Alpha"})),
            item(serde_json::json!({"Id": "t4", "Name": "loose"})),
        ];
        let tiles = albums_of_tracks(&tracks, 10);
        let ids: Vec<&str> = tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
        assert_eq!(tiles[0].art, Some(ArtKey::new("A", "x")));
        assert_eq!(tiles[1].art, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_genres_albums_are_asked_for_by_genre_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/jf/Items"))
            .and(query_param("genreIds", "g1"))
            .and(query_param("includeItemTypes", "MusicAlbum"))
            .and(query_param("parentId", "lib"))
            .and(header_regex("authorization", r#"Token="tok""#))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [{"Id": "al1", "Name": "Blue", "Type": "MusicAlbum"}],
                "TotalRecordCount": 1
            })))
            .mount(&server)
            .await;
        let client = JellyfinClient::new(
            ServerAddress::parse(&format!("{}/jf", server.uri())).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let albums = genre_albums(&client, "g1", Some("lib")).await.unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Blue");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_revoked_token_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = JellyfinClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        assert_eq!(
            genre_albums(&client, "g1", None).await,
            Err(Error::Unauthorized)
        );
    }
}
