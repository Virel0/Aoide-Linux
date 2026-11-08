//! A result's picture and, for an artist's portrait, its credit: the
//! author and the licence, each opening its page in the browser.

use aoide_melete::{Candidate, RequestKind};
use iced::widget::{Row, button};
use iced::{Element, Task};

use super::{Message, wrap};
use crate::app::Message as App;
use crate::artwork::Artwork;
use crate::{posters, theme, widgets};

/// How wide a result's picture is drawn.
const SIDE: f32 = 48.0;

/// The picture beside a result, round for an artist; `None` without one.
pub fn picture<'a>(candidate: &Candidate, artwork: &'a Artwork) -> Option<Element<'a, App>> {
    let key = posters::key(candidate.poster.as_deref()?);
    let radius = if candidate.kind == RequestKind::Artist {
        SIDE / 2.0
    } else {
        6.0
    };
    Some(widgets::art(Some(&key), artwork, SIDE, radius))
}

/// "Photo: {author}, {licence}" under a portrait, each part a link where
/// there is one; `None` without a credit, or without a picture to credit.
pub fn credit<'a>(candidate: &Candidate, key: u64) -> Option<Element<'a, App>> {
    candidate.poster.as_ref()?;
    let parts = posters::credit(candidate.poster_credit.as_ref()?);
    if parts.is_empty() {
        return None;
    }
    let count = parts.len();
    let mut line = Row::new();
    for (place, (words, link)) in parts.into_iter().enumerate() {
        let words = if place + 1 < count {
            format!("{words},")
        } else {
            words
        };
        let label = widgets::secondary(words).size(11);
        line = line.push(match link {
            Some(url) => Element::from(
                button(label)
                    .style(theme::row)
                    .padding([0, 2])
                    .on_press(App::Page(key, wrap(Message::OpenLink(url)))),
            ),
            None => label.into(),
        });
    }
    Some(line.spacing(2).into())
}

/// Opens a credit's link; one that won't open is said.
pub fn open(url: &str) -> Task<App> {
    match crate::updates::open_in_browser(url) {
        Ok(()) => Task::none(),
        Err(why) => Task::done(App::Notice(why)),
    }
}
