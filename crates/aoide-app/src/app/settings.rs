//! The Settings page's messages that reach past the page: the config, the
//! player's settings and the store's hidden songs.

use iced::Task;

use super::{App, Message};
use crate::screens::{self, Page};

impl App {
    pub(super) fn settings_message(
        &mut self,
        key: u64,
        message: screens::settings::Message,
    ) -> Task<Message> {
        use screens::settings::Message as S;
        let task = match &message {
            S::Toggle(which, on) => {
                which.apply(&mut self.config.settings, *on);
                self.settings_changed(*which)
            }
            S::LyricOffset(ms) => {
                self.config.settings.lyric_offset_ms =
                    crate::config::Settings::clamped_lyric_offset(*ms);
                self.save_config();
                Task::none()
            }
            S::Unhide(id) => return self.unhide(key, id.clone()),
            // Another pane starts at its top.
            S::Category(_) => iced::widget::operation::snap_to(
                crate::widgets::PAGE,
                iced::widget::scrollable::RelativeOffset::START,
            ),
            _ => Task::none(),
        };
        if let Some(Page::Settings(state)) = self
            .session
            .as_mut()
            .and_then(|s| s.nav.as_mut())
            .and_then(|nav| nav.iter_mut().find(|e| e.key == key))
            .map(|e| &mut e.page)
        {
            let _ = state.update(message);
        }
        task
    }

    /// A hidden song shown again: both flags cleared, then the list reloaded.
    fn unhide(&mut self, key: u64, id: String) -> Task<Message> {
        let store = self.store.clone();
        let now = aoide_store::now_millis();
        let reload = screens::settings::load_hidden(&store, key);
        Task::perform(
            async move {
                store
                    .call(move |w| {
                        let content_key = w
                            .store
                            .flags(&id)
                            .ok()
                            .flatten()
                            .map(|f| f.content_key)
                            .unwrap_or_default();
                        let _ = w.store.set_not_interested(false, &id, &content_key, now);
                        let _ = w.store.set_dont_count(false, &id, &content_key, now);
                    })
                    .await;
            },
            |()| Message::Ignore,
        )
        .chain(reload)
    }

    /// A setting changed: saved, and handed to whatever it governs.
    pub(super) fn settings_changed(&mut self, which: screens::settings::Toggle) -> Task<Message> {
        self.save_config();
        let _ = self.playback.set_settings(self.player_settings());
        match which {
            screens::settings::Toggle::Infinity | screens::settings::Toggle::InfinityAlbums => {
                self.infinity_switched()
            }
            screens::settings::Toggle::LevelVolume => {
                let switched = match self.session.as_ref() {
                    Some(signed) => self
                        .playback
                        .switch_level_volume(&self.config.settings, &signed.measures),
                    None => self
                        .playback
                        .set_level_volume(self.config.settings.level_volume),
                };
                self.result(switched)
            }
            _ => Task::none(),
        }
    }
}
