//! Collections: saved filters. A filter picks genres, years and tags (from
//! what the library actually has, `query_filters`) and optionally only
//! favourites, over songs or albums; saved under a name it is kept in the
//! config file and listed in the sidebar. Jellyfin has no saved filters of its
//! own (its collections are box sets), so they stay on this computer.

use aoide_jellyfin::{Item, ItemKind, ListQuery, QueryFilters, Sort, SortOrder};
use iced::widget::Column;
use iced::{Element, Fill, Task};
use serde::{Deserialize, Serialize};

use super::{Ctx, Load, PageMsg, Section, fetch};
use crate::app::{App, Message as AppMessage};
use crate::artwork::ArtKey;
use crate::failure::Failure;
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;

mod view;

/// How many results a filter shows.
pub const RESULTS: u32 = 200;

/// What a collection lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    #[default]
    Songs,
    Albums,
}

/// A saved filter, as the config file keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SavedFilter {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Genre item ids, with their names beside them for showing.
    pub genre_ids: Vec<String>,
    pub genre_names: Vec<String>,
    pub years: Vec<i32>,
    pub tags: Vec<String>,
    pub favourites_only: bool,
}

impl SavedFilter {
    /// The list query this filter is, inside `library`.
    pub fn query(&self, library: Option<&str>) -> ListQuery {
        ListQuery {
            parent_id: library.map(str::to_owned),
            genre_ids: self.genre_ids.clone(),
            years: self.years.clone(),
            tags: self.tags.clone(),
            favorites_only: self.favourites_only,
            sort: match self.kind {
                Kind::Songs => Sort::Name,
                Kind::Albums => Sort::Artist,
            },
            order: SortOrder::Ascending,
            limit: Some(RESULTS),
            ..ListQuery::default()
        }
    }

    /// "Folk · 2019, 2022 · late night · favourites".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.genre_names.is_empty() {
            parts.push(self.genre_names.join(", "));
        }
        if !self.years.is_empty() {
            let years: Vec<String> = self.years.iter().map(i32::to_string).collect();
            parts.push(years.join(", "));
        }
        if !self.tags.is_empty() {
            parts.push(self.tags.join(", "));
        }
        if self.favourites_only {
            parts.push("favourites".to_owned());
        }
        if parts.is_empty() {
            "Everything".to_owned()
        } else {
            parts.join(" · ")
        }
    }

    pub fn toggle_genre(&mut self, id: &str, name: &str) {
        match self.genre_ids.iter().position(|g| g == id) {
            Some(at) => {
                self.genre_ids.remove(at);
                if at < self.genre_names.len() {
                    self.genre_names.remove(at);
                }
            }
            None => {
                self.genre_ids.push(id.to_owned());
                self.genre_names.push(name.to_owned());
            }
        }
    }

    pub fn toggle_year(&mut self, year: i32) {
        toggle(&mut self.years, year);
    }

    pub fn toggle_tag(&mut self, tag: &str) {
        toggle(&mut self.tags, tag.to_owned());
    }
}

fn toggle<T: PartialEq>(list: &mut Vec<T>, value: T) {
    match list.iter().position(|v| *v == value) {
        Some(at) => {
            list.remove(at);
        }
        None => list.push(value),
    }
}

/// The app's side: the config holds the collections.
#[derive(Debug, Clone)]
pub enum Msg {
    /// The page with this key opens the saved collection with this id.
    Load {
        key: u64,
        id: String,
    },
    Save(SavedFilter),
    Delete(String),
}

#[derive(Debug)]
pub struct State {
    filter: SavedFilter,
    /// Saved already: the page is that collection.
    saved: bool,
    filters: Load<QueryFilters>,
    songs: Load<Vec<TrackMeta>>,
    albums: Load<Vec<Tile>>,
    name: String,
    confirm_delete: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Filters(Result<QueryFilters, Failure>),
    Saved(Option<SavedFilter>),
    Kind(Kind),
    Genre(String, String),
    Year(i32),
    Tag(String),
    Favourites(bool),
    Songs(Result<Vec<Item>, Failure>),
    Albums(Result<Vec<Item>, Failure>),
    Name(String),
    Save,
    Delete,
    ConfirmDelete(bool),
}

/// A new filter, or the saved collection `id`.
pub fn open(ctx: &Ctx<'_>, id: Option<&str>) -> (State, Task<AppMessage>) {
    let mut state = State {
        filter: SavedFilter::default(),
        saved: false,
        filters: Load::Loading,
        songs: Load::Loading,
        albums: Load::Loading,
        name: String::new(),
        confirm_delete: false,
    };
    let task = match id {
        Some(id) => Task::done(AppMessage::Collections(Msg::Load {
            key: ctx.key,
            id: id.to_owned(),
        })),
        None => Task::batch([state.load_filters(ctx), state.load_results(ctx)]),
    };
    (state, task)
}

impl State {
    fn load_filters(&self, ctx: &Ctx<'_>) -> Task<AppMessage> {
        let client = ctx.client.clone();
        let library = ctx.library.map(str::to_owned);
        let kind = match self.filter.kind {
            Kind::Songs => ItemKind::Audio,
            Kind::Albums => ItemKind::MusicAlbum,
        };
        fetch(
            ctx.key,
            async move { client.query_filters(library.as_deref(), &[kind]).await },
            |r| PageMsg::Collections(Message::Filters(r)),
        )
    }

    fn load_results(&mut self, ctx: &Ctx<'_>) -> Task<AppMessage> {
        let client = ctx.client.clone();
        let query = self.filter.query(ctx.library);
        match self.filter.kind {
            Kind::Songs => {
                self.songs = Load::Loading;
                fetch(
                    ctx.key,
                    async move { client.songs(&query).await.map(|p| p.items) },
                    |r| PageMsg::Collections(Message::Songs(r)),
                )
            }
            Kind::Albums => {
                self.albums = Load::Loading;
                fetch(
                    ctx.key,
                    async move { client.albums(&query).await.map(|p| p.items) },
                    |r| PageMsg::Collections(Message::Albums(r)),
                )
            }
        }
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<AppMessage> {
        match message {
            Message::Filters(result) => {
                self.filters = Load::from(result);
                return Task::none();
            }
            Message::Saved(None) => {
                self.filters = Load::Failed("That collection has been deleted.".into());
                return Task::none();
            }
            Message::Saved(Some(filter)) => return self.saved(filter, ctx),
            Message::Kind(kind) => {
                if kind == self.filter.kind {
                    return Task::none();
                }
                self.filter.kind = kind;
                return Task::batch([self.load_filters(ctx), self.load_results(ctx)]);
            }
            Message::Genre(id, name) => self.filter.toggle_genre(&id, &name),
            Message::Year(year) => self.filter.toggle_year(year),
            Message::Tag(tag) => self.filter.toggle_tag(&tag),
            Message::Favourites(on) => self.filter.favourites_only = on,
            Message::Songs(result) => {
                self.songs = Load::from(
                    result.map(|items| items.iter().map(TrackMeta::from_item).collect()),
                );
                return Task::none();
            }
            Message::Albums(result) => {
                self.albums =
                    Load::from(result.map(|items| items.iter().map(Tile::album).collect()));
                return Task::none();
            }
            Message::Name(name) => {
                self.name = name;
                return Task::none();
            }
            Message::Save => return self.save(),
            Message::Delete => {
                self.confirm_delete = true;
                return Task::none();
            }
            Message::ConfirmDelete(false) => {
                self.confirm_delete = false;
                return Task::none();
            }
            Message::ConfirmDelete(true) => {
                return Task::done(AppMessage::Collections(Msg::Delete(self.filter.id.clone())));
            }
        }
        // A filter changed: the results follow.
        self.load_results(ctx)
    }

    /// The saved collection this page opens, read from the config.
    fn saved(&mut self, filter: SavedFilter, ctx: &Ctx<'_>) -> Task<AppMessage> {
        self.name.clone_from(&filter.name);
        self.filter = filter;
        self.saved = true;
        Task::batch([self.load_filters(ctx), self.load_results(ctx)])
    }

    /// Saves the filter under the name typed, as a new collection or over
    /// the one this page is.
    fn save(&mut self) -> Task<AppMessage> {
        let name = self.name.trim().to_owned();
        if name.is_empty() {
            return Task::none();
        }
        let mut filter = self.filter.clone();
        filter.name = name;
        if filter.id.is_empty() {
            filter.id = aoide_store::new_id();
        }
        self.filter = filter.clone();
        self.saved = true;
        Task::done(AppMessage::Collections(Msg::Save(filter)))
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        match self.filter.kind {
            Kind::Songs => self
                .songs
                .ready()
                .map(|t| super::track_art(t))
                .unwrap_or_default(),
            Kind::Albums => self
                .albums
                .ready()
                .map(|t| t.iter().filter_map(|t| t.art.clone()).collect())
                .unwrap_or_default(),
        }
    }
}

impl App {
    pub(crate) fn collections(&mut self, message: Msg) -> Task<AppMessage> {
        match message {
            Msg::Load { key, id } => {
                let found = self.config.collections.iter().find(|c| c.id == id).cloned();
                Task::done(AppMessage::Page(
                    key,
                    PageMsg::Collections(Message::Saved(found)),
                ))
            }
            Msg::Save(filter) => {
                let id = filter.id.clone();
                match self.config.collections.iter_mut().find(|c| c.id == id) {
                    Some(existing) => *existing = filter,
                    None => self.config.collections.push(filter),
                }
                self.save_config();
                let showing = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.section == Section::Collection(id.clone()));
                if showing {
                    Task::none()
                } else {
                    Task::done(AppMessage::Section(Section::Collection(id)))
                }
            }
            Msg::Delete(id) => {
                self.config.collections.retain(|c| c.id != id);
                self.save_config();
                Task::done(AppMessage::Section(Section::Collections))
            }
        }
    }
}

/// The sidebar's Collections: a way to make one, and the saved ones.
pub fn sidebar<'a>(
    collections: &'a [SavedFilter],
    item: impl Fn(String, Section) -> Element<'a, AppMessage>,
) -> Column<'a, AppMessage> {
    let mut list = Column::new().spacing(2);
    for collection in collections {
        list = list.push(item(
            collection.name.clone(),
            Section::Collection(collection.id.clone()),
        ));
    }
    list.width(Fill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_is_the_list_query_it_names() {
        let mut filter = SavedFilter {
            kind: Kind::Albums,
            ..SavedFilter::default()
        };
        assert_eq!(filter.summary(), "Everything");
        filter.toggle_genre("g-Folk", "Folk");
        filter.toggle_year(2019);
        filter.toggle_year(2022);
        filter.toggle_tag("late night");
        filter.favourites_only = true;
        let query = filter.query(Some("lib1"));
        assert_eq!(query.genre_ids, ["g-Folk"]);
        assert_eq!(query.years, [2019, 2022]);
        assert_eq!(query.tags, ["late night"]);
        assert!(query.favorites_only);
        assert_eq!(query.parent_id.as_deref(), Some("lib1"));
        assert_eq!(
            filter.summary(),
            "Folk · 2019, 2022 · late night · favourites"
        );
        filter.toggle_genre("g-Folk", "Folk");
        filter.toggle_year(2019);
        assert!(filter.genre_names.is_empty());
        assert_eq!(filter.years, [2022]);
    }

    #[test]
    fn a_collection_survives_the_config_file() {
        let filter = SavedFilter {
            id: "c1".into(),
            name: "Folk evenings".into(),
            kind: Kind::Songs,
            genre_ids: vec!["g-Folk".into()],
            genre_names: vec!["Folk".into()],
            years: vec![2022],
            tags: Vec::new(),
            favourites_only: false,
        };
        let json = serde_json::to_string(&filter).unwrap();
        assert!(json.contains("\"genreIds\""), "{json}");
        assert_eq!(serde_json::from_str::<SavedFilter>(&json).unwrap(), filter);
        // An older file without a field still reads.
        let old: SavedFilter = serde_json::from_str(r#"{"id":"c2","name":"x"}"#).unwrap();
        assert_eq!(old.kind, Kind::Songs);
    }
}
