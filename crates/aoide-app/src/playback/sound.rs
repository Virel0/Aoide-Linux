//! What the player does to the sound, past choosing what plays: Level volume
//! over both its sources, the equaliser and compressor, the output device,
//! and the visualizer's tap.
//!
//! A child of `playback`, so it reaches the player and the output directly.

use std::collections::HashSet;

use super::Playback;
use super::tracks::{self, Measures};
use crate::config::Settings;

impl Playback {
    /// The Level volume switch, handed to the player (which weighs a file's
    /// ReplayGain tags against the sidecar's measurement, as
    /// `aoide_audio::loudness::level_gain` does), and the queued tracks'
    /// measured gains brought in line with it: so switching it on mid-queue
    /// levels the tracks already queued, not only the ones queued later.
    pub fn switch_level_volume(
        &mut self,
        settings: &Settings,
        measures: &Measures,
    ) -> Result<(), String> {
        self.set_level_volume(settings.level_volume)?;
        let ids: HashSet<String> = self.queued_ids().into_iter().collect();
        self.apply_measurements(&ids, |id| tracks::applied(id, measures, settings))
    }

    /// The switch alone: at launch, before anything is queued.
    pub fn set_level_volume(&mut self, on: bool) -> Result<(), String> {
        match self.player.as_mut() {
            Some(player) => player.set_level_volume(on).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aoide_audio::{EngineConfig, SoundBounds};
    use aoide_sidecar::AudioAnalysis;
    use aoide_store::PlaySource;

    use super::*;
    use crate::playback::Enqueue;
    use crate::playback::tracks::{BuiltTrack, TrackMeta};

    fn built(id: &str, measures: &Measures, settings: &Settings) -> BuiltTrack {
        let url =
            aoide_jellyfin::Url::parse(&format!("https://music.example.net/Audio/{id}")).unwrap();
        let (sound, gain_db) = tracks::applied(id, measures, settings);
        BuiltTrack {
            meta: TrackMeta {
                id: id.into(),
                title: id.into(),
                artist: String::new(),
                album_artist: None,
                album: String::new(),
                album_id: None,
                artist_id: None,
                artwork: None,
                duration_ms: None,
                track_number: None,
                disc_number: None,
                year: None,
                genres: Vec::new(),
                favourite: false,
            },
            track: aoide_audio::PlayerTrack {
                key: id.into(),
                source: aoide_audio::MediaSource::http(url.as_str()),
                mix: aoide_audio::MixTrack {
                    sound,
                    ..aoide_audio::MixTrack::default()
                },
                gain_db,
            },
            url,
        }
    }

    fn gains(playback: &Playback) -> Vec<Option<f64>> {
        playback
            .queue()
            .unwrap()
            .entries()
            .iter()
            .map(|e| e.track.gain_db)
            .collect()
    }

    #[test]
    fn switching_level_volume_relevels_what_is_already_queued() {
        let (_engine, handle, _events) = aoide_audio::Engine::new(EngineConfig {
            sample_rate: 48_000,
            channels: 2,
            decode: aoide_audio::DecodeMode::Inline,
        });
        let mut playback = Playback::on_engine(handle);
        let mut measures = Measures::default();
        let loud = AudioAnalysis {
            loudness_lufs: Some(-12.0),
            true_peak_dbfs: Some(-1.0),
            bpm: None,
            bpm_confidence: None,
            bpm_stability: None,
        };
        measures.insert("a", Some(SoundBounds::new(0, 9_000)), Some(loud));
        let mut settings = Settings {
            level_volume: false,
            ..Settings::default()
        };
        playback.set_level_volume(false).unwrap();
        playback
            .enqueue(
                vec![
                    built("a", &measures, &settings),
                    built("b", &measures, &settings),
                ],
                Enqueue::Replace {
                    start: 0,
                    shuffle: false,
                },
                PlaySource::Album,
                &HashMap::new(),
            )
            .unwrap();
        assert_eq!(gains(&playback), [None, None]);

        settings.level_volume = true;
        playback.switch_level_volume(&settings, &measures).unwrap();
        // −12 LUFS to −18: six decibels down; the unmeasured track as it is.
        assert_eq!(gains(&playback), [Some(-6.0), None]);

        settings.level_volume = false;
        playback.switch_level_volume(&settings, &measures).unwrap();
        assert_eq!(gains(&playback), [None, None]);
    }

    #[test]
    fn without_a_device_the_switch_is_kept_quiet() {
        let mut playback = Playback::without_output("no output device".into(), 1.0);
        assert_eq!(playback.set_level_volume(true), Ok(()));
        assert_eq!(
            playback.switch_level_volume(&Settings::default(), &Measures::default()),
            Ok(())
        );
    }
}
