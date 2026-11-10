//! The full player's ground: the playing song's cover, shrunk, blurred and
//! darkened off the UI thread, then drawn stretched across the window (the
//! stretching smooths it further), with its main colours for the moving
//! grounds. Made once per cover; the one before stays until the new one is
//! ready.

use iced::Task;
use iced::widget::image::Handle;
use image::imageops;

use crate::app::{App, Message};
use crate::artwork::ArtKey;
use crate::listening::Msg;

/// The blurred picture's longer side, in pixels.
const SIDE: u32 = 160;
/// How far the blur spreads, in those pixels.
const SIGMA: f32 = 9.0;
/// The brightest a pixel may be, as linear light: white words read on it at
/// 4.5:1 or better ((1 + 0.05) / (0.18 + 0.05) ≈ 4.6).
const CAP: f32 = 0.18;

/// A cover made into the ground.
#[derive(Debug, Clone)]
pub struct Made {
    /// Blurred and darkened.
    pub still: Handle,
    /// Its four main colours, as linear light, most of the cover first.
    pub palette: [[f32; 3]; 4],
}

impl App {
    /// Starts blurring the playing song's cover when it isn't the one the
    /// ground was made from, once the cover is here.
    pub(crate) fn reconcile_backdrop(&mut self) -> Task<Message> {
        let key = self
            .playback
            .current()
            .and_then(|(_, q)| q.meta.artwork.as_ref())
            .map(|(item, tag)| ArtKey::sized(item, tag, crate::artwork::ROW));
        let bytes = match key.as_ref().and_then(|key| self.artwork.get(key)) {
            Some(Handle::Bytes(_, bytes)) => Some(bytes.clone()),
            _ => None,
        };
        let Some(full) = self.listening.full.as_mut() else {
            return Task::none();
        };
        if full.backdrop_for == key {
            return Task::none();
        }
        let Some(key) = key else {
            full.backdrop_for = None;
            full.backdrop = None;
            return Task::none();
        };
        let Some(bytes) = bytes else {
            return Task::none();
        };
        full.backdrop_for = Some(key.clone());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let (width, height, pixels, palette) = blurred(&bytes)?;
                    Some(Made {
                        still: Handle::from_rgba(width, height, pixels),
                        palette,
                    })
                })
                .await
                .ok()
                .flatten()
            },
            move |made| Message::Listening(Msg::Backdrop(key, made)),
        )
    }

    /// A blurred cover is ready: it becomes the ground if it is still the
    /// playing song's.
    pub(crate) fn backdrop_made(&mut self, key: ArtKey, made: Option<Made>) -> Task<Message> {
        if let Some(full) = self.listening.full.as_mut()
            && full.backdrop_for.as_ref() == Some(&key)
        {
            full.backdrop = made;
        }
        Task::none()
    }
}

/// A blurred cover: its width, height, RGBA pixels, and main colours.
type Blurred = (u32, u32, Vec<u8>, [[f32; 3]; 4]);

/// `cover` (an encoded image) shrunk to `SIDE`, blurred and held under
/// `CAP`, with its main colours taken before darkening. `None` for what
/// isn't an image.
fn blurred(cover: &[u8]) -> Option<Blurred> {
    let small = image::load_from_memory(cover).ok()?.thumbnail(SIDE, SIDE);
    let mut soft = imageops::blur(&small.to_rgba8(), SIGMA);
    let light: Vec<[f32; 3]> = soft
        .pixels()
        .map(|p| [0, 1, 2].map(|i| to_linear(p.0[i])))
        .collect();
    let palette = super::palette::main_colours(&light);
    for pixel in soft.pixels_mut() {
        darken(&mut pixel.0);
    }
    Some((soft.width(), soft.height(), soft.into_raw(), palette))
}

/// A picture's main colours as sRGB, the most of it first, found as the
/// ground's are (for a page's or the window's colours where the server has
/// none); empty for what isn't a picture.
pub(crate) fn picture_palette(picture: &[u8]) -> Vec<[u8; 3]> {
    let Ok(picture) = image::load_from_memory(picture) else {
        return Vec::new();
    };
    let small = picture.thumbnail(SIDE / 4, SIDE / 4).to_rgb8();
    let light: Vec<[f32; 3]> = small
        .pixels()
        .map(|p| [0, 1, 2].map(|i| to_linear(p.0[i])))
        .collect();
    if light.is_empty() {
        return Vec::new();
    }
    super::palette::main_colours(&light)
        .iter()
        .map(|colour| colour.map(to_srgb))
        .collect()
}

/// Scales a pixel's light down to `CAP` when it is brighter, keeping its hue.
fn darken(rgba: &mut [u8; 4]) {
    let linear = [0, 1, 2].map(|i| to_linear(rgba[i]));
    let light = luminance(linear);
    if light <= CAP {
        return;
    }
    let scale = CAP / light;
    for (channel, value) in rgba.iter_mut().zip(linear) {
        *channel = to_srgb(value * scale);
    }
}

fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub(super) fn to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(linear: f32) -> u8 {
    let c = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    // In 0..=255 after the clamp, so the cast loses nothing.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, colour: [u8; 4]) -> Vec<u8> {
        let picture = image::RgbaImage::from_pixel(width, height, image::Rgba(colour));
        let mut bytes = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("a PNG is written");
        bytes.into_inner()
    }

    #[test]
    fn a_cover_comes_back_small_and_in_its_shape() {
        let (width, height, pixels, _) =
            blurred(&png(1000, 500, [40, 60, 90, 255])).expect("blurred");
        assert_eq!((width, height), (SIDE, SIDE / 2));
        assert_eq!(pixels.len(), (width * height * 4) as usize);
    }

    #[test]
    fn even_a_white_cover_is_dark_enough_for_white_words() {
        let (_, _, pixels, _) = blurred(&png(300, 300, [255, 255, 255, 255])).expect("blurred");
        for pixel in pixels.as_chunks::<4>().0 {
            let light = luminance([0, 1, 2].map(|i| to_linear(pixel[i])));
            // A byte's rounding, no more, over the cap.
            assert!(light <= CAP + 0.005, "{light}");
            assert!((1.05 / (light + 0.05)) >= 4.5);
        }
    }

    #[test]
    fn a_dark_cover_keeps_its_colour() {
        let mut pixel = [20, 40, 120, 255];
        darken(&mut pixel);
        assert_eq!(pixel, [20, 40, 120, 255]);
        // A bright red stays red, only dimmer.
        let mut red = [255, 40, 40, 255];
        darken(&mut red);
        assert!(red[0] < 255 && red[0] > red[1] * 3);
        assert_eq!(red[3], 255);
    }

    #[test]
    fn what_isnt_an_image_gives_no_ground() {
        assert!(blurred(b"not a picture").is_none());
        assert!(picture_palette(b"not a picture").is_empty());
    }

    #[test]
    fn a_pictures_colours_come_back_as_bytes() {
        let palette = picture_palette(&png(64, 64, [200, 30, 40, 255]));
        assert_eq!(palette.len(), 4);
        for colour in palette {
            assert!(
                colour
                    .iter()
                    .zip([200, 30, 40])
                    .all(|(a, b)| a.abs_diff(b) <= 2)
            );
        }
    }
}
