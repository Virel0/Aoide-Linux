//! Playback wired to everything around it: Melete's measurements before
//! tracks are queued, the store's play events, the saved queue, and
//! Infinity.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use aoide_audio::EngineEvent;
use aoide_sidecar::{AudioAnalysisResponse, SidecarClient, SidecarError, SoundBoundsResponse};
use aoide_store::PlaySource;
use iced::Task;

use crate::actions::PlayRequest;
use crate::app::{App, Message};
use crate::playback::Enqueue;
use crate::playback::tracks::{self, TrackMeta};

mod report;
mod saved;
mod top_up;

pub use report::leave_server;

/// How often the saved queue follows the playhead while a track plays.
const SAVE_QUEUE_EVERY_MS: u64 = 10_000;

/// How long queueing waits for Melete's measurements before playing
/// without them.
pub const MEASURE_WAIT: Duration = Duration::from_secs(2);
/// How many tracks are measured ahead of being queued: one server call.
/// The rest of the queue is measured afterwards in batches of this size.
pub const MEASURE_AHEAD: usize = aoide_sidecar::MAX_IDS_PER_CALL;
/// How long one of those later batches waits: nothing is held up by it.
pub const BATCH_WAIT: Duration = Duration::from_secs(30);

/// Tracks on their way into the queue.
#[derive(Debug, Clone)]
pub struct Pending {
    pub tracks: Vec<TrackMeta>,
    pub how: Destination,
    /// For a replacement: dropped if a newer one was asked for meanwhile.
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub enum Destination {
    Play {
        how: Enqueue,
        source: PlaySource,
        reasons: HashMap<String, String>,
    },
    /// The queue saved at the last quit: loaded held at the saved place.
    Restore { position: usize, elapsed_ms: u64 },
    /// A listening-together session's queue, at its place, playing as it is.
    Together {
        position: usize,
        elapsed_ms: u64,
        playing: bool,
    },
}

/// The server's answers for one batch.
#[derive(Debug, Clone, Default)]
pub struct Measured {
    pub ids: Vec<String>,
    pub bounds: Option<SoundBoundsResponse>,
    pub analysis: Option<AudioAnalysisResponse>,
    /// The server has neither endpoint: stop asking this session.
    pub absent: bool,
}

/// Asks for sound bounds and loudness together, giving up after a short wait.
pub async fn measure(sidecar: SidecarClient, ids: Vec<String>) -> Measured {
    measure_within(sidecar, ids, MEASURE_WAIT).await
}

/// The same, waiting at most `wait`.
pub async fn measure_within(sidecar: SidecarClient, ids: Vec<String>, wait: Duration) -> Measured {
    if ids.is_empty() {
        return Measured::default();
    }
    let asked = async { tokio::join!(sidecar.sound_bounds(&ids), sidecar.audio_analysis(&ids)) };
    match tokio::time::timeout(wait, asked).await {
        Ok((bounds, analysis)) => {
            let absent = matches!(bounds, Err(SidecarError::Absent { .. }))
                && matches!(analysis, Err(SidecarError::Absent { .. }));
            Measured {
                ids,
                bounds: bounds.ok(),
                analysis: analysis.ok(),
                absent,
            }
        }
        Err(_) => Measured {
            ids,
            ..Measured::default()
        },
    }
}

/// The next tracks of the queue to ask Melete about: not yet measured,
/// not asked about already this session (a track still being analysed is
/// asked again only when it is queued again), at most one call's worth.
pub fn next_batch(
    measures: &tracks::Measures,
    asked: &HashSet<String>,
    queued: Vec<String>,
) -> Vec<String> {
    measures
        .unknown(queued.iter().map(String::as_str))
        .into_iter()
        .filter(|id| !asked.contains(id))
        .take(MEASURE_AHEAD)
        .collect()
}

impl App {
    /// Queues tracks: measured first where the server can say, then built
    /// and handed to the player.
    pub(crate) fn play(&mut self, request: PlayRequest) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.menu = None;
        if matches!(request.how, Enqueue::Replace { .. }) {
            signed.play_generation += 1;
        }
        let pending = Pending {
            tracks: request.tracks,
            how: Destination::Play {
                how: request.how,
                source: request.source,
                reasons: HashMap::new(),
            },
            generation: signed.play_generation,
        };
        self.measure_then_queue(pending)
    }

    pub(crate) fn measure_then_queue(&mut self, pending: Pending) -> Task<Message> {
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        let ids = signed.measures.unknown(
            pending
                .tracks
                .iter()
                .take(MEASURE_AHEAD)
                .map(|t| t.id.as_str()),
        );
        match (&signed.sidecar, signed.measures_available) {
            (Some(sidecar), true) if !ids.is_empty() => {
                let sidecar = sidecar.clone();
                Task::perform(measure(sidecar, ids), move |measured| {
                    Message::Measured(Box::new((pending.clone(), measured)))
                })
            }
            _ => self.queue(pending),
        }
    }

    pub(crate) fn measured(&mut self, pending: Pending, measured: Measured) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            if measured.absent {
                signed.measures_available = false;
            }
            signed.measures.absorb(
                &measured.ids,
                measured.bounds.as_ref(),
                measured.analysis.as_ref(),
            );
        }
        self.queue(pending)
    }

    /// Builds and queues, now that the measurements are in.
    fn queue(&mut self, pending: Pending) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let replaces = matches!(
            pending.how,
            Destination::Restore { .. }
                | Destination::Together { .. }
                | Destination::Play {
                    how: Enqueue::Replace { .. },
                    ..
                }
        );
        if replaces && pending.generation != signed.play_generation {
            if matches!(pending.how, Destination::Together { .. }) {
                signed.together.applying_ended();
            }
            return Task::none();
        }
        let streams = signed.connection.streams();
        let format = crate::sound::stream_format(&self.config.sound.transcode);
        let built: Vec<_> = pending
            .tracks
            .iter()
            .map(|meta| {
                tracks::build(
                    &streams,
                    meta,
                    &signed.measures,
                    &self.config.settings,
                    format,
                )
            })
            .collect();
        let result = match pending.how {
            Destination::Play {
                how,
                source,
                reasons,
            } => {
                if replaces {
                    signed.infinity.reset();
                }
                self.playback.enqueue(built, how, source, &reasons)
            }
            Destination::Restore {
                position,
                elapsed_ms,
            } => self.playback.restore(built, position, elapsed_ms),
            Destination::Together {
                position,
                elapsed_ms,
                playing,
            } => {
                signed.infinity.reset();
                signed.together.applying_ended();
                self.playback
                    .restore(built, position, elapsed_ms)
                    .and_then(|()| {
                        if playing {
                            self.playback.play()
                        } else {
                            Ok(())
                        }
                    })
            }
        };
        if let Err(error) = result {
            self.notice = Some(error);
        }
        Task::batch([
            self.check_infinity(),
            self.save_queue(),
            self.measure_queue(),
        ])
    }

    /// Measures the rest of the queue as it grows, one batch at a time, and
    /// hands the answers to the queued tracks: so tracks past the first
    /// [`MEASURE_AHEAD`] are measured too.
    pub(crate) fn measure_queue(&mut self) -> Task<Message> {
        let queued = self.playback.queued_ids();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if signed.measuring || !signed.measures_available {
            return Task::none();
        }
        let Some(sidecar) = signed.sidecar.clone() else {
            return Task::none();
        };
        let batch = next_batch(&signed.measures, &signed.measure_asked, queued);
        if batch.is_empty() {
            return Task::none();
        }
        signed.measure_asked.extend(batch.iter().cloned());
        signed.measuring = true;
        Task::perform(measure_within(sidecar, batch, BATCH_WAIT), |measured| {
            Message::QueueMeasured(Box::new(measured))
        })
    }

    pub(crate) fn queue_measured(&mut self, measured: Measured) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.measuring = false;
        if measured.absent {
            signed.measures_available = false;
        }
        signed.measures.absorb(
            &measured.ids,
            measured.bounds.as_ref(),
            measured.analysis.as_ref(),
        );
        let ids: HashSet<String> = measured.ids.into_iter().collect();
        let measures = &signed.measures;
        let settings = &self.config.settings;
        let applied = self
            .playback
            .apply_measurements(&ids, |id| tracks::applied(id, measures, settings));
        if let Err(error) = applied {
            self.notice = Some(error);
        }
        self.measure_queue()
    }

    pub(crate) fn engine_event(&mut self, event: EngineEvent) -> Task<Message> {
        if let Err(error) = self.playback.handle(&event) {
            self.notice = Some(error);
        }
        let now = self.now();
        let task = match &event {
            EngineEvent::TrackStarted { id } => self.track_started(*id, now),
            EngineEvent::TrackEnded { id } => {
                let entry = *id;
                self.store.send(move |w| w.recorder.ended(entry));
                Task::none()
            }
            EngineEvent::Position { id, ms } => self.position(*id, *ms, now),
            EngineEvent::Finished => {
                let at = aoide_store::now_millis();
                self.store.send(move |w| {
                    let _ = w.recorder.stopped(&w.store, at);
                });
                self.save_queue()
            }
            EngineEvent::Error { id, message } => {
                let title = self.playback.queued(*id).map_or_else(
                    || "A track".to_owned(),
                    |q| format!("\u{201c}{}\u{201d}", q.meta.title),
                );
                self.notice = Some(format!("{title} couldn't be played: {message}"));
                Task::none()
            }
            EngineEvent::DeviceError { .. } | EngineEvent::DeviceRestored => Task::none(),
        };
        Task::batch([task, self.sleep_on_event(&event)])
    }

    /// A track started: its play event opened (unless listening privately),
    /// and what follows a new track.
    fn track_started(&mut self, entry: aoide_audio::EntryId, now: u64) -> Task<Message> {
        let Some(queued) = self.playback.queued(entry).cloned() else {
            return Task::none();
        };
        if let Some(signed) = self.session.as_mut() {
            signed.queue_saved_at = now;
        }
        let row = queued.meta.track_ref(aoide_store::now_millis());
        let source = queued.source;
        let at = aoide_store::now_millis();
        let activity = aoide_store::Activity::parse(self.config.activity.as_deref());
        // Private listening opens no play events.
        if !self.listening.private {
            self.store.send(move |w| {
                let _ = w
                    .recorder
                    .started(&w.store, entry, &row, source, activity, at);
            });
        }
        Task::batch([
            self.check_infinity(),
            self.save_queue(),
            self.track_changed(),
        ])
    }

    /// Four times a second: the play event's progress, the lit lyric line,
    /// and every ten seconds the saved queue's place.
    fn position(&mut self, entry: aoide_audio::EntryId, ms: u64, now: u64) -> Task<Message> {
        self.store.send(move |w| w.recorder.progress(entry, ms));
        let lyrics = self.follow_lyrics();
        let due = self.session.as_mut().is_some_and(|signed| {
            let due = now.saturating_sub(signed.queue_saved_at) >= SAVE_QUEUE_EVERY_MS;
            if due {
                signed.queue_saved_at = now;
            }
            due
        });
        if !due {
            return lyrics;
        }
        // The store skips a save that moved less than its own threshold.
        Task::batch([lyrics, self.save_queue()])
    }
}

#[cfg(test)]
mod tests {
    use aoide_melete::MeleteClient;

    use super::saved::read_saved;
    use super::*;
    use aoide_melete::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn measurements_arrive_together_and_a_missing_sidecar_is_named() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/aoide/sound-bounds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "bounds": {"t1": {"soundStartMs": 100, "soundEndMs": 5000}}, "pending": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/aoide/audio-analysis"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "analysis": {"t1": {"loudnessLufs": -10.0, "truePeakDbfs": -1.0}}, "pending": []
            })))
            .mount(&server)
            .await;
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let measured = measure(sidecar, vec!["t1".into()]).await;
        assert!(!measured.absent);
        let mut measures = tracks::Measures::default();
        measures.absorb(
            &measured.ids,
            measured.bounds.as_ref(),
            measured.analysis.as_ref(),
        );
        assert!(measures.bounds("t1").is_some());
        assert!(measures.analysis("t1").is_some());

        let bare = MockServer::start().await;
        let sidecar = SidecarClient::new(&bare.uri(), "tok").unwrap();
        assert!(measure(sidecar, vec!["t1".into()]).await.absent);
    }

    #[test]
    fn the_queue_is_measured_a_call_at_a_time_and_never_twice() {
        let queued: Vec<String> = (0..450).map(|n| format!("t{n}")).collect();
        let mut measures = tracks::Measures::default();
        let mut asked = HashSet::new();

        let first = next_batch(&measures, &asked, queued.clone());
        assert_eq!(first.len(), MEASURE_AHEAD);
        assert_eq!(first[0], "t0");
        asked.extend(first.iter().cloned());
        // Measured before it was queued: never asked about.
        measures.insert("t205", None, None);

        let second = next_batch(&measures, &asked, queued.clone());
        assert_eq!(second.len(), MEASURE_AHEAD);
        assert_eq!(second[0], "t200");
        assert!(!second.contains(&"t205".to_owned()));
        asked.extend(second.iter().cloned());

        let third = next_batch(&measures, &asked, queued.clone());
        assert_eq!(third.first().map(String::as_str), Some("t401"));
        assert_eq!(third.len(), 49);
        asked.extend(third.iter().cloned());
        assert!(next_batch(&measures, &asked, queued).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_slow_sidecar_is_not_waited_for() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(20)))
            .mount(&server)
            .await;
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let started = std::time::Instant::now();
        let measured = measure(sidecar, vec!["t1".into()]).await;
        assert!(started.elapsed() < MEASURE_WAIT + Duration::from_secs(1));
        assert!(measured.bounds.is_none() && !measured.absent);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_saved_queue_follows_its_track_when_others_have_gone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("id", "1,2,3"))
            // 1 was deleted from the server since the queue was saved.
            .respond_with(
                ResponseTemplate::new(200).set_body_json(crate::test_json::page(vec![
                    crate::test_json::track(2, "B"),
                    crate::test_json::track(3, "C"),
                ])),
            )
            .mount(&server)
            .await;
        let store = crate::store_worker::StoreHandle::in_memory("device-1").unwrap();
        store
            .call(|w| {
                let ids: Vec<String> = ["1", "2", "3"].map(String::from).to_vec();
                w.store.save_queue("Desk", &ids, 2, 42_000, 1).unwrap();
            })
            .await
            .unwrap();
        let saved = read_saved(store, client(&server)).await.unwrap().unwrap();
        assert_eq!(saved.items.len(), 2);
        assert_eq!(saved.items[saved.position].id, "3");
        assert_eq!(saved.elapsed_ms, 42_000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_empty_saved_queue_restores_nothing() {
        let server = MockServer::start().await;
        let store = crate::store_worker::StoreHandle::in_memory("device-1").unwrap();
        assert!(
            read_saved(store.clone(), client(&server))
                .await
                .unwrap()
                .is_none()
        );
        store
            .call(|w| {
                w.store
                    .save_queue("Desk", &["a".to_owned()], 0, 0, 1)
                    .unwrap();
                w.store.clear_queue(2).unwrap();
            })
            .await
            .unwrap();
        assert!(read_saved(store, client(&server)).await.unwrap().is_none());
    }
}
