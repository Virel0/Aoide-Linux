//! Aoide for Linux: the window, the screens, and the wiring between them.

use iced::widget::{center, text};
use iced::{Element, Theme};

fn main() -> iced::Result {
    iced::application(App::default, App::update, App::view)
        .title("Aoide")
        .theme(|_: &App| Theme::Dark)
        .run()
}

#[derive(Default)]
struct App;

#[derive(Debug, Clone)]
enum Message {}

impl App {
    fn update(&mut self, message: Message) {
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        center(text("Aoide").size(32)).into()
    }
}
