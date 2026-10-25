//! Aoide for Linux: the window, the screens, and the wiring between them.

mod actions;
mod app;
mod artwork;
mod config;
mod credentials;
mod handoff;
mod icons;
mod integration;
mod library;
mod listening;
mod lyrics_panel;
mod nav;
mod now_playing;
mod playback;
mod player_bar;
mod playing;
mod queue_panel;
mod radio;
mod screens;
mod session;
mod shell;
mod store_worker;
mod sync;
mod theme;
mod widgets;

use iced::window;

use crate::app::App;
use crate::config::{Config, Paths, WindowGeometry};

fn main() -> iced::Result {
    // Single instance, before anything else: a second launch asks the running
    // Aoide to come forward and leaves, touching neither the config nor a
    // window. The integration lives until `main` returns.
    let desktop = match integration::start(&aoide_mpris::Bus::Session) {
        integration::Start::First(desktop) => desktop,
        integration::Start::Other { raised } => {
            if !raised {
                eprintln!("Aoide is already running, but didn't answer.");
            }
            return Ok(());
        }
    };
    let link = desktop.link.clone();

    let paths = Paths::system();
    let config = paths
        .as_ref()
        .map(|paths| Config::load(&paths.config_file))
        .unwrap_or_else(|| Config::load(std::path::Path::new("")));
    if let Some(paths) = &paths {
        // The device id is minted on first load and must survive this launch
        // even if the app is killed before it next saves.
        let _ = config.save(&paths.config_file);
    }

    let geometry = config.window;
    let settings = window::Settings {
        size: iced::Size::new(geometry.width, geometry.height),
        min_size: Some(iced::Size::new(
            WindowGeometry::MIN_WIDTH,
            WindowGeometry::MIN_HEIGHT,
        )),
        position: match (geometry.x, geometry.y) {
            (Some(x), Some(y)) => window::Position::Specific(iced::Point::new(x, y)),
            _ => window::Position::Centered,
        },
        ..window::Settings::default()
    };

    let result = iced::application(
        move || App::boot(paths.clone(), config.clone(), link.clone()),
        App::update,
        App::view,
    )
    .font(widgets::FONTS[0])
    .font(widgets::FONTS[1])
    .default_font(widgets::REGULAR)
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .window(settings)
    .exit_on_close_request(false)
    .run();
    drop(desktop);
    result
}
