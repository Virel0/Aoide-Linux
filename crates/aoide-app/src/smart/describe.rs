//! Search by description: a phrase, through the model, to filters, and the
//! filters to an ordinary Melete query, so the songs are always the server's.
//! The orders Melete can't sort songs by (newest, most or last played,
//! random) are put on a pool of what matches, here.

use std::sync::Arc;

use aoide_integrations::openrouter::OpenRouter;
use aoide_integrations::openrouter::smart_search::{MusicQuery, Ordering};
use aoide_melete::{Item, MeleteClient, Sort};

use super::read_key;
use crate::credentials::CredentialStore;
use crate::playback::tracks::TrackMeta;
use crate::song_filter::SongFilter;
use crate::store_worker::StoreHandle;

/// How many songs a description finds at most.
pub const DESCRIBED_SONGS: usize = 100;
/// How many matching songs are read to put in an order of the app's own.
const ORDER_POOL: usize = 500;

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
    Server(aoide_melete::Error),
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
        parts.push("liked".to_owned());
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
/// an empty list means no year of the library is in range (no years would
/// read as every year).
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

/// The filter for a model's filters: the named genres as the library spells
/// them, the years, likes, the term. `None` when the years rule every song
/// out.
pub fn song_filter(
    query: &MusicQuery,
    library_genres: &[String],
    library_years: &[i32],
    this_year: i32,
) -> Option<SongFilter> {
    let years = match years_in_range(query, library_years, this_year) {
        Some(years) if years.is_empty() => return None,
        Some(years) => years,
        None => Vec::new(),
    };
    let genres = query
        .genres
        .iter()
        .filter_map(|name| {
            library_genres
                .iter()
                .find(|g| g.eq_ignore_ascii_case(name))
                .cloned()
        })
        .collect();
    Some(SongFilter {
        term: query.search_term.clone(),
        genres,
        years,
        liked_only: query.favorites_only,
        sort: match query.ordering {
            Ordering::ByName => Sort::Name,
            _ => Sort::Default,
        },
    })
}

/// The songs in the order asked for, at most [`DESCRIBED_SONGS`]: by name as
/// Melete sent them, else newest, most or last played (this device's
/// history), or shuffled.
async fn ordered(songs: Vec<Item>, ordering: Ordering, store: &StoreHandle) -> Vec<Item> {
    let mut songs = match ordering {
        Ordering::ByName => songs,
        Ordering::Random => crate::screens::home::pick(songs, DESCRIBED_SONGS),
        Ordering::NewestFirst => {
            let mut songs = songs;
            songs.sort_by_key(|song| std::cmp::Reverse(song.year));
            songs
        }
        Ordering::MostPlayed | Ordering::RecentlyPlayed => {
            let ids: Vec<String> = songs.iter().map(|song| song.id.clone()).collect();
            let stats = store
                .call(move |w| w.store.stats_for(&ids).unwrap_or_default())
                .await
                .unwrap_or_default();
            let mut songs = songs;
            songs.sort_by_key(|song| {
                let stat = stats.get(&song.id);
                std::cmp::Reverse(match ordering {
                    Ordering::MostPlayed => stat.map_or(0, |s| s.play_count),
                    _ => stat.and_then(|s| s.last_played_at).unwrap_or(0),
                })
            });
            songs
        }
    };
    songs.truncate(DESCRIBED_SONGS);
    songs
}

fn this_year() -> i32 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    // Close enough for a bound: 1970 plus whole Julian years.
    1970 + i32::try_from(days * 4 / 1461).unwrap_or(0)
}

/// A phrase, through the model, to Melete's songs.
pub async fn search_by_description(
    openrouter: OpenRouter,
    keyring: Arc<dyn CredentialStore>,
    model: String,
    client: MeleteClient,
    store: StoreHandle,
    phrase: String,
) -> Result<Described, Failure> {
    let key = read_key(keyring).await.map_err(Failure::Other)?;
    let filters = crate::song_filter::library_filters(&client)
        .await
        .map_err(Failure::Server)?;
    let outcome = openrouter
        .translate_search(&key, &model, &phrase, &filters.genres)
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
    let Some(filter) = song_filter(&query, &filters.genres, &filters.years, this_year()) else {
        return Ok(Described {
            understood: Some(words),
            songs: Vec::new(),
            note: Some("Nothing in your library is from those years.".to_owned()),
        });
    };
    let pool = match query.ordering {
        Ordering::ByName => DESCRIBED_SONGS,
        _ => ORDER_POOL,
    };
    let songs = filter
        .songs(&client, Some(pool))
        .await
        .map_err(Failure::Server)?;
    let songs = ordered(songs, query.ordering, &store).await;
    Ok(Described {
        understood: Some(words),
        songs: songs.iter().map(TrackMeta::from_item).collect(),
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

    fn genres() -> Vec<String> {
        ["Folk", "Indie", "Ambient"].map(String::from).to_vec()
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
            "“harbour” · Folk, Indie · 2010–2019 · liked · newest first"
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
    fn the_filter_carries_every_filter() {
        let q = MusicQuery {
            genres: vec!["folk".into(), "Jazz".into()],
            from_year: Some(2018),
            to_year: Some(2019),
            favorites_only: true,
            ordering: Ordering::MostPlayed,
            search_term: Some("lights".into()),
        };
        let filter = song_filter(&q, &genres(), &[2019, 2015], 2026).unwrap();
        assert_eq!(
            filter.genres,
            ["Folk"],
            "as the library spells it; Jazz it hasn't"
        );
        assert_eq!(filter.years, [2019]);
        assert!(filter.liked_only);
        assert_eq!(filter.term.as_deref(), Some("lights"));
        assert_eq!(filter.sort, Sort::Default);
        let by_name = MusicQuery {
            ordering: Ordering::ByName,
            ..q.clone()
        };
        assert_eq!(
            song_filter(&by_name, &genres(), &[2019], 2026)
                .unwrap()
                .sort,
            Sort::Name
        );
        let out_of_range = MusicQuery {
            from_year: Some(1950),
            to_year: Some(1959),
            ..q
        };
        assert_eq!(song_filter(&out_of_range, &genres(), &[2019], 2026), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn orders_melete_cant_sort_by_are_put_here() {
        let song = |id: &str, year: i32| Item {
            id: id.into(),
            year: Some(year),
            ..Item::default()
        };
        let store = StoreHandle::in_memory("desk").unwrap();
        let songs = vec![song("1", 2001), song("2", 2020), song("3", 2010)];
        let newest = ordered(songs.clone(), Ordering::NewestFirst, &store).await;
        let ids: Vec<&str> = newest.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["2", "3", "1"]);
        let random = ordered(songs, Ordering::Random, &store).await;
        assert_eq!(random.len(), 3);
    }

    fn melete(server: &wiremock::MockServer) -> MeleteClient {
        MeleteClient::new(
            aoide_melete::ServerAddress::parse(&server.uri()).unwrap(),
            aoide_melete::Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(aoide_melete::Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "tester".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_description_becomes_a_melete_query_and_only_words_leave() {
        use crate::test_json::{page, track};
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/genres"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"name": "Folk", "tracks": 3, "albums": 1}, {"name": "Indie", "tracks": 1, "albums": 1}
            ])))
            .mount(&server)
            .await;
        let album = |id: i64, year: i32| {
            serde_json::json!({"id": id, "title": "A", "artist_id": 1, "artist": "B",
                "year": year, "tracks": 1, "duration_seconds": 1.0, "cover": null})
        };
        Mock::given(method("GET"))
            .and(path("/api/v1/music/albums"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![
                album(1, 2015),
                album(2, 2019),
                album(3, 2022),
            ])))
            .mount(&server)
            .await;
        let mut lights = track(1, "Harbour Lights");
        lights["year"] = serde_json::json!(2019);
        let mut late = track(2, "Later");
        late["year"] = serde_json::json!(2022);
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("genre", "Folk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![lights, late])))
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
            melete(&server),
            StoreHandle::in_memory("desk").unwrap(),
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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_key_asks_nothing_of_anyone() {
        let server = wiremock::MockServer::start().await;
        let openrouter = OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap();
        let result = search_by_description(
            openrouter,
            Arc::new(MemoryStore::default()),
            DEFAULT_MODEL.into(),
            melete(&server),
            StoreHandle::in_memory("desk").unwrap(),
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
