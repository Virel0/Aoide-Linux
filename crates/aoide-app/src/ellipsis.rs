//! One line of text that ends in "…" when it doesn't fit, as a title does
//! in Apple Music. iced 0.14's text only clips. The cut is measured with the
//! renderer's own paragraph and made where the text shaper puts a cursor (a
//! grapheme cluster's edge), so a letter, an accent or an emoji is never cut
//! in half. What fits is shown whole and costs one paragraph, as a `text`
//! does; a cut is worked out once per text, size and width, then kept.

use std::borrow::Cow;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::widget::text::{Alignment, LineHeight, Shaping, Wrapping};
use iced::{Color, Element, Font, Length, Pixels, Point, Rectangle, Size, Theme, alignment, mouse};

/// What stands for the rest.
pub const MARK: &str = "\u{2026}";

/// A line of text, shortened with "…" to the room it is given.
pub struct Ellipsis<'a> {
    content: Cow<'a, str>,
    size: Option<Pixels>,
    font: Option<Font>,
    width: Length,
    color: Colour<'a>,
}

/// The colour from the theme; `None` is the surrounding text's.
type Colour<'a> = Box<dyn Fn(&Theme) -> Option<Color> + 'a>;

pub fn ellipsis<'a>(content: impl Into<Cow<'a, str>>) -> Ellipsis<'a> {
    Ellipsis {
        content: content.into(),
        size: None,
        font: None,
        width: Length::Shrink,
        color: Box::new(|_| None),
    }
}

impl<'a> Ellipsis<'a> {
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = Some(size.into());
        self
    }

    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// The colour, from the theme; `None` is the surrounding text colour.
    pub fn color(mut self, color: impl Fn(&Theme) -> Option<Color> + 'a) -> Self {
        self.color = Box::new(color);
        self
    }
}

/// The longest start of `content` that fits `room` with "…" after it, or
/// `content` itself when it fits. `width` measures a string as drawn;
/// `boundary_at` gives the byte offset of the cursor the shaper would put
/// at that distance into the whole text: always a cluster's edge.
pub fn fit<'s>(
    content: &'s str,
    room: f32,
    width: impl Fn(&str) -> f32,
    boundary_at: impl Fn(f32) -> usize,
) -> Cow<'s, str> {
    if width(content) <= room {
        return Cow::Borrowed(content);
    }
    let mut target = room - width(MARK);
    loop {
        if target <= 0.0 {
            return Cow::Borrowed(MARK);
        }
        let mut end = boundary_at(target).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        let shortened = format!("{}{MARK}", content[..end].trim_end());
        let drawn = width(&shortened);
        if drawn <= room || end == 0 {
            return Cow::Owned(shortened);
        }
        // The shaper's cursor can land past the middle of the last letter:
        // step back by what is over, and ask again.
        target -= (drawn - room).max(0.5);
    }
}

struct State<P> {
    /// What the paragraph was made for: the text, its size, font and room.
    made_for: Option<(String, f32, Font, f32)>,
    paragraph: P,
}

fn text_of(content: &str, size: Pixels, font: Font) -> text::Text<&str, Font> {
    text::Text {
        content,
        bounds: Size::INFINITE,
        size,
        line_height: LineHeight::default(),
        font,
        align_x: Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::default(),
        wrapping: Wrapping::None,
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Ellipsis<'_>
where
    Renderer: text::Renderer<Font = Font>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph> {
            made_for: None,
            paragraph: Renderer::Paragraph::default(),
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();
        let size = self.size.unwrap_or_else(|| renderer.default_size());
        let font = self.font.unwrap_or_else(|| renderer.default_font());
        let limits = limits.width(self.width).height(Length::Shrink);
        let room = limits.max().width;
        let current = state.made_for.as_ref().is_some_and(|(content, s, f, r)| {
            content == self.content.as_ref() && *s == size.0 && *f == font && *r == room
        });
        if !current {
            let whole = Renderer::Paragraph::with_text(text_of(&self.content, size, font));
            let middle = whole.min_height() / 2.0;
            let shown = fit(
                &self.content,
                room,
                |s| Renderer::Paragraph::with_text(text_of(s, size, font)).min_width(),
                |x| {
                    whole
                        .hit_test(Point::new(x, middle))
                        .map_or(0, text::Hit::cursor)
                },
            );
            state.paragraph = match shown {
                Cow::Borrowed(s) if s.len() == self.content.len() => whole,
                shown => Renderer::Paragraph::with_text(text_of(&shown, size, font)),
            };
            state.made_for = Some((self.content.to_string(), size.0, font, room));
        }
        let bounds = state.paragraph.min_bounds();
        layout::Node::new(limits.resolve(self.width, Length::Shrink, bounds))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let bounds = layout.bounds();
        renderer.fill_paragraph(
            &state.paragraph,
            bounds.position(),
            (self.color)(theme).unwrap_or(style.text_color),
            *viewport,
        );
    }
}

impl<'a, Message, Renderer> From<Ellipsis<'a>> for Element<'a, Message, Theme, Renderer>
where
    Renderer: text::Renderer<Font = Font> + 'a,
{
    fn from(ellipsis: Ellipsis<'a>) -> Self {
        Element::new(ellipsis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every char 10 wide, the mark too; the cursor lands on the nearest
    /// char edge, as a shaper's does (past the middle of one, after it).
    fn width(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    fn boundary_in(content: &str) -> impl Fn(f32) -> usize + '_ {
        move |x| {
            let n = (x / 10.0).round() as usize;
            content
                .char_indices()
                .nth(n)
                .map_or(content.len(), |(i, _)| i)
        }
    }

    #[test]
    fn what_fits_is_shown_whole() {
        let content = "Harbour Lights";
        assert_eq!(fit(content, 140.0, width, boundary_in(content)), content);
        assert!(matches!(
            fit(content, 500.0, width, boundary_in(content)),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn what_doesnt_ends_in_an_ellipsis_within_the_room() {
        let content = "The Extraordinarily Long Title";
        for room in [40.0, 75.0, 100.0, 155.0, 290.0] {
            let shown = fit(content, room, width, boundary_in(content));
            assert!(shown.ends_with(MARK), "{room}: {shown}");
            assert!(width(&shown) <= room, "{room}: {shown}");
            let kept = shown.trim_end_matches(MARK);
            assert!(content.starts_with(kept), "{room}: {shown}");
        }
        // No space before the mark.
        assert_eq!(
            fit(content, 50.0, width, boundary_in(content)),
            "The\u{2026}"
        );
    }

    #[test]
    fn a_cut_never_splits_a_letter() {
        // Multi-byte letters: every cut is on a char edge.
        let content = "Ça ne s’arrête jamais, été comme hiver";
        for room in (10..400).step_by(7) {
            let shown = fit(content, room as f32, width, boundary_in(content));
            assert!(width(&shown) <= room as f32 || shown == MARK);
        }
        // A shaper that reports a cursor inside a char is pulled back to
        // the char's start.
        let shown = fit("ééééé", 35.0, width, |_| 3);
        assert_eq!(shown, "é\u{2026}");
    }

    #[test]
    fn too_little_room_shows_the_mark_alone() {
        let content = "Saltwater";
        assert_eq!(fit(content, 12.0, width, boundary_in(content)), MARK);
        assert_eq!(fit(content, 0.0, width, boundary_in(content)), MARK);
    }
}
