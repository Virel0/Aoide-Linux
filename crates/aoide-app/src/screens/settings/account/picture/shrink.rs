//! A chosen file made into one the server takes: a JPEG, PNG or WebP picture
//! of at most 5 MB. One that is already is sent as it is; anything else the
//! app can read is made smaller and saved again as a JPEG.

use aoide_melete::{AVATAR_MAX_BYTES, PictureType};
use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;

/// The longest sides tried, largest first, until the JPEG fits.
const SIDES: [u32; 4] = [2048, 1536, 1024, 512];
/// The JPEG quality a smaller copy is saved at.
const QUALITY: u8 = 88;

/// `bytes` as the server will take them, and their type.
///
/// # Errors
///
/// What to tell the person, for a file that isn't a picture the app can read.
pub fn to_fit(bytes: Vec<u8>) -> Result<(Vec<u8>, PictureType), String> {
    if let Some(kind) = PictureType::of(&bytes)
        && bytes.len() <= AVATAR_MAX_BYTES
    {
        return Ok((bytes, kind));
    }
    let picture = image::load_from_memory(&bytes).map_err(|_| {
        "That file isn't a picture Aoide can read. Choose a JPEG, PNG or WebP picture.".to_owned()
    })?;
    for side in SIDES {
        let jpeg = smaller(&picture, side)?;
        if jpeg.len() <= AVATAR_MAX_BYTES {
            return Ok((jpeg, PictureType::Jpeg));
        }
    }
    Err("That picture couldn't be made small enough for your server. Choose a smaller one.".into())
}

/// The picture at most `side` across, as a JPEG.
fn smaller(picture: &DynamicImage, side: u32) -> Result<Vec<u8>, String> {
    let fitted = if picture.width() > side || picture.height() > side {
        picture.thumbnail(side, side)
    } else {
        picture.clone()
    };
    let rgb = DynamicImage::ImageRgb8(fitted.to_rgb8());
    let mut jpeg = Vec::new();
    rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, QUALITY))
        .map_err(|e| format!("That picture couldn't be made smaller: {e}"))?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};

    fn png(width: u32, height: u32) -> Vec<u8> {
        // Noise, so PNG can't squeeze it far.
        let picture = RgbImage::from_fn(width, height, |x, y| {
            let n = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503);
            Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        picture.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn a_picture_that_fits_is_sent_as_it_is() {
        let bytes = png(16, 16);
        let (sent, kind) = to_fit(bytes.clone()).unwrap();
        assert_eq!((sent, kind), (bytes, PictureType::Png));
    }

    #[test]
    fn a_picture_over_the_limit_is_made_a_smaller_jpeg() {
        let bytes = png(1600, 1200);
        assert!(bytes.len() > AVATAR_MAX_BYTES, "{}", bytes.len());
        let (sent, kind) = to_fit(bytes).unwrap();
        assert_eq!(kind, PictureType::Jpeg);
        assert!(sent.len() <= AVATAR_MAX_BYTES);
        let back = image::load_from_memory(&sent).unwrap();
        assert!(back.width() <= 2048 && back.height() <= 2048);
    }

    #[test]
    fn a_file_that_isnt_a_picture_says_so() {
        let refused = to_fit(b"not a picture".to_vec()).unwrap_err();
        assert!(refused.contains("isn't a picture"), "{refused}");
    }
}
