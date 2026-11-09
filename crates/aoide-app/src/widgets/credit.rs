//! A portrait's credit as one small line, "Photo: {author}, {licence}",
//! each part a link where it has one.

use aoide_melete::PosterCredit;
use iced::Element;
use iced::widget::{Row, button};

use crate::app::Message;
use crate::{posters, theme};

/// The credit's line, each linked part sending `open` with its `https`
/// address; `None` when the credit names nothing.
pub fn credit_line<'a>(
    credit: &PosterCredit,
    open: impl Fn(String) -> Message,
) -> Option<Element<'a, Message>> {
    let parts = posters::credit(credit);
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
        let label = super::secondary(words).size(11);
        line = line.push(match link {
            Some(url) => Element::from(
                button(label)
                    .style(theme::row)
                    .padding([0, 2])
                    .on_press(open(url)),
            ),
            None => label.into(),
        });
    }
    Some(line.spacing(2).into())
}
