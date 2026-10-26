//! Search by description: a phrase, through the model, to filters, and the
//! filters to an ordinary Jellyfin query, so the songs are always the
//! server's.

use std::sync::Arc;

use aoide_integrations::openrouter::OpenRouter;
use aoide_integrations::openrouter::smart_search::{MusicQuery, Ordering};
use aoide_jellyfin::{ItemKind, JellyfinClient, ListQuery, NameId, Sort, SortOrder};

use super::read_key;
use crate::credentials::CredentialStore;
use crate::playback::tracks::TrackMeta;

/// How many songs a description finds at most.
pub const DESCRIBED_SONGS: u32 = 100;

/// Most years a range is spelled out as, where the server lists none.
const MAX_YEARS: i32 = 150;

/// What a description found.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    /// The filters in words, or `None` when the description gave nothing to
    /// filter by (the plain search stands).
    pub understood: Option<String>,
    pub songs: Vec<TrackMeta>,
    /// Why there are no filters, or why nothing could match.
    pub note: Option<String>,
}

/// Why a description could not be searched.
#[derive(Debug)]
pub enum Failure {
    Server(aoide_jellyfin::Error),
    Other(String),
}

/// The filters in words, for "Understood as …".
pub fn understood(query: &MusicQuery) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(term) = &query.search_term {
        parts.push(format!("“{term}”"));
    }
    if !query.genres.is_empty() {
        parts.push(query.genres.join(", "));
    }
    match (query.from_year, query.to_year) {
        (Some(a), Some(b)) if a == b => parts.push(a.to_string()),
        (Some(a), Some(b)) => parts.push(format!("{a}–{b}")),
        (Some(a), None) => parts.push(format!("from {a}")),
        (None, Some(b)) => parts.push(format!("up to {b}")),
        (None, None) => {}
    }
    if query.favorites_only {
        parts.push("favourites".to_owned());
    }
    let order = match query.ordering {
        Ordering::ByName => None,
        Ordering::MostPlayed => Some("most played first"),
        Ordering::NewestFirst => Some("newest first"),
        Ordering::Random => Some("in random order"),
        Ordering::RecentlyPlayed => Some("recently played first"),
    };
    parts.extend(order.map(str::to_owned));
    parts.join(" · ")
}

/// The years of the library inside the query's range: the server's own list
/// where it gave one, else the range spelled out. `None` for no year filter;
/// an empty list means no year of the library is in range, and must not be
/// sent (Jellyfin reads no years as every year).
pub fn years_in_range(
    query: &MusicQuery,
    library_years: &[i32],
    this_year: i32,
) -> Option<Vec<i32>> {
    if query.from_year.is_none() && query.to_year.is_none() {
        return None;
    }
    let from = query.from_year.unwrap_or(i32::MIN);
    let to = query.to_year.unwrap_or(i32::MAX);
    let mut years: Vec<i32> = if library_years.is_empty() {
        let to = to.min(this_year);
        let from = from.max(to - MAX_YEARS + 1);
        (from..=to).collect()
    } else {
        library_years
            .iter()
            .copied()
            .filter(|y| (from..=to).contains(y))
            .collect()
    };
    years.sort_unstable();
    years.dedup();
    Some(years)
}

/// The Jellyfin query for a model's filters: songs in the library, the
/// named genres by id, the years, favourites, the term, the order. `None`
/// when the years rule every song out.
pub fn song_query(
    query: &MusicQuery,
    genres: &[NameId],
    library_years: &[i32],
    library: Option<&str>,
    this_year: i32,
) -> Option<ListQuery> {
    let years = match years_in_range(query, library_years, this_year) {
        Some(years) if years.is_empty() => return None,
        Some(years) => years,
        None => Vec::new(),
    };
    let genre_ids = query
        .genres
        .iter()
        .filter_map(|name| {
            genres
                .iter()
                .find(|g| g.name.eq_ignore_ascii_case(name))
                .map(|g| g.id.clone())
        })
        .collect();
    let (sort, order) = match query.ordering {
        Ordering::ByName => (Sort::Name, SortOrder::Ascending),
        Ordering::MostPlayed => (Sort::PlayCount, SortOrder::Descending),
        Ordering::NewestFirst => (Sort::ReleaseDate, SortOrder::Descending),
        Ordering::Random => (Sort::Random, SortOrder::Ascending),
        Ordering::RecentlyPlayed => (Sort::DatePlayed, SortOrder::Descending),
    };
    Some(ListQuery {
        parent_id: library.map(str::to_owned),
        search_term: query.search_term.clone(),
        genre_ids,
        years,
        favorites_only: query.favorites_only,
        sort,
        order,
        limit: Some(DESCRIBED_SONGS),
        ..ListQuery::default()
    })
}

fn this_year() -> i32 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    // Close enough for a bound: 1970 plus whole Julian years.
    1970 + i32::try_from(days * 4 / 1461).unwrap_or(0)
}

/// A phrase, through the model, to Jellyfin's songs.
pub async fn search_by_description(
    openrouter: OpenRouter,
    keyring: Arc<dyn CredentialStore>,
    model: String,
    client: JellyfinClient,
    library: Option<String>,
    phrase: String,
) -> Result<Described, Failure> {
    let key = read_key(keyring).await.map_err(Failure::Other)?;
    let filters = client
        .query_filters(library.as_deref(), &[ItemKind::Audio])
        .await
        .map_err(Failure::Server)?;
    let names: Vec<String> = filters.genres.iter().map(|g| g.name.clone()).collect();
    let outcome = openrouter
        .translate_search(&key, &model, &phrase, &names)
        .await;
    let Some(query) = outcome.query.filter(|q| !q.is_empty()) else {
        return Ok(Described {
            understood: None,
            songs: Vec::new(),
            note: Some(outcome.reason.unwrap_or_else(|| {
                "That description gave nothing to filter by, so the plain search stands.".to_owned()
            })),
        });
    };
    let words = understood(&query);
    let Some(list) = song_query(
        &query,
        &filters.genres,
        &filters.years,
        library.as_deref(),
        this_year(),
    ) else {
        return Ok(Described {
            understood: Some(words),
            songs: Vec::new(),
            note: Some("Nothing in your library is from those years.".to_owned()),
        });
    };
    let page = client.songs(&list).await.map_err(Failure::Server)?;
    Ok(Described {
        understood: Some(words),
        songs: page.items.iter().map(TrackMeta::from_item).collect(),
        note: None,
    })
}

#[cfg(test)]
mod tests {
    use aoide_integrations::openrouter::{DEFAULT_MODEL, KEYRING_ENTRY};

    use super::*;
    use crate::credentials::MemoryStore;

    fn query() -> MusicQuery {
        MusicQuery::default()
    }

    fn genres() -> Vec<NameId> {
        ["Folk", "Indie", "Ambient"]
            .iter()
            .map(|g| NameId {
                id: format!("g-{g}"),
                name: (*g).to_owned(),
            })
            .collect()
    }

    #[test]
    fn filters_read_as_words() {
        let q = MusicQuery {
            genres: vec!["Folk".into(), "Indie".into()],
            from_year: Some(2010),
            to_year: Some(2019),
            favorites_only: true,
            ordering: Ordering::NewestFirst,
            search_term: Some("harbour".into()),
        };
        assert_eq!(
            understood(&q),
            "“harbour” · Folk, Indie · 2010–2019 · favourites · newest first"
        );
        let one_year = MusicQuery {
            from_year: Some(1999),
            to_year: Some(1999),
            ..query()
        };
        assert_eq!(understood(&one_year), "1999");
        let open = MusicQuery {
            from_year: Some(2020),
            ordering: Ordering::Random,
            ..query()
        };
        assert_eq!(understood(&open), "from 2020 · in random order");
    }

    #[test]
    fn years_come_from_the_library_inside_the_range() {
        let library = [2023, 2022, 2021, 2019, 2018, 2015];
        let q = MusicQuery {
            from_year: Some(2016),
            to_year: Some(2021),
            ..query()
        };
        assert_eq!(
            years_in_range(&q, &library, 2026),
            Some(vec![2018, 2019, 2021])
        );
        assert_eq!(years_in_range(&query(), &library, 2026), None);
        // Nothing in range: an empty list, which the query must not send.
        let none = MusicQuery {
            from_year: Some(1960),
            to_year: Some(1969),
            ..query()
        };
        assert_eq!(years_in_range(&none, &library, 2026), Some(vec![]));
        // A server that lists no years: the range spelled out, up to now.
        let open = MusicQuery {
            from_year: Some(2024),
            ..query()
        };
        assert_eq!(
            years_in_range(&open, &[], 2026),
            Some(vec![2024, 2025, 2026])
        );
        let up_to = MusicQuery {
            to_year: Some(1999),
            ..query()
        };
        let spelled = years_in_range(&up_to, &[], 2026).unwrap();
        assert_eq!(spelled.len(), MAX_YEARS as usize);
        assert_eq!(spelled.last(), Some(&1999));
    }

    #[test]
    fn the_jellyfin_query_carries_every_filter() {
        let q = MusicQuery {
            genres: vec!["folk".into(), "Jazz".into()],
            from_year: Some(2018),
            to_year: Some(2019),
            favorites_only: true,
            ordering: Ordering::MostPlayed,
            search_term: Some("lights".into()),
        };
        let list = song_query(&q, &genres(), &[2019, 2015], Some("lib1"), 2026).unwrap();
        assert_eq!(list.genre_ids, ["g-Folk"]);
        assert_eq!(list.years, [2019]);
        assert!(list.favorites_only);
        assert_eq!(list.search_term.as_deref(), Some("lights"));
        assert_eq!(list.parent_id.as_deref(), Some("lib1"));
        assert_eq!(
            (list.sort, list.order),
            (Sort::PlayCount, SortOrder::Descending)
        );
        assert_eq!(list.limit, Some(DESCRIBED_SONGS));
        let out_of_range = MusicQuery {
            from_year: Some(1950),
            to_year: Some(1959),
            ..q
        };
        assert_eq!(
            song_query(&out_of_range, &genres(), &[2019], None, 2026),
            None
        );
    }

    #[test]
    fn orderings_sort_the_way_the_phone_does() {
        let sorted = |ordering| {
            let list = song_query(
                &MusicQuery {
                    ordering,
                    ..query()
                },
                &[],
                &[],
                None,
                2026,
            )
            .unwrap();
            (list.sort, list.order)
        };
        assert_eq!(sorted(Ordering::ByName), (Sort::Name, SortOrder::Ascending));
        assert_eq!(
            sorted(Ordering::NewestFirst),
            (Sort::ReleaseDate, SortOrder::Descending)
        );
        assert_eq!(
            sorted(Ordering::Random),
            (Sort::Random, SortOrder::Ascending)
        );
        assert_eq!(
            sorted(Ordering::RecentlyPlayed),
            (Sort::DatePlayed, SortOrder::Descending)
        );
    }

    fn jellyfin(server: &wiremock::MockServer) -> JellyfinClient {
        JellyfinClient::new(
            aoide_jellyfin::ServerAddress::parse(&server.uri()).unwrap(),
            aoide_jellyfin::Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(aoide_jellyfin::Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "tester".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_description_becomes_a_jellyfin_query_and_only_words_leave() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/Filters2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Genres": [{"Id": "g-folk", "Name": "Folk"}, {"Id": "g-indie", "Name": "Indie"}],
                "Tags": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items/Filters"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Years": [2015, 2019, 2022]})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("genreIds", "g-folk"))
            .and(query_param("years", "2015,2019"))
            .and(query_param("includeItemTypes", "Audio"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [{"Id": "t1", "Name": "Harbour Lights", "Type": "Audio"}],
                "TotalRecordCount": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        let reply = serde_json::json!({"genres": ["folk"], "fromYear": 2010, "toYear": 2019,
            "favoritesOnly": false, "searchTerm": null, "ordering": "byName"});
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-or-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": reply.to_string()}}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let keyring: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
        keyring.set(KEYRING_ENTRY, "sk-or-test").unwrap();
        let openrouter = OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap();
        let found = search_by_description(
            openrouter,
            keyring,
            DEFAULT_MODEL.into(),
            jellyfin(&server),
            Some("lib1".into()),
            "calm folk from the twenty tens".into(),
        )
        .await
        .unwrap();
        assert_eq!(found.understood.as_deref(), Some("Folk · 2010–2019"));
        assert_eq!(found.songs.len(), 1);
        assert_eq!(found.songs[0].title, "Harbour Lights");
        assert_eq!(found.note, None);
        // What went to OpenRouter: the phrase and the genre names, nothing else
        // of the library's.
        let requests = server.received_requests().await.unwrap();
        let sent: Vec<_> = requests
            .iter()
            .filter(|r| r.url.path() == "/api/v1/chat/completions")
            .collect();
        assert_eq!(sent.len(), 1);
        let body = String::from_utf8_lossy(&sent[0].body);
        assert!(body.contains("calm folk from the twenty tens"));
        assert!(body.contains("Folk, Indie"));
        assert!(!body.contains("Harbour Lights"));
        assert!(!body.contains("lib1"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_key_asks_nothing_of_anyone() {
        let server = wiremock::MockServer::start().await;
        let openrouter = OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap();
        let result = search_by_description(
            openrouter,
            Arc::new(MemoryStore::default()),
            DEFAULT_MODEL.into(),
            jellyfin(&server),
            None,
            "calm folk from the twenty tens".into(),
        )
        .await;
        let Err(Failure::Other(reason)) = result else {
            panic!("no key is a reason, not a request");
        };
        assert!(reason.contains("No OpenRouter key"), "{reason}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn this_year_is_plausible() {
        let year = this_year();
        assert!((2025..2200).contains(&year), "{year}");
    }
}
