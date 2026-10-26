//! The equaliser's curve, drawn on a canvas over its decibel and frequency
//! grid.

use aoide_audio::eq;
use iced::widget::canvas;
use iced::{Color, Point, Rectangle, Renderer, Theme, mouse};

use crate::{sound, theme, widgets};

/// The equaliser's response, drawn from 20 Hz to 20 kHz and ±15 dB.
pub(super) struct CurveGraph {
    pub(super) points: Vec<(f64, f64)>,
    pub(super) enabled: bool,
}

const GRAPH_DB: f64 = 15.0;

impl<Message> canvas::Program<Message> for CurveGraph {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let scale = Scale {
            width: f64::from(bounds.width),
            height: f64::from(bounds.height),
            low: f64::from(eq::MIN_FREQ_HZ),
            high: f64::from(eq::MAX_FREQ_HZ).max(f64::from(eq::MIN_FREQ_HZ) * 2.0),
        };
        let faint = theme::secondary(theme).scale_alpha(0.35);
        let label = theme::secondary(theme);
        gain_lines(&mut frame, &scale, faint, label);
        frequency_lines(&mut frame, &scale, faint, label);
        let curve = canvas::Path::new(|path| {
            for (i, (freq, db)) in self.points.iter().enumerate() {
                let point = Point::new(scale.x(*freq), scale.y(*db));
                if i == 0 {
                    path.move_to(point);
                } else {
                    path.line_to(point);
                }
            }
        });
        let colour: Color = if self.enabled {
            theme::accent(theme)
        } else {
            theme::secondary(theme)
        };
        frame.stroke(
            &curve,
            canvas::Stroke::default()
                .with_color(colour)
                .with_width(2.5)
                .with_line_join(canvas::LineJoin::Round),
        );
        vec![frame.into_geometry()]
    }
}

/// Where a frequency (on a log scale) and a gain land on the canvas.
struct Scale {
    width: f64,
    height: f64,
    low: f64,
    high: f64,
}

impl Scale {
    fn x(&self, freq: f64) -> f32 {
        ((freq / self.low).ln() / (self.high / self.low).ln() * self.width) as f32
    }

    fn y(&self, db: f64) -> f32 {
        ((GRAPH_DB - db.clamp(-GRAPH_DB, GRAPH_DB)) / (2.0 * GRAPH_DB) * self.height) as f32
    }
}

/// A line every 6 dB, labelled, 0 dB a little stronger.
fn gain_lines(frame: &mut canvas::Frame, scale: &Scale, faint: Color, label: Color) {
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let line = canvas::Path::line(
            Point::new(0.0, scale.y(db)),
            Point::new(scale.width as f32, scale.y(db)),
        );
        frame.stroke(
            &line,
            canvas::Stroke::default()
                .with_color(if db == 0.0 {
                    label.scale_alpha(0.6)
                } else {
                    faint
                })
                .with_width(1.0),
        );
        frame.fill_text(canvas::Text {
            content: sound::gain_label(db as f32),
            position: Point::new(4.0, scale.y(db) - 14.0),
            color: label,
            size: 11.0.into(),
            font: widgets::REGULAR,
            ..canvas::Text::default()
        });
    }
}

/// A line at five frequencies two octaves apart, labelled.
fn frequency_lines(frame: &mut canvas::Frame, scale: &Scale, faint: Color, label: Color) {
    for freq in [32.0, 125.0, 500.0, 2_000.0, 8_000.0] {
        let at = scale.x(freq);
        frame.stroke(
            &canvas::Path::line(Point::new(at, 0.0), Point::new(at, scale.height as f32)),
            canvas::Stroke::default().with_color(faint).with_width(1.0),
        );
        frame.fill_text(canvas::Text {
            content: format!("{} Hz", sound::frequency_label(freq as f32)),
            position: Point::new(at + 4.0, scale.height as f32 - 15.0),
            color: label,
            size: 11.0.into(),
            font: widgets::REGULAR,
            ..canvas::Text::default()
        });
    }
}
