//! Motion covers: an album's looping video, played muted over its still
//! cover on the album's page and in the full-screen player (never the player
//! bar), as the iPhone app plays them.
//!
//! One video at a time: the full player's album while it is open,
//! else the album page on top. After every message the app checks which album
//! that is (`reconcile`); a new one is looked for in the cache (`cache.rs`)
//! and opened off the UI thread (`video.rs`), and the one before is ended.
//! Whatever fails, the still cover stays.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use aoide_melete::MotionCoverChoice;
use iced::widget::image;
use iced::{Element, Length, Subscription, Task};

use crate::app::{App, Message};
use crate::screens::Page;

mod cache;
mod settings;
mod video;

pub use settings::section as settings_section;
use video::Loop;

/// What the app holds for motion covers.
#[derive(Debug, Default)]
pub struct Motion {
    dir: Option<PathBuf>,
    /// The album wanted on screen, and its video once it plays.
    slot: Option<(String, Option<Loop>)>,
    /// Albums with nothing this computer plays, found this run.
    none: HashSet<String>,
    /// Bumped whenever what was found is forgotten, so a look started
    /// before then is dropped when it ends.
    generation: u64,
    /// What the server says, once asked (Settings).
    pub choice: Option<MotionCoverChoice>,
    /// Why the last change to the choice failed.
    pub problem: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Settings opened: ask the server about its motion covers.
    AskChoice,
    Choice(Option<MotionCoverChoice>),
    /// This computer's switch.
    Enabled(bool),
    /// "Ones your server made", for this user on every device.
    ShowGenerated(bool),
    ShowGeneratedSet(Result<MotionCoverChoice, String>),
    /// The videos on disk are gone: forget what was found.
    Forget,
    /// What looking for an album's video came to: `None` when it has none
    /// this app plays, `Some(Err)` when it can't be had now.
    Opened {
        album: String,
        generation: u64,
        video: Option<Result<Loop, ()>>,
    },
    /// The playing video failed: the still cover for this album from now on.
    Broke(String),
    /// A frame of the fade.
    Fade,
}

impl Motion {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            ..Self::default()
        }
    }

    /// Forgets every video found or playing: signed out, the cache cleared,
    /// or a change to which the server shows.
    pub fn forget(&mut self) {
        self.slot = None;
        self.none.clear();
        self.generation += 1;
    }

    /// `album`'s video while it plays.
    fn playing(&self, album: &str) -> Option<&Loop> {
        match &self.slot {
            Some((wanted, video)) if wanted == album => video.as_ref(),
            _ => None,
        }
    }
}

/// `still` (a cover `size` square with `radius` corners) with `album`'s
/// motion cover over it, while it plays.
pub fn over<'a>(
    motion: &'a Motion,
    album: Option<&str>,
    still: Element<'a, Message>,
    cover: Option<&image::Handle>,
    size: impl Into<Length> + Copy,
    radius: f32,
) -> Element<'a, Message> {
    match album.and_then(|album| Some((album, motion.playing(album)?))) {
        Some((album, video)) => {
            let broke = Message::Motion(Msg::Broke(album.to_owned()));
            video.over(still, cover, size, radius, broke)
        }
        None => still,
    }
}

impl App {
    pub(crate) fn motion(&mut self, message: Msg) -> Task<Message> {
        match message {
            Msg::AskChoice => self.ask_motion_choice(),
            Msg::Choice(choice) => {
                self.motion.choice = choice;
                Task::none()
            }
            Msg::Enabled(on) => {
                self.config.interface.motion_covers = on;
                self.save_config();
                Task::none()
            }
            Msg::ShowGenerated(shows) => self.set_show_generated(shows),
            Msg::ShowGeneratedSet(Ok(choice)) => {
                self.motion.choice = Some(choice);
                // Forgotten once the files are gone, so none is played again.
                let dir = self.motion.dir.clone();
                Task::future(async move {
                    if let Some(dir) = dir {
                        let _ = tokio::task::spawn_blocking(move || {
                            crate::maintenance::clear_dir(&dir)
                        })
                        .await;
                    }
                    Message::Motion(Msg::Forget)
                })
            }
            Msg::Forget => {
                self.motion.forget();
                Task::none()
            }
            Msg::ShowGeneratedSet(Err(problem)) => {
                self.motion.problem = Some(problem);
                Task::none()
            }
            Msg::Opened {
                album,
                generation,
                video,
            } => {
                self.opened(album, generation, video);
                Task::none()
            }
            Msg::Broke(album) => {
                log::warn!("A motion cover stopped playing; the still cover stays");
                self.motion.none.insert(album);
                self.motion.slot = None;
                Task::none()
            }
            Msg::Fade => Task::none(),
        }
    }

    fn ask_motion_choice(&mut self) -> Task<Message> {
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        // A server older than motion covers answers 404: no switch for it.
        Task::perform(
            async move { client.motion_cover_choice().await.ok() },
            |choice| Message::Motion(Msg::Choice(choice)),
        )
    }

    fn set_show_generated(&mut self, shows: bool) -> Task<Message> {
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        self.motion.problem = None;
        let client = signed.connection.client.clone();
        Task::perform(
            async move {
                client
                    .set_show_generated_motion_covers(shows)
                    .await
                    .map_err(|e| crate::failure::describe(&e).summary)
            },
            |result| Message::Motion(Msg::ShowGeneratedSet(result)),
        )
    }

    fn opened(&mut self, album: String, generation: u64, video: Option<Result<Loop, ()>>) {
        if generation != self.motion.generation {
            return;
        }
        let Some((wanted, playing)) = self.motion.slot.as_mut() else {
            return;
        };
        if *wanted != album {
            return;
        }
        match video {
            Some(Ok(mut video)) => {
                video.started = Instant::now();
                *playing = Some(video);
            }
            Some(Err(())) => {}
            None => {
                self.motion.none.insert(album);
            }
        }
    }

    /// The album whose motion cover belongs on screen now, and the path its
    /// album gave when the page has it.
    fn wanted_motion(&self) -> Option<(String, Option<String>)> {
        if !self.config.interface.motion_covers {
            return None;
        }
        let signed = self.session.as_ref()?;
        if self.listening.full.is_some() {
            let album = self.playback.current()?.1.meta.album_id.clone()?;
            return Some((album, None));
        }
        match &signed.nav.as_ref()?.current().page {
            Page::Album(state) => {
                let path = state.motion_cover()??;
                Some((state.id.clone(), Some(path.to_owned())))
            }
            _ => None,
        }
    }

    /// Starts the video that belongs on screen and ends any other.
    pub(crate) fn reconcile_motion(&mut self) -> Task<Message> {
        let wanted = self
            .wanted_motion()
            .filter(|(album, _)| !self.motion.none.contains(album));
        let Some((album, path)) = wanted else {
            self.motion.slot = None;
            return Task::none();
        };
        if self.motion.slot.as_ref().is_some_and(|(a, _)| *a == album) {
            return Task::none();
        }
        self.motion.slot = Some((album.clone(), None));
        let (Some(dir), Some(signed)) = (self.motion.dir.clone(), self.session.as_ref()) else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        let generation = self.motion.generation;
        Task::perform(open(client, dir, album.clone(), path), move |video| {
            Message::Motion(Msg::Opened {
                album: album.clone(),
                generation,
                video,
            })
        })
    }

    /// Frames while a video fades in; nothing otherwise.
    pub(crate) fn motion_subscription(&self) -> Subscription<Message> {
        match &self.motion.slot {
            Some((_, Some(video))) if video.fading(Instant::now()) => {
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(|_| Message::Motion(Msg::Fade))
            }
            _ => Subscription::none(),
        }
    }
}

/// Finds the album's video and opens it, off the UI thread.
async fn open(
    client: aoide_melete::MeleteClient,
    dir: PathBuf,
    album: String,
    path: Option<String>,
) -> Option<Result<Loop, ()>> {
    let file = match cache::find(client, dir, album, path).await {
        cache::Found::File(file) => file,
        cache::Found::Nothing => return None,
        cache::Found::Failed => return Some(Err(())),
    };
    let opened = tokio::task::spawn_blocking(move || Loop::open(&file)).await;
    match opened {
        Ok(Ok(video)) => Some(Ok(video)),
        // No decoder for it, or GStreamer can't start: this computer plays
        // none of it, so it isn't tried again this run.
        Ok(Err(reason)) => {
            log::warn!("A motion cover can't play here ({reason}); the still cover stays");
            None
        }
        Err(_) => Some(Err(())),
    }
}
