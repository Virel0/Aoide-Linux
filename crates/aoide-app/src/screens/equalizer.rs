//! The equaliser and the compressor: on and off, the presets (built-in and
//! the listener's own), the preamp and a slider per band with the curve
//! drawn above them, and the compressor's controls.
//!
//! Opened from Settings → Playback and from the player bar's menu. The page
//! holds only what is being typed; the settings are the app's (it saves
//! them and hands them to the engine), so the app draws this page.

use aoide_audio::eq;
use iced::widget::{
    Column, button, canvas, column, container, pick_list, row, slider, text, text_input, toggler,
    vertical_slider,
};
use iced::{Center, Color, Element, Fill, Length, Point, Rectangle, Renderer, Task, Theme, mouse};

use super::{PageMsg, View};
use crate::app::{App, Message as AppMsg};
use crate::sound::{self, CompressorChoice, CompressorField, EqualizerSettings, SoundSettings};
use crate::{theme, widgets};

#[derive(Debug, Default)]
pub struct State {
    /// The name being typed for Save Preset.
    pub name: String,
    /// Why the last save was refused.
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Equalizer(bool),
    Preset(String),
    Preamp(f32),
    Band(usize, f32),
    Name(String),
    SavePreset,
    DeletePreset(String),
    Compressor(bool),
    CompressorPreset(String),
    CompressorValue(CompressorField, f32),
    /// A slider was let go: the settings are saved now, not on every step.
    Commit,
}

/// What a message asks of the app once the settings have taken it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Nothing,
    /// Hand the settings to the engine; save them later.
    Engine,
    /// Hand them to the engine and save them.
    EngineAndSave,
    Save,
}

/// Applies `message` to the settings and the page: pure, so the screen's
/// rules are tested without a window.
pub fn apply(sound: &mut SoundSettings, page: &mut State, message: Message) -> Effect {
    let eq = &mut sound.equalizer;
    let comp = &mut sound.compressor;
    match message {
        Message::Equalizer(on) => {
            eq.enabled = on;
            Effect::EngineAndSave
        }
        Message::Preset(name) => {
            if eq.choose(&name) {
                page.error = None;
                Effect::EngineAndSave
            } else {
                Effect::Nothing
            }
        }
        Message::Preamp(db) => {
            eq.set_preamp(db);
            Effect::Engine
        }
        Message::Band(index, db) => {
            eq.set_band(index, db);
            Effect::Engine
        }
        Message::Name(name) => {
            page.name = name;
            page.error = None;
            Effect::Nothing
        }
        Message::SavePreset => match eq.save_preset(&page.name) {
            Ok(()) => {
                page.name.clear();
                page.error = None;
                Effect::Save
            }
            Err(why) => {
                page.error = Some(why);
                Effect::Nothing
            }
        },
        Message::DeletePreset(name) => {
            eq.delete_preset(&name);
            Effect::Save
        }
        Message::Compressor(on) => {
            comp.enabled = on;
            Effect::EngineAndSave
        }
        Message::CompressorPreset(name) => {
            if comp.choose(&name) {
                Effect::EngineAndSave
            } else {
                Effect::Nothing
            }
        }
        Message::CompressorValue(field, value) => {
            comp.set(field, value);
            Effect::Engine
        }
        Message::Commit => Effect::Save,
    }
}

impl App {
    /// A message from the equaliser page with `key`.
    pub(crate) fn equalizer_message(&mut self, key: u64, message: Message) -> Task<AppMsg> {
        let page = self
            .session
            .as_mut()
            .and_then(|s| s.nav.as_mut())
            .and_then(|nav| nav.iter_mut().find(|e| e.key == key))
            .and_then(|e| match &mut e.page {
                super::Page::Equalizer(state) => Some(state),
                _ => None,
            });
        let mut scratch = State::default();
        let page = page.unwrap_or(&mut scratch);
        let effect = apply(&mut self.config.sound, page, message);
        if matches!(effect, Effect::Engine | Effect::EngineAndSave)
            && let Err(error) = self.playback.apply_sound(&self.config.sound)
        {
            self.notice = Some(format!("The equaliser couldn't be set: {error}"));
        }
        if matches!(effect, Effect::Save | Effect::EngineAndSave) {
            self.save_config();
        }
        Task::none()
    }
}

/// The line in Settings → Playback that opens this page, saying how things
/// stand.
pub fn settings_row(sound: &SoundSettings) -> Element<'_, AppMsg> {
    let state = |on: bool, label: String| {
        if on {
            format!("On · {label}")
        } else {
            "Off".to_owned()
        }
    };
    let summary = format!(
        "Equaliser: {}  ·  Compressor: {}",
        state(sound.equalizer.enabled, sound.equalizer.label()),
        state(sound.compressor.enabled, sound.compressor.label()),
    );
    row![
        column![
            text("Equaliser and Compressor"),
            widgets::secondary(summary).size(13)
        ]
        .spacing(2)
        .width(Fill),
        button(text("Open"))
            .style(theme::tonal)
            .padding([6, 14])
            .on_press(AppMsg::Listening(crate::listening::Msg::OpenEqualizer)),
    ]
    .spacing(10)
    .align_y(Center)
    .into()
}

/// The page's own message, as the app routes it.
fn wrap(key: u64) -> impl Fn(Message) -> AppMsg + Copy {
    move |m| AppMsg::Page(key, PageMsg::Equalizer(m))
}

fn section<'a>(
    title: &'a str,
    switch: Element<'a, AppMsg>,
    body: Column<'a, AppMsg>,
    footer: &'a str,
) -> Element<'a, AppMsg> {
    column![
        row![text(title).size(20).font(widgets::BOLD).width(Fill), switch].align_y(Center),
        container(body.spacing(14))
            .padding(16)
            .width(Fill)
            .style(theme::menu),
        widgets::secondary(footer).size(12),
    ]
    .spacing(10)
    .into()
}

pub fn view<'a>(
    state: &'a State,
    sound: &'a SoundSettings,
    sample_rate: u32,
    view: &View<'a>,
) -> Element<'a, AppMsg> {
    let message = wrap(view.key);
    widgets::page(
        column![
            widgets::title_bar("Equaliser and Compressor", view.can_go_back),
            equalizer(state, &sound.equalizer, sample_rate, message),
            compressor(&sound.compressor, message),
        ]
        .spacing(28)
        .max_width(820),
    )
}

fn equalizer<'a>(
    state: &'a State,
    eq: &'a EqualizerSettings,
    sample_rate: u32,
    message: impl Fn(Message) -> AppMsg + Copy + 'a,
) -> Element<'a, AppMsg> {
    let curve = sound::editable(&eq.curve);
    let mut names = eq.preset_names();
    let label = eq.label();
    if !names.contains(&label) {
        names.insert(0, label.clone());
    }
    let mut presets = row![
        text("Preset").width(Length::Fixed(70.0)),
        pick_list(names, Some(label.clone()), move |name| message(
            Message::Preset(name)
        ))
        .padding([6, 10])
        .width(Length::Fixed(220.0)),
    ]
    .spacing(10)
    .align_y(Center);
    if eq.is_user_preset(&label) {
        presets = presets.push(
            button(text("Delete Preset").size(13))
                .style(theme::destructive)
                .padding([6, 12])
                .on_press(message(Message::DeletePreset(label.clone()))),
        );
    }
    presets = presets.push(crate::shell::gap()).push(
        text_input("Name a new preset", &state.name)
            .on_input(move |s| message(Message::Name(s)))
            .on_submit(message(Message::SavePreset))
            .padding(6)
            .size(13)
            .width(Length::Fixed(180.0))
            .style(theme::field),
    );
    presets = presets.push(
        button(text("Save Preset").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press_maybe((!state.name.trim().is_empty()).then(|| message(Message::SavePreset))),
    );

    let graph = canvas(CurveGraph {
        points: sound::curve_points(&curve, sample_rate, 160),
        enabled: eq.enabled,
    })
    .width(Fill)
    .height(Length::Fixed(170.0));

    let band = |index: usize, freq: f32, db: f32| -> Element<'a, AppMsg> {
        column![
            widgets::secondary(sound::gain_label(db)).size(11),
            vertical_slider(
                -eq::GAIN_LIMIT_DB..=eq::GAIN_LIMIT_DB,
                db,
                move |v| message(Message::Band(index, v))
            )
            .step(0.5_f32)
            .height(Length::Fixed(150.0))
            .on_release(message(Message::Commit))
            .style(theme::scrubber),
            text(sound::frequency_label(freq)).size(12),
        ]
        .spacing(6)
        .align_x(Center)
        .width(Fill)
        .into()
    };
    let bands = row(curve
        .bands
        .iter()
        .enumerate()
        .map(|(i, b)| band(i, b.freq_hz, b.gain_db)))
    .spacing(4)
    .width(Fill);

    let preamp = row![
        text("Preamp").width(Length::Fixed(70.0)),
        slider(
            -eq::GAIN_LIMIT_DB..=eq::GAIN_LIMIT_DB,
            curve.preamp_db,
            move |v| message(Message::Preamp(v))
        )
        .step(0.5_f32)
        .on_release(message(Message::Commit))
        .style(theme::scrubber),
        widgets::secondary(sound::gain_label(curve.preamp_db))
            .size(13)
            .width(Length::Fixed(64.0)),
    ]
    .spacing(10)
    .align_y(Center);

    let mut body = column![presets];
    if let Some(error) = &state.error {
        body = body.push(
            text(error.as_str())
                .size(13)
                .style(|t: &Theme| text::Style {
                    color: Some(t.palette().danger),
                }),
        );
    }
    body = body.push(graph).push(preamp).push(bands);
    if eq.enabled
        && let Some(advice) = clip_advice(curve.preamp_db, curve.peak_db(sample_rate))
    {
        body = body.push(widgets::secondary(advice).size(12));
    }
    section(
        "Equaliser",
        toggler(eq.enabled)
            .style(theme::switch)
            .on_toggle(move |on| message(Message::Equalizer(on)))
            .into(),
        body,
        "Shapes the sound on this computer: the preamp first, then ten bands an octave apart. \
         The built-in presets lower the preamp by their highest boost, so they never make a \
         track clip. Off, the sound is left exactly as it was.",
    )
}

fn compressor<'a>(
    comp: &'a CompressorChoice,
    message: impl Fn(Message) -> AppMsg + Copy + 'a,
) -> Element<'a, AppMsg> {
    let mut names = CompressorChoice::preset_names();
    let label = comp.label();
    if !names.contains(&label) {
        names.insert(0, label.clone());
    }
    let mut body = column![
        row![
            text("Preset").width(Length::Fixed(110.0)),
            pick_list(names, Some(label), move |name| message(
                Message::CompressorPreset(name)
            ))
            .padding([6, 10])
            .width(Length::Fixed(220.0)),
        ]
        .spacing(10)
        .align_y(Center)
    ];
    for field in CompressorField::ALL {
        let (low, high) = field.range();
        let value = field.value(&comp.settings);
        body = body.push(
            row![
                text(field.title()).width(Length::Fixed(110.0)),
                slider(low..=high, value, move |v| message(
                    Message::CompressorValue(field, v)
                ))
                .step(field.step())
                .on_release(message(Message::Commit))
                .style(theme::scrubber),
                widgets::secondary(field.label(value))
                    .size(13)
                    .width(Length::Fixed(90.0)),
            ]
            .spacing(10)
            .align_y(Center),
        );
    }
    section(
        "Compressor",
        toggler(comp.enabled)
            .style(theme::switch)
            .on_toggle(move |on| message(Message::Compressor(on)))
            .into(),
        body,
        "Evens out loud and quiet passages: above the threshold, every few decibels in come out \
         as one. Late Night lifts quiet passages and holds loud ones down, for listening at a low \
         volume. Makeup gain above 0 dB can clip a track that already peaks at full scale.",
    )
}

/// What to say when the curve lifts anything above the music: how far, and
/// the preamp (to the half decibel below) that takes it back under.
pub fn clip_advice(preamp_db: f32, peak_db: f64) -> Option<String> {
    if peak_db <= 0.05 {
        return None;
    }
    let needed = f64::from(preamp_db) - peak_db;
    let suggested = ((needed * 2.0).floor() / 2.0) as f32;
    let peak = sound::gain_label(peak_db as f32);
    Some(if suggested >= -aoide_audio::eq::GAIN_LIMIT_DB {
        format!(
            "The curve peaks at {peak} above the music: a loud track may clip. A preamp of {} \
             keeps it under.",
            sound::gain_label(suggested)
        )
    } else {
        format!(
            "The curve peaks at {peak} above the music: a loud track may clip. Lower the \
             highest bands to keep it under."
        )
    })
}

/// The equaliser's response, drawn from 20 Hz to 20 kHz and ±15 dB.
struct CurveGraph {
    points: Vec<(f64, f64)>,
    enabled: bool,
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
        let (width, height) = (f64::from(bounds.width), f64::from(bounds.height));
        let (low, high) = (
            f64::from(eq::MIN_FREQ_HZ),
            f64::from(eq::MAX_FREQ_HZ).max(f64::from(eq::MIN_FREQ_HZ) * 2.0),
        );
        let x = |freq: f64| ((freq / low).ln() / (high / low).ln() * width) as f32;
        let y = |db: f64| {
            ((GRAPH_DB - db.clamp(-GRAPH_DB, GRAPH_DB)) / (2.0 * GRAPH_DB) * height) as f32
        };
        let faint = theme::secondary(theme).scale_alpha(0.35);
        let label = theme::secondary(theme);

        for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
            let line = canvas::Path::line(Point::new(0.0, y(db)), Point::new(width as f32, y(db)));
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
                position: Point::new(4.0, y(db) - 14.0),
                color: label,
                size: 11.0.into(),
                font: widgets::REGULAR,
                ..canvas::Text::default()
            });
        }
        for freq in [32.0, 125.0, 500.0, 2_000.0, 8_000.0] {
            let at = x(freq);
            frame.stroke(
                &canvas::Path::line(Point::new(at, 0.0), Point::new(at, height as f32)),
                canvas::Stroke::default().with_color(faint).with_width(1.0),
            );
            frame.fill_text(canvas::Text {
                content: format!("{} Hz", sound::frequency_label(freq as f32)),
                position: Point::new(at + 4.0, height as f32 - 15.0),
                color: label,
                size: 11.0.into(),
                font: widgets::REGULAR,
                ..canvas::Text::default()
            });
        }
        let curve = canvas::Path::new(|path| {
            for (i, (freq, db)) in self.points.iter().enumerate() {
                let point = Point::new(x(*freq), y(*db));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_advice_rounds_the_preamp_down() {
        assert_eq!(clip_advice(0.0, 0.0), None);
        assert_eq!(clip_advice(-6.0, -1.0), None);
        let advice = clip_advice(-6.0, 4.4).unwrap();
        assert!(advice.contains("\u{2212}10.5 dB"), "{advice}");
        let advice = clip_advice(-6.0, 4.5).unwrap();
        assert!(advice.contains("\u{2212}10.5 dB"), "{advice}");
        let advice = clip_advice(-10.0, 6.0).unwrap();
        assert!(advice.contains("Lower the highest bands"), "{advice}");
    }

    #[test]
    fn switching_and_choosing_go_to_the_engine_and_the_file() {
        let mut sound = SoundSettings::default();
        let mut page = State::default();
        assert_eq!(
            apply(&mut sound, &mut page, Message::Equalizer(true)),
            Effect::EngineAndSave
        );
        assert!(sound.equalizer.enabled);
        assert_eq!(
            apply(&mut sound, &mut page, Message::Preset("Vocal".into())),
            Effect::EngineAndSave
        );
        assert_eq!(sound.equalizer.label(), "Vocal");
        assert_eq!(
            apply(&mut sound, &mut page, Message::Preset("Nope".into())),
            Effect::Nothing
        );
        assert_eq!(sound.equalizer.label(), "Vocal");
    }

    #[test]
    fn dragging_reaches_the_engine_and_letting_go_saves() {
        let mut sound = SoundSettings::default();
        let mut page = State::default();
        assert_eq!(
            apply(&mut sound, &mut page, Message::Band(4, 3.0)),
            Effect::Engine
        );
        assert_eq!(
            apply(&mut sound, &mut page, Message::Preamp(-3.0)),
            Effect::Engine
        );
        assert_eq!(
            apply(
                &mut sound,
                &mut page,
                Message::CompressorValue(CompressorField::Ratio, 4.0)
            ),
            Effect::Engine
        );
        assert_eq!(apply(&mut sound, &mut page, Message::Commit), Effect::Save);
        assert_eq!(sound.equalizer.curve.bands[4].gain_db, 3.0);
        assert_eq!(sound.equalizer.curve.preamp_db, -3.0);
        assert_eq!(sound.compressor.settings.ratio, 4.0);
        assert_eq!(sound.compressor.label(), sound::CUSTOM);
    }

    #[test]
    fn a_refused_name_is_said_and_a_good_one_clears_the_field() {
        let mut sound = SoundSettings::default();
        let mut page = State::default();
        apply(&mut sound, &mut page, Message::Name("Flat".into()));
        assert_eq!(
            apply(&mut sound, &mut page, Message::SavePreset),
            Effect::Nothing
        );
        assert!(page.error.is_some());
        apply(&mut sound, &mut page, Message::Name("Headphones".into()));
        assert!(page.error.is_none());
        assert_eq!(
            apply(&mut sound, &mut page, Message::SavePreset),
            Effect::Save
        );
        assert!(page.name.is_empty());
        assert_eq!(sound.equalizer.label(), "Headphones");
        assert_eq!(
            apply(
                &mut sound,
                &mut page,
                Message::DeletePreset("Headphones".into())
            ),
            Effect::Save
        );
        assert!(sound.equalizer.user_presets.is_empty());
    }
}
