//! Playing a motion cover: a GStreamer `playbin` with no sound, looped, drawn
//! by iced_video_player over the still cover, which shows through until the
//! first frame and fades out over it.
//!
//! Making the pipeline waits (up to 5 s) for the decoder to start, and ending
//! it waits for its thread, so both happen off the UI thread. Any failure,
//! from no decoder for the video to a file that won't play, leaves the still
//! cover; nothing here panics or says so on screen.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use iced::widget::{image, stack};
use iced::{ContentFit, Element, Length};
use iced_video_player::{Video, VideoPlayer};

use crate::app::Message;

/// How long the still cover takes to fade out over the video.
pub const FADE: Duration = Duration::from_millis(400);

/// A video playing in a loop. Cheap to clone; the last clone dropped ends
/// the pipeline on a thread of its own.
#[derive(Clone)]
pub struct Loop {
    video: Option<Arc<Video>>,
    /// When it was put on screen, for the fade.
    pub started: Instant,
}

impl std::fmt::Debug for Loop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Loop")
    }
}

impl Drop for Loop {
    fn drop(&mut self) {
        // iced_video_player stops the pipeline and joins its thread when the
        // video drops, and panics if GStreamer refuses: neither on the UI
        // thread. If no thread can be made, it ends here.
        if let Some(video) = self.video.take().and_then(Arc::into_inner) {
            let _ = std::thread::Builder::new()
                .name("motion-cover-end".into())
                .spawn(move || drop(video));
        }
    }
}

impl Loop {
    /// Plays `file` muted and looped, or says why it can't. Blocks: run it
    /// off the UI thread.
    ///
    /// # Errors
    ///
    /// The reason, for the log: GStreamer couldn't start, a plugin is
    /// missing (such as an H.264 decoder), or the file doesn't play.
    pub fn open(file: &Path) -> Result<Self, String> {
        let uri = reqwest::Url::from_file_path(file).map_err(|()| "not a file path".to_owned())?;
        let mut video = silent(&uri).map_err(|e| e.to_string())?;
        video.set_looping(true);
        Ok(Self {
            video: Some(Arc::new(video)),
            started: Instant::now(),
        })
    }

    /// How far the still cover has faded, 0 (all cover) to 1 (all video).
    pub fn shown(&self, now: Instant) -> f32 {
        (now.duration_since(self.started).as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Whether the fade is still going: a frame past its end, so the last
    /// view drawn has the cover gone.
    pub fn fading(&self, now: Instant) -> bool {
        now.duration_since(self.started) < FADE + Duration::from_millis(50)
    }

    /// The video over `still`, with `cover` fading out above it.
    pub fn over<'a>(
        &'a self,
        still: Element<'a, Message>,
        cover: Option<&image::Handle>,
        size: impl Into<Length> + Copy,
        radius: f32,
        broke: Message,
    ) -> Element<'a, Message> {
        let Some(video) = self.video.as_deref() else {
            return still;
        };
        let player = VideoPlayer::new(video)
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .on_error(move |_| broke.clone());
        let fading = 1.0 - self.shown(Instant::now());
        match cover.filter(|_| fading > 0.0) {
            Some(cover) => stack![
                still,
                player,
                image(cover.clone())
                    .width(size)
                    .height(size)
                    .content_fit(ContentFit::Cover)
                    .border_radius(radius)
                    .opacity(fading)
            ]
            .into(),
            None => stack![still, player].into(),
        }
    }
}

/// iced_video_player's own `playbin`, but for video alone: no audio sink is
/// made, so a video with a sound track never opens the sound device the
/// music plays through, and no subtitles are looked for.
fn silent(uri: &reqwest::Url) -> Result<Video, iced_video_player::Error> {
    gst::init()?;
    let description = format!(
        "playbin uri=\"{}\" flags=video video-sink=\"videoscale ! videoconvert ! \
         appsink name=iced_video drop=true caps=video/x-raw,format=NV12,pixel-aspect-ratio=1/1\"",
        uri.as_str()
    );
    let pipeline = gst::parse::launch(&description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| iced_video_player::Error::Cast)?;
    // Checked first: reading a property that isn't there panics.
    let sink = pipeline
        .has_property("video-sink", Some(gst::Element::static_type()))
        .then(|| pipeline.property::<Option<gst::Element>>("video-sink"))
        .flatten()
        .and_then(|sink| sink.downcast::<gst::Bin>().ok())
        .and_then(|bin| bin.by_name("iced_video"))
        .and_then(|sink| sink.downcast::<gstreamer_app::AppSink>().ok())
        .ok_or_else(|| iced_video_player::Error::AppSink("iced_video".into()))?;
    Video::from_gst_pipeline(pipeline, sink, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays `description` (a GStreamer pipeline) to its end.
    fn run(description: &str) {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(description).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let end = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(60),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        assert!(matches!(
            end.map(|m| m.type_()),
            Some(gst::MessageType::Eos)
        ));
    }

    /// Whether anything in the pipeline plays sound.
    fn has_audio_sink(video: &Video) -> bool {
        video
            .pipeline()
            .iterate_recurse()
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|element| element.factory())
            .any(|factory| factory.klass().contains("Sink/Audio"))
    }

    // Needs GStreamer's base and good plugins (Vorbis, VP8, WebM):
    // `cargo test -p aoide-app -- --ignored motion::video`.
    #[test]
    #[ignore = "needs GStreamer's base and good plugins"]
    fn a_loop_with_a_sound_track_plays_looped_without_sound() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("loop.webm");
        run(&format!(
            "webmmux name=mux ! filesink location=\"{}\" \
             videotestsrc num-buffers=30 ! vp8enc ! mux. \
             audiotestsrc num-buffers=20 ! audioconvert ! vorbisenc ! mux.",
            file.display()
        ));
        let opened = Loop::open(&file).unwrap();
        let video = opened.video.as_deref().unwrap();
        assert!(video.looping());
        assert_eq!(video.size(), (320, 240));
        assert!(!has_audio_sink(video));
    }

    #[test]
    #[ignore = "needs GStreamer's base and good plugins"]
    fn a_file_that_isnt_a_video_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.video");
        std::fs::write(&file, b"\0\0\0\x18ftypisom but nothing after").unwrap();
        assert!(Loop::open(&file).is_err());
    }
}
