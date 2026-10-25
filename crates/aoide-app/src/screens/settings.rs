//! Settings: exactly the Round 1 rows — Crossfade, Keep albums whole,
//! Infinity, Skip silence, Level volume, Lyric offset, Hidden from mixes, and
//! the account with Sync Now.

use iced::widget::{Column, button, column, container, row, slider, text, toggler};
use iced::{Center, Element, Fill, Task};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Crossfade,
    KeepAlbumsWhole,
    Infinity,
    SkipSilence,
    LevelVolume,
}

impl Toggle {
    pub fn apply(self, settings: &mut Settings, on: bool) {
        match self {
            Toggle::Crossfade => settings.crossfade = on,
            Toggle::KeepAlbumsWhole => settings.keep_albums_whole = on,
            Toggle::Infinity => settings.infinity = on,
            Toggle::SkipSilence => settings.skip_silence = on,
            Toggle::LevelVolume => settings.level_volume = on,
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
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    (
        State {
            confirm_sign_out: false,
            hidden: Load::Loading,
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
    let mut block = column![
        text(title)
            .size(13)
            .font(widgets::SEMIBOLD)
            .style(|t| text::Style {
                color: Some(theme::secondary(t)),
            }),
        container(rows.spacing(12))
            .padding(14)
            .width(Fill)
            .style(theme::menu),
    ]
    .spacing(8);
    if !footer.is_empty() {
        block = block.push(widgets::secondary(footer).size(12));
    }
    block.into()
}

pub fn view<'a>(
    state: &'a State,
    settings: &Settings,
    account: Option<&'a Account>,
    sync: crate::sync::Status<'a>,
    view: &View<'a>,
    system: Element<'a, App>,
    extras: crate::listening::SettingsExtras<'a>,
) -> Element<'a, App> {
    let key = view.key;
    let message = move |m: Message| App::Page(key, PageMsg::Settings(m));
    let switch = |label: &'a str, value: bool, which: Toggle| {
        toggler(value)
            .label(label)
            .width(Fill)
            .on_toggle(move |on| message(Message::Toggle(which, on)))
    };

    let playback = section(
        "Playback",
        column![
            switch("Crossfade", settings.crossfade, Toggle::Crossfade),
            switch(
                "Keep Albums Whole",
                settings.keep_albums_whole,
                Toggle::KeepAlbumsWhole
            ),
            switch("Infinity", settings.infinity, Toggle::Infinity),
            crate::screens::equalizer::settings_row(&extras.config.sound),
            crate::player_menu::transcode_rows(&extras.config.sound.transcode),
        ],
        "Crossfade fades one song into the next instead of stopping between them, longer when \
         the two are at the same tempo. Keep Albums Whole leaves consecutive tracks of one album \
         running straight into each other, the way they were sequenced. Infinity keeps the queue \
         going when it runs out, with songs your server picks to follow the one playing.",
    );
    let sound = section(
        "Sound",
        column![
            switch("Skip Silence", settings.skip_silence, Toggle::SkipSilence),
            switch("Level Volume", settings.level_volume, Toggle::LevelVolume),
            crate::player_menu::device_row(&extras),
        ],
        "Skip Silence plays only the span of each track your server measured as sound. Level \
         Volume turns loud records down to the same loudness; a quieter track is left alone, \
         never turned up. A file with its own ReplayGain tags plays by them instead, album gain \
         first, never louder than its peak allows. Tracks with neither play as they are.",
    );
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
        crate::lyrics_online::settings_row(&extras),
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
    let lyrics = section(
        "Lyrics",
        lyrics_rows,
        "Nudge the words if they run ahead of or behind the music.",
    );

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
    let hidden = section(
        "Hidden from Mixes",
        hidden_rows,
        "Songs marked Not Interested stay out of Infinity; songs marked Don't Count Plays leave \
         your history alone. Clear a song to undo both.",
    );

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
    account_rows = account_rows.push(
        row![
            column![text("Sync"), widgets::secondary(sync_line).size(13)]
                .spacing(2)
                .width(Fill),
            button(text(if sync.running {
                "Syncing…"
            } else {
                "Sync Now"
            }))
            .style(theme::tonal)
            .padding([6, 14])
            .on_press_maybe((sync.available && !sync.running).then_some(App::SyncNow)),
        ]
        .spacing(10)
        .align_y(Center),
    );
    account_rows = account_rows.push(if state.confirm_sign_out {
        Element::from(
            column![
                text("Sign out of Aoide? You'll need your server address and password to sign back in."),
                row![
                    button("Sign Out")
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
        button("Sign Out")
            .style(theme::destructive)
            .padding([6, 14])
            .on_press(message(Message::SignOut))
            .into()
    });
    let account = section("Account", account_rows, "");

    widgets::page(
        column![
            widgets::title_bar("Settings", view.can_go_back),
            crate::appearance::general_section(&extras),
            playback,
            sound,
            lyrics,
            hidden,
            system,
            account
        ]
        .spacing(24)
        .max_width(720),
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
        assert_eq!(
            settings,
            Settings {
                crossfade: true,
                keep_albums_whole: false,
                infinity: true,
                skip_silence: false,
                level_volume: false,
                lyric_offset_ms: 0,
            }
        );
    }
}
