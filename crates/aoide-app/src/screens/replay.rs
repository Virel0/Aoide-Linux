//! Replay: this listener's listening as a story, for this month, this year
//! or all of it. Every figure is the store's `Recap` (computed on this
//! computer, never sent anywhere); the top songs are fetched from the server
//! in rank order so they can play. The iOS `ReplayView`.

use aoide_jellyfin::Item;
use aoide_store::{Calendar, PlaySource, Recap, ReplayPeriod};
use iced::widget::{Column, button, column, container, grid, row, text};
use iced::{Element, Fill, Task};

use super::{Ctx, Load, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::playback::tracks::TrackMeta;
use crate::{theme, widgets};

#[derive(Debug)]
pub struct State {
    period: ReplayPeriod,
    recap: Load<Recap>,
    /// The top songs, in rank order, as far as the server still has them.
    top: Vec<TrackMeta>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Period(ReplayPeriod),
    Recap(ReplayPeriod, Result<Box<Recap>, String>),
    Top(ReplayPeriod, Result<Vec<Item>, String>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let mut state = State {
        // As on the phone: the year is the story most people open this for.
        period: ReplayPeriod::Year,
        recap: Load::Loading,
        top: Vec::new(),
    };
    let task = state.load(ctx);
    (state, task)
}

/// `1,234`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Hours to at most one decimal, without a trailing `.0`: "12", "12.5".
pub fn hours(recap: &Recap) -> String {
    let hours = recap.hours();
    if hours.fract() == 0.0 {
        thousands(hours as i64)
    } else {
        format!("{hours:.1}")
    }
}

pub fn plays(n: i64) -> String {
    if n == 1 {
        "1 play".to_owned()
    } else {
        format!("{} plays", thousands(n))
    }
}

/// "Busiest day: 21 Oct 2026, 12 plays."
pub fn busiest(recap: &Recap) -> Option<String> {
    let date = recap.busiest_date()?;
    let day = recap.busiest_day.as_ref()?;
    Some(format!(
        "Busiest day: {}, {}.",
        date.format("%-d %b %Y"),
        plays(day.play_count)
    ))
}

/// "4 plays were of songs this computer no longer knows…", when there are any.
pub fn unattributed(recap: &Recap) -> Option<String> {
    match recap.unattributed_plays {
        0 => None,
        1 => Some(
            "1 play was of a song this computer no longer knows, and counts only in the totals."
                .to_owned(),
        ),
        n => Some(format!(
            "{} plays were of songs this computer no longer knows, and count only in the totals.",
            thousands(n)
        )),
    }
}

impl State {
    fn load(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        self.recap = Load::Loading;
        self.top.clear();
        let period = self.period;
        let store = ctx.store.clone();
        let key = ctx.key;
        Task::perform(
            async move {
                let now = aoide_store::now_millis();
                store
                    .call(move |w| {
                        w.store
                            .replay(period, now, Calendar::Local)
                            .map(Box::new)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |result| App::Page(key, PageMsg::Replay(Message::Recap(period, result))),
        )
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Period(period) => {
                if period == self.period {
                    return Task::none();
                }
                self.period = period;
                self.load(ctx)
            }
            // An answer for a period no longer shown.
            Message::Recap(period, _) | Message::Top(period, _) if period != self.period => {
                Task::none()
            }
            Message::Recap(period, result) => {
                let ids: Vec<String> = result
                    .as_ref()
                    .map(|r| r.top_tracks.iter().map(|t| t.jellyfin_id.clone()).collect())
                    .unwrap_or_default();
                self.recap = Load::from(result.map(|r| *r));
                if ids.is_empty() {
                    return Task::none();
                }
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.items_by_ids(&ids).await },
                    move |r| PageMsg::Replay(Message::Top(period, r)),
                )
            }
            Message::Top(_, result) => {
                // Without the server the songs are not playable; the numbers stand.
                self.top = result
                    .map(|items| items.iter().map(TrackMeta::from_item).collect())
                    .unwrap_or_default();
                Task::none()
            }
        }
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        super::track_art(&self.top)
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let periods = row(ReplayPeriod::ALL.into_iter().map(|period| {
            button(text(period.label()).size(14))
                .style(if period == self.period {
                    theme::filled
                } else {
                    theme::tonal
                })
                .padding([6, 14])
                .on_press(App::Page(key, PageMsg::Replay(Message::Period(period))))
                .into()
        }))
        .spacing(8);
        let mut body = column![widgets::title_bar("Replay", view.can_go_back), periods].spacing(24);
        let recap = match &self.recap {
            Load::Loading => return widgets::page(body.push(widgets::loading())),
            Load::Failed(error) => return widgets::page(body.push(widgets::failure(error))),
            Load::Ready(recap) => recap,
        };
        if recap.is_empty() {
            return widgets::page(body.push(widgets::empty(
                "Nothing to Replay yet. Play some music and it'll add up here.",
            )));
        }
        let tiles = [
            ("Plays", thousands(recap.total_plays)),
            ("Hours", hours(recap)),
            ("Songs", thousands(recap.distinct_tracks)),
            ("Artists", thousands(recap.distinct_artists)),
        ]
        .into_iter()
        .map(|(label, value)| {
            container(
                column![
                    text(value).size(30).font(widgets::BOLD),
                    widgets::secondary(label.to_uppercase())
                        .size(11)
                        .font(widgets::SEMIBOLD),
                ]
                .spacing(2),
            )
            .padding(14)
            .width(Fill)
            .style(theme::menu)
            .into()
        });
        let mut totals = column![
            grid(tiles)
                .columns(4)
                .spacing(10)
                .height(grid::Sizing::EvenlyDistribute(iced::Length::Fixed(84.0)))
        ]
        .spacing(10);
        if let Some(busiest) = busiest(recap) {
            totals = totals.push(widgets::secondary(busiest).size(13));
        }
        body = body.push(totals);
        if !self.top.is_empty() {
            body = body.push(
                column![
                    text("Top Songs").size(20).font(widgets::BOLD),
                    widgets::track_list(
                        &self.top,
                        view,
                        widgets::ListStyle {
                            numbered: false,
                            show_album: true,
                        },
                        PlaySource::Unknown,
                    ),
                ]
                .spacing(10),
            );
        }
        if !recap.top_artists.is_empty() {
            body = body.push(ranked(
                "Top Artists",
                recap
                    .top_artists
                    .iter()
                    .map(|a| (a.artist.clone(), a.play_count)),
            ));
        }
        if !recap.top_albums.is_empty() {
            body = body.push(ranked(
                "Top Albums",
                recap
                    .top_albums
                    .iter()
                    .map(|a| (format!("{} · {}", a.album, a.artist), a.play_count)),
            ));
        }
        if let Some(note) = unattributed(recap) {
            body = body.push(widgets::secondary(note).size(12));
        }
        widgets::page(body)
    }
}

/// A numbered list of names with their play counts.
fn ranked<'a>(title: &'a str, rows: impl Iterator<Item = (String, i64)>) -> Element<'a, App> {
    let mut list = Column::new().spacing(2);
    for (index, (name, count)) in rows.enumerate() {
        list = list.push(
            container(
                row![
                    container(widgets::secondary((index + 1).to_string()).size(13)).width(26),
                    text(name)
                        .size(14)
                        .width(Fill)
                        .wrapping(text::Wrapping::None),
                    widgets::secondary(plays(count)).size(12),
                ]
                .spacing(10),
            )
            .padding([7, 8]),
        );
    }
    column![text(title).size(20).font(widgets::BOLD), list]
        .spacing(8)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_store::DayPlays;

    fn recap() -> Recap {
        Recap {
            from: 0,
            to: i64::MAX,
            total_plays: 1_234,
            total_ms_played: 45_000_000,
            distinct_tracks: 310,
            distinct_artists: 42,
            distinct_albums: 61,
            top_tracks: Vec::new(),
            top_artists: Vec::new(),
            top_albums: Vec::new(),
            busiest_day: Some(DayPlays {
                day: "2026-10-21".into(),
                play_count: 12,
            }),
            unattributed_plays: 4,
        }
    }

    #[test]
    fn the_tiles_read_as_numbers_people_say() {
        let recap = recap();
        assert_eq!(thousands(recap.total_plays), "1,234");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(hours(&recap), "12.5");
        let whole = Recap {
            total_ms_played: 7_200_000,
            ..recap.clone()
        };
        assert_eq!(hours(&whole), "2");
    }

    #[test]
    fn the_busiest_day_and_the_unattributed_plays_say_what_they_are() {
        let recap = recap();
        assert_eq!(
            busiest(&recap).as_deref(),
            Some("Busiest day: 21 Oct 2026, 12 plays.")
        );
        assert!(unattributed(&recap).unwrap().starts_with("4 plays were"));
        let one = Recap {
            unattributed_plays: 1,
            busiest_day: None,
            ..recap
        };
        assert!(unattributed(&one).unwrap().starts_with("1 play was"));
        assert_eq!(busiest(&one), None);
        assert_eq!(plays(1), "1 play");
    }
}
