//! What is being heard, for drawing: a spectrum in log-spaced bands and a
//! short waveform.
//!
//! The render thread only copies: every frame it renders (silence included)
//! goes, downmixed to mono, into a ring of atomics, and the engine says which
//! frame is being heard (the rendered position minus the output latency, as
//! for the position). That costs it one store per frame and never blocks or
//! allocates. Everything else happens on the reading side: a [`Visualizer`]
//! takes the window that ends at the heard frame, applies a Hann window, runs
//! a real FFT and sums the power into bands. Read it at whatever rate the UI
//! draws, 30 to 60 times a second.
//!
//! The tap is after the equaliser, the compressor and the pause fade, before
//! the volume: the bars show the music, not where the volume slider is.
//!
//! ```no_run
//! # use aoide_audio::{Output, visualizer::{Visualizer, VisualizerConfig}};
//! let (output, handle, events) = Output::start_default()?;
//! let mut visualizer = Visualizer::new(handle.visualizer_tap(), VisualizerConfig::default());
//! // Each frame the UI draws:
//! let frame = visualizer.read();
//! let _bars: &[f32] = frame.levels; // 0 to 1, low to high
//! let _wave: &[f32] = frame.waveform; // −1 to 1, oldest first
//! # Ok::<(), aoide_audio::Error>(())
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// The render side and the reading side's shared ring.
pub(crate) struct TapRing {
    samples: Box<[AtomicU32]>,
    mask: u64,
    /// Frames ever written.
    written: AtomicU64,
    /// The frame being heard now.
    heard: AtomicU64,
    sample_rate: u32,
}

impl TapRing {
    /// A ring holding about a second, so the window, however large, and the
    /// output latency both fit.
    pub(crate) fn new(sample_rate: u32) -> Self {
        let capacity = (sample_rate as usize).next_power_of_two().max(1 << 14);
        Self {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            mask: capacity as u64 - 1,
            written: AtomicU64::new(0),
            heard: AtomicU64::new(0),
            sample_rate,
        }
    }

    /// Stores the mono sample for frame `at`. Real-time safe.
    #[inline]
    pub(crate) fn write(&self, at: u64, sample: f32) {
        self.samples[(at & self.mask) as usize].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// Everything before `written` has been stored; `heard` is being heard.
    pub(crate) fn publish(&self, written: u64, heard: u64) {
        self.written.store(written, Ordering::Release);
        self.heard.store(heard.min(written), Ordering::Release);
    }

    fn capacity(&self) -> u64 {
        self.mask + 1
    }

    /// Copies the `out.len()` frames that end just before `end`, zeros for
    /// any never written. Whether they were all still in the ring when the
    /// copy finished (a copy the writer lapped is torn).
    fn copy(&self, end: u64, out: &mut [f32]) -> bool {
        let start = end.saturating_sub(out.len() as u64);
        let missing = out.len() - (end - start) as usize;
        out[..missing].fill(0.0);
        for (slot, at) in out[missing..].iter_mut().zip(start..end) {
            *slot = f32::from_bits(self.samples[(at & self.mask) as usize].load(Ordering::Relaxed));
        }
        // The writer stores a block's frames before it publishes them, so
        // leave it a quarter of the ring past what it has said.
        let written = self.written.load(Ordering::Acquire);
        written.saturating_sub(start) <= self.capacity() - self.capacity() / 4
    }
}

/// The handle a [`Visualizer`] reads through; from
/// [`crate::EngineHandle::visualizer_tap`]. Cheap to clone.
#[derive(Clone)]
pub struct VisualizerTap {
    ring: Arc<TapRing>,
}

impl VisualizerTap {
    pub(crate) fn new(ring: Arc<TapRing>) -> Self {
        Self { ring }
    }

    /// The engine's sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.ring.sample_rate
    }
}

/// How a [`Visualizer`] analyses what is heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualizerConfig {
    /// FFT length in frames, a power of two: 2048 is 43 ms at 48 kHz, with
    /// bins 23 Hz apart.
    pub fft_size: usize,
    /// How many bands, log-spaced from `min_hz` to `max_hz`.
    pub bands: usize,
    /// The lowest band's lower edge, in hertz.
    pub min_hz: f32,
    /// The highest band's upper edge, in hertz.
    pub max_hz: f32,
    /// A band at this level or below shows as 0; 0 dBFS (a full-scale sine
    /// in the band) shows as 1.
    pub floor_db: f32,
    /// How a falling band eases down: the time constant, in heard time, of
    /// its approach to the new level. A rising band jumps straight up. Zero
    /// for no smoothing.
    pub fall_seconds: f32,
    /// Frames of waveform, ending at the heard frame.
    pub waveform_frames: usize,
}

impl Default for VisualizerConfig {
    fn default() -> Self {
        Self {
            fft_size: 2048,
            bands: 32,
            min_hz: 50.0,
            max_hz: 16_000.0,
            floor_db: -80.0,
            fall_seconds: 0.15,
            waveform_frames: 1024,
        }
    }
}

/// One reading.
pub struct VisualFrame<'a> {
    /// Each band, 0 (at or under the floor) to 1 (0 dBFS), smoothed.
    pub levels: &'a [f32],
    /// Each band's power in dBFS, unsmoothed: a full-scale sine wholly in a
    /// band reads 0.
    pub levels_db: &'a [f32],
    /// The heard waveform, mono, oldest first.
    pub waveform: &'a [f32],
    /// The frame (on the engine's clock) the window ends at.
    pub heard_frame: u64,
}

/// The reading side. Allocates in [`Visualizer::new`] only.
pub struct Visualizer {
    tap: VisualizerTap,
    config: VisualizerConfig,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Each band's bins, `start..end`.
    band_bins: Vec<(usize, usize)>,
    band_edges: Vec<f32>,
    /// What a full-scale sine's power sums to.
    full_scale: f32,
    levels: Vec<f32>,
    levels_db: Vec<f32>,
    waveform: Vec<f32>,
    last_heard: Option<u64>,
}

impl Visualizer {
    /// `config.fft_size` is rounded up to a power of two of at least 64, and
    /// `bands` held to at least one.
    pub fn new(tap: VisualizerTap, mut config: VisualizerConfig) -> Self {
        config.fft_size = config.fft_size.max(64).next_power_of_two();
        config.bands = config.bands.max(1);
        let n = config.fft_size;
        let rate = tap.sample_rate() as f32;
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let window: Vec<f32> = (0..n)
            .map(|k| {
                let phase = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (0.5 - 0.5 * phase.cos()) as f32
            })
            .collect();
        // Parseval: a sine of amplitude A puts N·Σw²·A²/4 into the
        // positive-frequency bins.
        let full_scale = n as f32 * window.iter().map(|w| w * w).sum::<f32>() / 4.0;

        let max_hz = config.max_hz.min(rate / 2.0).max(config.min_hz * 1.01);
        let min_hz = config.min_hz.max(rate / n as f32 / 2.0);
        let ratio = max_hz / min_hz;
        let band_edges: Vec<f32> = (0..=config.bands)
            .map(|i| min_hz * ratio.powf(i as f32 / config.bands as f32))
            .collect();
        let bin_hz = rate / n as f32;
        let last_bin = n / 2;
        let band_bins = band_edges
            .windows(2)
            .map(|edge| {
                let start = (edge[0] / bin_hz).ceil() as usize;
                let end = ((edge[1] / bin_hz).ceil() as usize).min(last_bin + 1);
                if start < end {
                    (start, end)
                } else {
                    // Narrower than a bin: the bin nearest its centre.
                    let centre = ((edge[0] * edge[1]).sqrt() / bin_hz).round() as usize;
                    let centre = centre.min(last_bin);
                    (centre, centre + 1)
                }
            })
            .collect();
        let input = fft.make_input_vec();
        let spectrum = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self {
            config,
            fft,
            window,
            input,
            spectrum,
            scratch,
            band_bins,
            band_edges,
            full_scale,
            levels: vec![0.0; config.bands],
            levels_db: vec![config.floor_db; config.bands],
            waveform: vec![0.0; config.waveform_frames],
            last_heard: None,
            tap,
        }
    }

    /// The configuration it was made with.
    pub fn config(&self) -> VisualizerConfig {
        self.config
    }

    /// The bands' edges in Hz, `bands + 1` of them, low to high: band `i`
    /// runs from edge `i` to edge `i + 1`.
    pub fn band_edges(&self) -> &[f32] {
        &self.band_edges
    }

    /// The spectrum and waveform of what is being heard now. Never blocks.
    pub fn read(&mut self) -> VisualFrame<'_> {
        let ring = &self.tap.ring;
        let mut heard = ring.heard.load(Ordering::Acquire);
        // A torn copy means the reader fell a whole ring behind: take it
        // again from where the writer is now.
        for _ in 0..3 {
            let whole = ring.copy(heard, &mut self.input) && ring.copy(heard, &mut self.waveform);
            if whole {
                break;
            }
            heard = ring.heard.load(Ordering::Acquire);
        }

        for (sample, w) in self.input.iter_mut().zip(&self.window) {
            *sample *= w;
        }
        // The lengths are the plan's own, so this cannot fail.
        let _ =
            self.fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch);

        let elapsed = self.last_heard.map_or(0.0, |last| {
            heard.saturating_sub(last) as f32 / ring.sample_rate as f32
        });
        self.last_heard = Some(heard);
        let keep = if self.config.fall_seconds > 0.0 {
            (-elapsed / self.config.fall_seconds).exp()
        } else {
            0.0
        };
        let floor = self.config.floor_db;
        for (band, &(start, end)) in self.band_bins.iter().enumerate() {
            let power: f32 = self.spectrum[start..end].iter().map(|c| c.norm_sqr()).sum();
            let db = if power > 0.0 {
                (10.0 * (power / self.full_scale).log10()).max(floor)
            } else {
                floor
            };
            self.levels_db[band] = db;
            let level = ((db - floor) / -floor).clamp(0.0, 1.0);
            let previous = self.levels[band];
            self.levels[band] = if level >= previous {
                level
            } else {
                level + (previous - level) * keep
            };
        }
        VisualFrame {
            levels: &self.levels,
            levels_db: &self.levels_db,
            waveform: &self.waveform,
            heard_frame: heard,
        }
    }
}
