//! Playback wired to everything around it: the sidecar's measurements before
//! tracks are queued, the Jellyfin reports, the store's play events, the
//! saved queue, and Infinity.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use aoide_audio::{EngineEvent, RepeatMode};
use aoide_jellyfin::{Item, JellyfinClient};
use aoide_sidecar::{AudioAnalysisResponse, SidecarClient, SidecarError, SoundBoundsResponse};
use aoide_store::PlaySource;
use iced::Task;

use crate::actions::PlayRequest;
use crate::app::{App, Message};
use crate::playback::Enqueue;
use crate::playback::infinity::{self, TopUp};
use crate::playback::report::Report;
use crate::playback::tracks::{self, TrackMeta};
use crate::session;

/// How long queueing waits for the sidecar's measurements before playing
/// without them.
pub const MEASURE_WAIT: Duration = Duration::from_secs(2);
/// How many tracks are measured ahead of being queued: one sidecar call.
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
}

/// The sidecar's answers for one batch.
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

/// The next tracks of the queue to ask the sidecar about: not yet measured,
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

/// The saved queue, read back: ids in order, where it was, how far in.
#[derive(Debug, Clone)]
pub struct Saved {
    pub items: Vec<Item>,
    pub position: usize,
    pub elapsed_ms: u64,
}

/// Reads the saved queue and fetches its tracks. The saved position names a
/// track; if some tracks have gone from the server since, the position follows
/// that track rather than its old index.
pub async fn read_saved(
    store: crate::store_worker::StoreHandle,
    client: JellyfinClient,
) -> Result<Option<Saved>, aoide_jellyfin::Error> {
    let saved = store
        .call(|w| w.store.local_queue().ok().flatten())
        .await
        .flatten();
    let Some(saved) = saved else {
        return Ok(None);
    };
    let ids = saved.track_id_list();
    if ids.is_empty() {
        return Ok(None);
    }
    let position = usize::try_from(saved.position.max(0))
        .unwrap_or(0)
        .min(ids.len() - 1);
    let current = ids[position].clone();
    let items = client.items_by_ids(&ids).await?;
    if items.is_empty() {
        return Ok(None);
    }
    let position = items
        .iter()
        .position(|item| item.id == current)
        .unwrap_or(0);
    Ok(Some(Saved {
        items,
        position,
        elapsed_ms: u64::try_from(saved.elapsed_ms.max(0)).unwrap_or(0),
    }))
}

impl App {
    /// Queues tracks: measured first where the sidecar can say, then built
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
                | Destination::Play {
                    how: Enqueue::Replace { .. },
                    ..
                }
        );
        if replaces && pending.generation != signed.play_generation {
            return Task::none();
        }
        let built: Vec<_> = pending
            .tracks
            .iter()
            .filter_map(|meta| {
                tracks::build(
                    &signed.connection.client,
                    meta,
                    &signed.measures,
                    &self.config.settings,
                )
                .ok()
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

    /// At launch: the queue as it was left, held, not playing.
    pub(crate) fn restore_queue(&self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        if self.playback.queue().is_some_and(|q| !q.is_empty()) {
            return Task::none();
        }
        let store = self.store.clone();
        let client = signed.connection.client.clone();
        let generation = signed.play_generation;
        Task::perform(read_saved(store, client), move |result| match result {
            Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
            Ok(Some(saved)) => Message::Restore(Box::new(Pending {
                tracks: saved.items.iter().map(TrackMeta::from_item).collect(),
                how: Destination::Restore {
                    position: saved.position,
                    elapsed_ms: saved.elapsed_ms,
                },
                generation,
            })),
            _ => Message::Ignore,
        })
    }

    /// Writes the queue as it stands, when it has changed enough to matter.
    pub(crate) fn save_queue(&self) -> Task<Message> {
        if self.session.is_none() {
            return Task::none();
        }
        let (ids, position) = self.playback.saved_form();
        let elapsed = i64::try_from(self.playback.position_ms()).unwrap_or(0);
        let name = crate::app::device_name();
        let now = aoide_store::now_millis();
        self.store.send(move |w| {
            let _ = w
                .store
                .save_queue_if_changed(&name, &ids, position, elapsed, now);
        });
        Task::none()
    }

    pub(crate) fn engine_event(&mut self, event: EngineEvent) -> Task<Message> {
        if let Err(error) = self.playback.handle(&event) {
            self.notice = Some(error);
        }
        let now = self.now();
        let mut tasks = Vec::new();
        match &event {
            EngineEvent::TrackStarted { id } => {
                if let Some(queued) = self.playback.queued(*id).cloned() {
                    let reports = self
                        .session
                        .as_mut()
                        .map(|s| s.reporter.started(*id, &queued.meta.id, now))
                        .unwrap_or_default();
                    tasks.push(self.send_reports(reports));
                    let entry = *id;
                    let row = queued.meta.track_ref(aoide_store::now_millis());
                    let source = queued.source;
                    let at = aoide_store::now_millis();
                    self.store.send(move |w| {
                        let _ = w.recorder.started(&w.store, entry, &row, source, at);
                    });
                    tasks.push(self.check_infinity());
                    tasks.push(self.save_queue());
                    tasks.push(self.track_changed());
                }
            }
            EngineEvent::TrackEnded { id } => {
                let entry = *id;
                self.store.send(move |w| w.recorder.ended(entry));
            }
            EngineEvent::Position { id, ms } => {
                let (entry, ms) = (*id, *ms);
                self.store.send(move |w| w.recorder.progress(entry, ms));
                let report = self
                    .session
                    .as_mut()
                    .and_then(|s| s.reporter.position(entry, ms, now));
                tasks.push(self.follow_lyrics());
                if let Some(report) = report {
                    tasks.push(self.send_reports(vec![report]));
                    // Every ten seconds, with the report: the store skips a
                    // save that moved less than its own threshold.
                    tasks.push(self.save_queue());
                }
            }
            EngineEvent::Finished => {
                let report = self.session.as_mut().and_then(|s| s.reporter.stopped());
                tasks.push(self.send_reports(report.into_iter().collect()));
                let at = aoide_store::now_millis();
                self.store.send(move |w| {
                    let _ = w.recorder.stopped(&w.store, at);
                });
                tasks.push(self.save_queue());
            }
            EngineEvent::Error { id, message } => {
                let title = self.playback.queued(*id).map_or_else(
                    || "A track".to_owned(),
                    |q| format!("\u{201c}{}\u{201d}", q.meta.title),
                );
                self.notice = Some(format!("{title} couldn't be played: {message}"));
            }
            EngineEvent::DeviceError { .. } | EngineEvent::DeviceRestored => {}
        }
        Task::batch(tasks)
    }

    /// Pause and resume are reported straight away.
    pub(crate) fn report_pause_state(&mut self) -> Task<Message> {
        let paused = !self.playback.is_playing();
        let now = self.now();
        let report = self
            .session
            .as_mut()
            .and_then(|s| s.reporter.paused(paused, now));
        self.send_reports(report.into_iter().collect())
    }

    pub(crate) fn send_reports(&self, reports: Vec<Report>) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        Task::batch(reports.into_iter().map(|report| {
            let client = signed.connection.client.clone();
            Task::perform(send_report(client, report), |result| match result {
                Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
                _ => Message::Ignore,
            })
        }))
    }

    /// Tops the queue up if Infinity is on and it is running short.
    pub(crate) fn check_infinity(&mut self) -> Task<Message> {
        let enabled = self.config.settings.infinity;
        let repeat_off = self.playback.repeat() == RepeatMode::Off;
        let current = self
            .playback
            .current()
            .map(|(entry, q)| (entry, q.meta.id.clone()));
        let ahead = self.playback.ahead();
        let length = self.playback.queue().map_or(0, |q| q.entries().len());
        let known = self.playback.known_ids();
        let queue = self.playback.up_next_ids();
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if !signed.infinity.should_fire(
            enabled,
            repeat_off,
            current.as_ref().map(|c| c.0),
            ahead,
            length,
        ) {
            return Task::none();
        }
        let Some((_, seed)) = current else {
            return Task::none();
        };
        let ask = infinity::Ask {
            seed,
            queue,
            known,
            use_sidecar: signed.next_available,
        };
        let client = signed.connection.client.clone();
        let sidecar = signed.sidecar.clone();
        Task::perform(
            infinity::top_up(client, sidecar, store, ask, aoide_store::now_millis()),
            |found| Message::ToppedUp(Box::new(found)),
        )
    }

    pub(crate) fn topped_up(&mut self, found: TopUp) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.infinity.finished();
        if found.sidecar_absent {
            signed.next_available = false;
        }
        if found.items.is_empty() {
            return Task::none();
        }
        let pending = Pending {
            tracks: found.items.iter().map(TrackMeta::from_item).collect(),
            how: Destination::Play {
                how: Enqueue::Extend,
                source: PlaySource::Unknown,
                reasons: found.reasons,
            },
            generation: signed.play_generation,
        };
        self.measure_then_queue(pending)
    }

    /// Infinity was switched on: look at the queue now, not at the next track.
    pub(crate) fn infinity_switched(&mut self) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.infinity.reset();
        }
        self.check_infinity()
    }

    /// Stops reporting: signing out or quitting. Returns the report to send.
    pub(crate) fn final_report(&mut self) -> Option<Report> {
        self.session.as_mut().and_then(|s| s.reporter.stopped())
    }

    /// Hands what is playing to the desktop. The cover goes as the cached
    /// file, never as a server URL: that one carries the token.
    pub(crate) fn publish(&self) {
        let current = self.playback.current().map(|(_, q)| &q.meta);
        let art_url = current
            .and_then(|m| m.artwork.as_ref())
            .zip(self.artwork.dir())
            .and_then(|((id, tag), dir)| {
                crate::integration::art_url(
                    &dir.join(crate::artwork::ArtKey::new(id, tag).file_name()),
                )
            });
        crate::integration::publish(
            &self.desktop,
            &crate::integration::NowPlaying {
                track_id: current.map(|m| m.id.clone()),
                title: current.map(|m| m.title.clone()).unwrap_or_default(),
                artist: current.map(|m| m.artist.clone()).unwrap_or_default(),
                album_artist: current.and_then(|m| m.album_artist.clone()),
                album: current.map(|m| m.album.clone()).unwrap_or_default(),
                track_number: current
                    .and_then(|m| m.track_number)
                    .and_then(|n| u32::try_from(n).ok()),
                art_url,
                length_ms: current
                    .and_then(|m| m.duration_ms)
                    .map(|ms| ms.max(0) as u64),
                position_ms: self.playback.position_ms(),
                playing: self.playback.is_playing(),
                has_next: self.playback.ahead() > 0,
                shuffle: self.playback.is_shuffled(),
                repeat: self.playback.repeat(),
                volume: self.playback.volume(),
            },
        );
    }
}

/// One report to Jellyfin.
pub async fn send_report(
    client: JellyfinClient,
    report: Report,
) -> Result<(), aoide_jellyfin::Error> {
    let ms = |ms: u64| Duration::from_millis(ms);
    match report {
        Report::Started { item, position_ms } => {
            client.report_playback_started(&item, ms(position_ms)).await
        }
        Report::Progress {
            item,
            position_ms,
            paused,
        } => {
            client
                .report_playback_progress(&item, ms(position_ms), paused)
                .await
        }
        Report::Stopped { item, position_ms } => {
            client.report_playback_stopped(&item, ms(position_ms)).await
        }
    }
}

/// The last report and the sign-out together, inside the grace period: the
/// token has to be alive for both, and neither may hold up the screen.
pub async fn leave_server(client: JellyfinClient, report: Option<Report>) {
    let _ = tokio::time::timeout(session::SIGN_OUT_GRACE, async {
        if let Some(report) = report {
            let _ = send_report(client.clone(), report).await;
        }
        let _ = client.sign_out().await;
    })
    .await;
}

/// The window is closing: the last report gets a moment, no more.
pub async fn report_on_quit(client: JellyfinClient, report: Report) {
    let _ = tokio::time::timeout(Duration::from_millis(1_500), send_report(client, report)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> JellyfinClient {
        JellyfinClient::new(
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
    async fn reports_go_to_the_session_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing"))
            .and(body_partial_json(serde_json::json!({"ItemId": "t1"})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Progress"))
            .and(body_partial_json(
                serde_json::json!({"IsPaused": true, "PositionTicks": 100_000_000}),
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Stopped"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let client = client(&server);
        send_report(
            client.clone(),
            Report::Started {
                item: "t1".into(),
                position_ms: 0,
            },
        )
        .await
        .unwrap();
        send_report(
            client.clone(),
            Report::Progress {
                item: "t1".into(),
                position_ms: 10_000,
                paused: true,
            },
        )
        .await
        .unwrap();
        leave_server(
            client,
            Some(Report::Stopped {
                item: "t1".into(),
                position_ms: 12_000,
            }),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_saved_queue_follows_its_track_when_others_have_gone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("ids", "a,b,c"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                // "a" was deleted from the server since the queue was saved.
                "Items": [{"Id": "b", "Name": "B", "Type": "Audio"}, {"Id": "c", "Name": "C", "Type": "Audio"}],
                "TotalRecordCount": 2
            })))
            .mount(&server)
            .await;
        let store = crate::store_worker::StoreHandle::in_memory("device-1").unwrap();
        store
            .call(|w| {
                let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
                w.store.save_queue("Desk", &ids, 2, 42_000, 1).unwrap();
            })
            .await
            .unwrap();
        let saved = read_saved(store, client(&server)).await.unwrap().unwrap();
        assert_eq!(saved.items.len(), 2);
        assert_eq!(saved.items[saved.position].id, "c");
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
