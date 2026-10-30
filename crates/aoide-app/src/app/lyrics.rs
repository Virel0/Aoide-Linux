//! The lyrics panel: fetching the playing song's lyrics, and keeping the
//! lit line in view.

use aoide_melete::Lyrics;
use iced::Task;

use super::{App, Message, Panel};
use crate::lyrics_panel::LyricsState;
use crate::screens::Load;

impl App {
    pub(super) fn lyrics_arrived(
        &mut self,
        item: String,
        result: Result<Option<Lyrics>, crate::failure::Failure>,
    ) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.lyrics = Some(LyricsState {
                item,
                lyrics: Load::from(result),
            });
            signed.lyric_line = None;
        }
        self.follow_lyrics()
    }

    pub(super) fn retry_lyrics(&mut self) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.lyrics = None;
        }
        self.track_changed()
    }

    /// What is playing changed, or the lyrics panel opened: fetch the lyrics
    /// if they are showing and not already here.
    pub(crate) fn track_changed(&mut self) -> Task<Message> {
        let current = self.playback.current().map(|(_, q)| q.meta.id.clone());
        let meta = self.playback.current().map(|(_, q)| q.meta.clone());
        let online = self.lyrics_online();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let (Some(Panel::Lyrics), Some(item)) = (signed.panel, current) else {
            return Task::none();
        };
        if signed.lyrics.as_ref().is_some_and(|l| l.item == item) {
            return self.follow_lyrics();
        }
        signed.lyrics = Some(LyricsState {
            item: item.clone(),
            lyrics: Load::Loading,
        });
        signed.lyric_line = None;
        let client = signed.connection.client.clone();
        let Some(meta) = meta else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result =
                    crate::lyrics_online::find(client, online, meta, aoide_store::now_millis())
                        .await;
                (item, result)
            },
            |(item, result)| match result {
                Err(aoide_melete::Error::Unauthorized) => Message::TokenRejected,
                result => Message::Listening(crate::listening::Msg::LyricsFound(
                    item,
                    result.map_err(|e| crate::failure::describe(&e)),
                )),
            },
        )
    }

    /// Keeps the lit lyric line in view as the song plays.
    pub(crate) fn follow_lyrics(&mut self) -> Task<Message> {
        let position = self.playback.position_ms();
        let offset = self.config.settings.lyric_offset_ms;
        let current = self.playback.current().map(|(_, q)| q.meta.id.clone());
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if signed.panel != Some(Panel::Lyrics) {
            return Task::none();
        }
        let Some(LyricsState {
            item,
            lyrics: Load::Ready(Some(lyrics)),
        }) = &signed.lyrics
        else {
            return Task::none();
        };
        if current.as_deref() != Some(item.as_str()) {
            return Task::none();
        }
        let line = crate::playback::lyrics::current_line(lyrics, position, offset);
        if line == signed.lyric_line {
            return Task::none();
        }
        signed.lyric_line = line;
        let Some(line) = line else {
            return Task::none();
        };
        // Lines wrap, so the place is a share of the whole rather than a
        // measured offset; held a little above centre.
        let share = (line as f32 / lyrics.lines.len().max(1) as f32 - 0.1).clamp(0.0, 1.0);
        iced::widget::operation::snap_to(
            crate::lyrics_panel::SCROLL,
            iced::widget::scrollable::RelativeOffset { x: 0.0, y: share },
        )
    }
}
