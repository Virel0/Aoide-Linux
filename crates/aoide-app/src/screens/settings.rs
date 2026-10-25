//! Settings, as macOS System Settings lays them out: a list of categories
//! and one category's rows at a time, each switch on the right of its label.

use iced::widget::{Column, button, column, container, row, scrollable, slider, text};
use iced::{Center, Element, Fill, Length, Task};

use super::{Ctx, Load, PageMsg, View};
use crate::app::Message as App;
use crate::config::{Account, Settings};
use crate::playback::lyrics;
use crate::theme;
use crate::widgets;

/// A track the listener has flagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hidden {
    pub jellyfin_id: String,
    pub content_key: String,
    pub title: String,
    pub artist: String,
    pub not_interested: bool,
    pub dont_count: bool,
}

#[derive(Debug)]
pub struct State {
    pub confirm_sign_out: bool,
    pub hidden: Load<Vec<Hidden>>,
    /// The category whose rows are shown.
    pub category: Category,
}

/// One pane of Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Category {
    #[default]
    General,
    Playback,
    Sound,
    Lyrics,
    Infinity,
    SmartSearch,
    Sync,
    Server,
    System,
    Account,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::General,
        Category::Playback,
        Category::Sound,
        Category::Lyrics,
        Category::Infinity,
        Category::SmartSearch,
        Category::Sync,
        Category::Server,
        Category::System,
        Category::Account,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Playback => "Playback",
            Category::Sound => "Sound",
            Category::Lyrics => "Lyrics",
            Category::Infinity => "Infinity",
            Category::SmartSearch => "Smart search",
            Category::Sync => "Sync",
            Category::Server => "Server",
            Category::System => "System",
            Category::Account => "Account",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Crossfade,
    KeepAlbumsWhole,
    Infinity,
    SkipSilence,
    LevelVolume,
    InfinityAlbums,
}

impl Toggle {
    pub fn apply(self, settings: &mut Settings, on: bool) {
        match self {
            Toggle::Crossfade => settings.crossfade = on,
            Toggle::KeepAlbumsWhole => settings.keep_albums_whole = on,
            Toggle::Infinity => settings.infinity = on,
            Toggle::SkipSilence => settings.skip_silence = on,
            Toggle::LevelVolume => settings.level_volume = on,
            Toggle::InfinityAlbums => settings.infinity_albums = on,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Hidden(Result<Vec<Hidden>, String>),
    Toggle(Toggle, bool),
    LyricOffset(i32),
    Unhide(String),
    SignOut,
    ConfirmSignOut(bool),
    Category(Category),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    (
        State {
            confirm_sign_out: false,
            hidden: Load::Loading,
            category: Category::default(),
        },
        load_hidden(ctx.store, ctx.key),
    )
}

/// Every flagged track, newest first, named from the track cache.
pub fn load_hidden(store: &crate::store_worker::StoreHandle, key: u64) -> Task<App> {
    let store = store.clone();
    Task::perform(
        async move {
            store
                .call(|w| {
                    w.store
                        .flagged_tracks()
                        .map(|rows| {
                            rows.into_iter()
                                .map(|(flags, track)| Hidden {
                                    title: track.as_ref().map_or_else(
                                        || "Unknown song".to_owned(),
                                        |t| t.title.clone(),
                                    ),
                                    artist: track.map(|t| t.artist).unwrap_or_default(),
                                    jellyfin_id: flags.jellyfin_id,
                                    content_key: flags.content_key,
                                    not_interested: flags.not_interested,
                                    dont_count: flags.dont_count,
                                })
                                .collect()
                        })
                        .map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|| Err("the library is closing".into()))
        },
        move |r| App::Page(key, PageMsg::Settings(Message::Hidden(r))),
    )
}

impl State {
    /// Only what the page holds; the app applies the settings themselves.
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Hidden(result) => self.hidden = Load::from(result),
            Message::SignOut => self.confirm_sign_out = true,
            Message::ConfirmSignOut(false) => self.confirm_sign_out = false,
            Message::Category(category) => {
                self.category = category;
                self.confirm_sign_out = false;
            }
            _ => {}
        }
        Task::none()
    }
}

pub(crate) fn section<'a>(
    title: &'a str,
    rows: Column<'a, App>,
    footer: &'a str,
) -> Element<'a, App> {
    let mut block = column![].spacing(8);
    if !title.is_empty() {
        block = block.push(
            text(title)
                .size(13)
                .font(widgets::SEMIBOLD)
                .style(|t| text::Style {
                    color: Some(theme::secondary(t)),
                }),
        );
    }
    block = block.push(
        container(rows.spacing(12))
            .padding(14)
            .width(Fill)
            .style(theme::menu),
    );
    if !footer.is_empty() {
        block = block.push(widgets::secondary(footer).size(12));
    }
    block.into()
}

pub fn view<'a>(
    state: &'a State,
    sync: crate::sync::Status<'a>,
    view: &View<'a>,
    app: &'a crate::app::App,
    extras: crate::listening::SettingsExtras<'a>,
    wide: bool,
) -> Element<'a, App> {
    let key = view.key;
    let settings = &app.config.settings;
    let account = app.config.account.as_ref();
    let pane: Element<'a, App> = match state.category {
        Category::General => crate::appearance::general_section(&extras),
        Category::Playback => playback_section(settings, &extras, key),
        Category::Sound => sound_section(settings, &extras, key),
        Category::Lyrics => lyrics_section(settings, &extras, key),
        Category::Infinity => column![
            infinity_albums_section(settings, key),
            hidden_section(state, key)
        ]
        .spacing(24)
        .into(),
        Category::SmartSearch => {
            crate::smart::settings_section(&app.smart, &app.config.system.smart_search)
        }
        Category::Sync => sync_section(sync),
        Category::Server => server_section(),
        Category::System => crate::system::settings_sections(app),
        Category::Account => account_section(state, account, key),
    };
    let pane = column![
        text(state.category.title()).size(20).font(widgets::BOLD),
        pane
    ]
    .spacing(16)
    .max_width(640);
    let pane = scrollable(container(pane).padding(iced::Padding {
        top: 0.0,
        right: 28.0,
        bottom: 24.0,
        left: 0.0,
    }))
    .id(widgets::PAGE)
    .height(Fill);
    let body: Element<'a, App> = if wide {
        row![
            container(categories(state.category, key, false))
                .width(Length::Fixed(CATEGORY_LIST_WIDTH)),
            pane
        ]
        .spacing(24)
        .height(Fill)
        .into()
    } else {
        column![categories(state.category, key, true), pane]
            .spacing(16)
            .height(Fill)
            .into()
    };
    column![widgets::title_bar("Settings", view.can_go_back), body]
        .spacing(20)
        .padding(iced::Padding {
            top: 24.0,
            right: 0.0,
            bottom: 0.0,
            left: 28.0,
        })
        .height(Fill)
        .into()
}

/// The list of categories down the left, as System Settings has it; on a
/// narrow window, the same buttons wrapped in rows above the pane.
fn categories<'a>(chosen: Category, key: u64, wrapped: bool) -> Element<'a, App> {
    let item = |category: Category| {
        let selected = category == chosen;
        button(
            text(category.title())
                .size(14)
                .wrapping(text::Wrapping::None),
        )
        .width(if wrapped { Length::Shrink } else { Fill })
        .padding([6, 10])
        .style(if selected {
            theme::row_selected
        } else {
            theme::row
        })
        .on_press(App::Page(
            key,
            PageMsg::Settings(Message::Category(category)),
        ))
    };
    if wrapped {
        let mut line = row![].spacing(4);
        for category in Category::ALL {
            line = line.push(item(category));
        }
        line.wrap().vertical_spacing(4).into()
    } else {
        let mut list = column![].spacing(2);
        for category in Category::ALL {
            list = list.push(item(category));
        }
        list.into()
    }
}

/// The category list's width beside the pane.
const CATEGORY_LIST_WIDTH: f32 = 170.0;

/// Below this much room for the page, the categories wrap above the pane
/// instead of standing beside it.
pub const WIDE_ENOUGH: f32 = 640.0;

fn playback_section<'a>(
    settings: &Settings,
    extras: &crate::listening::SettingsExtras<'a>,
    key: u64,
) -> Element<'a, App> {
    section(
        "",
        column![
            switch("Crossfade", settings.crossfade, Toggle::Crossfade, key),
            switch(
                "Keep albums whole",
                settings.keep_albums_whole,
                Toggle::KeepAlbumsWhole,
                key
            ),
            switch("Infinity", settings.infinity, Toggle::Infinity, key),
            crate::screens::equalizer::settings_row(&extras.config.sound),
            crate::player_menu::transcode_rows(&extras.config.sound.transcode),
        ],
        "Crossfade fades one song into the next instead of stopping between them, longer when \
         the two are at the same tempo. Keep albums whole leaves consecutive tracks of one album \
         running straight into each other, the way they were sequenced. Infinity keeps the queue \
         going when it runs out, with songs your server picks to follow the one playing.",
    )
}

fn sound_section<'a>(
    settings: &Settings,
    extras: &crate::listening::SettingsExtras<'a>,
    key: u64,
) -> Element<'a, App> {
    section(
        "",
        column![
            switch(
                "Skip silence",
                settings.skip_silence,
                Toggle::SkipSilence,
                key
            ),
            switch(
                "Level volume",
                settings.level_volume,
                Toggle::LevelVolume,
                key
            ),
            crate::player_menu::device_row(extras),
        ],
        "Skip silence plays only the span of each track your server measured as sound. Level \
         volume turns loud records down to the same loudness; a quieter track is left alone, \
         never turned up. A file with its own ReplayGain tags plays by them instead, album gain \
         first, never louder than its peak allows. Tracks with neither play as they are.",
    )
}

/// One of the page's own switches.
fn switch<'a>(label: &'a str, value: bool, which: Toggle, key: u64) -> Element<'a, App> {
    widgets::switch(label, value, move |on| {
        App::Page(key, PageMsg::Settings(Message::Toggle(which, on)))
    })
    .into()
}

fn lyrics_section<'a>(
    settings: &Settings,
    extras: &crate::listening::SettingsExtras<'a>,
    key: u64,
) -> Element<'a, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Settings(m));
    let offset = settings.lyric_offset_ms;
    let lyrics_rows = column![
        row![
            text("Timing").width(Fill),
            widgets::secondary(lyrics::offset_label(offset)).size(13),
        ],
        slider(
            -Settings::LYRIC_OFFSET_RANGE_MS..=Settings::LYRIC_OFFSET_RANGE_MS,
            offset,
            move |ms| message(Message::LyricOffset(ms)),
        )
        .step(50),
        crate::lyrics_online::settings_row(extras),
        crate::lyrics_extras::settings_row(extras),
    ];
    let lyrics_rows = if offset != 0 {
        lyrics_rows.push(
            button(text("Reset").size(13))
                .style(theme::tonal)
                .padding([4, 10])
                .on_press(message(Message::LyricOffset(0))),
        )
    } else {
        lyrics_rows
    };
    section(
        "",
        lyrics_rows,
        "Nudge the words if they run ahead of or behind the music.",
    )
}

fn hidden_section(state: &State, key: u64) -> Element<'_, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Settings(m));
    let mut hidden_rows = column![];
    match &state.hidden {
        Load::Loading => hidden_rows = hidden_rows.push(widgets::secondary("Loading…")),
        Load::Failed(error) => hidden_rows = hidden_rows.push(text(error.as_str())),
        Load::Ready(rows) if rows.is_empty() => {
            hidden_rows = hidden_rows.push(widgets::secondary("Nothing hidden."));
        }
        Load::Ready(rows) => {
            for hidden in rows {
                let what = match (hidden.not_interested, hidden.dont_count) {
                    (true, true) => "Not interested · Plays not counted",
                    (true, false) => "Not interested",
                    _ => "Plays not counted",
                };
                hidden_rows = hidden_rows.push(
                    row![
                        column![
                            text(hidden.title.as_str()),
                            widgets::secondary(format!("{} — {what}", hidden.artist)).size(12),
                        ]
                        .width(Fill),
                        button(text("Clear").size(13))
                            .style(theme::tonal)
                            .padding([4, 10])
                            .on_press(message(Message::Unhide(hidden.jellyfin_id.clone()))),
                    ]
                    .align_y(Center),
                );
            }
        }
    }
    section(
        "Hidden from mixes",
        hidden_rows,
        "Songs marked \u{201c}Not interested\u{201d} stay out of Infinity; songs marked \
         \u{201c}Don't count plays\u{201d} leave your history alone. Clear a song to undo both.",
    )
}

fn sync_section(sync: crate::sync::Status<'_>) -> Element<'_, App> {
    let sync_line = if !sync.available {
        "This server has no Aoide sidecar; playlists and history stay on this computer."
    } else if sync.running {
        "Syncing…"
    } else {
        match sync.last {
            Some(Ok(words) | Err(words)) => words.as_str(),
            None => "Not synced yet.",
        }
    };
    let rows = column![
        row![
            column![text("Sync"), widgets::secondary(sync_line).size(13)]
                .spacing(2)
                .width(Fill),
            button(text(if sync.running {
                "Syncing…"
            } else {
                "Sync now"
            }))
            .style(theme::tonal)
            .padding([6, 14])
            .on_press_maybe((sync.available && !sync.running).then_some(App::SyncNow)),
            super::sync_status::settings_link(),
        ]
        .spacing(10)
        .align_y(Center),
    ];
    section("", rows, "")
}

fn account_section<'a>(state: &State, account: Option<&'a Account>, key: u64) -> Element<'a, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Settings(m));
    let mut account_rows = column![];
    if let Some(account) = account {
        account_rows = account_rows
            .push(row![
                text("Server").width(Fill),
                widgets::secondary(account.server.as_str())
            ])
            .push(row![
                text("Signed in as").width(Fill),
                widgets::secondary(account.username.as_str())
            ]);
    }
    account_rows = account_rows.push(if state.confirm_sign_out {
        Element::from(
            column![
                text("Sign out of Aoide? You'll need your server address and password to sign back in."),
                row![
                    button("Sign out")
                        .style(theme::destructive)
                        .padding([6, 14])
                        .on_press(App::SignOut),
                    button("Cancel")
                        .style(theme::tonal)
                        .padding([6, 14])
                        .on_press(message(Message::ConfirmSignOut(false))),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
    } else {
        button("Sign out")
            .style(theme::destructive)
            .padding([6, 14])
            .on_press(message(Message::SignOut))
            .into()
    });
    section("", account_rows, "")
}

/// The server's own maintenance: what an administrator can start from here.
fn server_section<'a>() -> Element<'a, App> {
    section(
        "",
        column![
            row![
                text("Rescan library").width(Fill),
                button(text("Rescan"))
                    .style(theme::tonal)
                    .padding([6, 14])
                    .on_press(App::Rescan(crate::rescan::Msg::Start)),
            ]
            .align_y(Center),
        ],
        "Asks the server to look for new and changed music now, rather than at its next \
         scheduled scan. Only a server administrator can.",
    )
}

/// Infinity's albums mode: its own row and the reason it ranks here.
fn infinity_albums_section<'a>(settings: &Settings, key: u64) -> Element<'a, App> {
    section(
        "",
        column![widgets::switch(
            "Add whole albums",
            settings.infinity_albums,
            move |on| {
                App::Page(
                    key,
                    PageMsg::Settings(Message::Toggle(Toggle::InfinityAlbums, on)),
                )
            }
        ),],
        "When Infinity keeps the queue going, it adds two whole albums at a time instead of \
         single songs: records like the one playing, chosen by what you finish. They are chosen \
         on this computer, because your server's picks are song by song.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_toggle_sets_its_own_setting() {
        let mut settings = Settings::default();
        Toggle::Crossfade.apply(&mut settings, true);
        Toggle::KeepAlbumsWhole.apply(&mut settings, false);
        Toggle::Infinity.apply(&mut settings, true);
        Toggle::SkipSilence.apply(&mut settings, false);
        Toggle::LevelVolume.apply(&mut settings, false);
        Toggle::InfinityAlbums.apply(&mut settings, true);
        assert_eq!(
            settings,
            Settings {
                crossfade: true,
                keep_albums_whole: false,
                infinity: true,
                skip_silence: false,
                level_volume: false,
                lyric_offset_ms: 0,
                infinity_albums: true,
            }
        );
    }
}
