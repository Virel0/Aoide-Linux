//! Aoide for Linux: the window, the screens, and the wiring between them.

// Services built ahead of the screens that use them; removed once they are wired.
#![allow(dead_code)]

mod app;
mod artwork;
mod config;
mod credentials;
mod icons;
mod integration;
mod library;
mod nav;
mod playback;
mod session;
mod store_worker;
mod sync;
mod theme;

use iced::window;

use crate::app::App;
use crate::config::{Config, Paths, WindowGeometry};

fn main() -> iced::Result {
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

    // MPRIS: single instance goes here. With aoide-mpris, a launch that finds
    // the bus name taken asks the running one to raise its window and returns
    // before opening a second (see `integration.rs`).

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

    iced::application(
        move || App::boot(paths.clone(), config.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .window(settings)
    .exit_on_close_request(false)
    .run()
}
