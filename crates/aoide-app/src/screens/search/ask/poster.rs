//! A result's picture and, for an artist's portrait, its credit: the
//! author and the licence, each opening its page in the browser.

use aoide_melete::{Candidate, RequestKind};
use iced::Element;

use super::{Message, wrap};
use crate::app::Message as App;
use crate::artwork::Artwork;
use crate::{posters, widgets};

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
    widgets::credit_line(candidate.poster_credit.as_ref()?, move |url| {
        App::Page(key, wrap(Message::OpenLink(url)))
    })
}
