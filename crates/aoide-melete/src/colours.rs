//! An album cover's colours, as Melete works them out so every app matches.
//! Only the palette is read: the cover's main colours, for what lies around
//! a square cover on a screen of another shape, such as the full player.

use serde::Deserialize;

/// `CoverColours`, trimmed to its palette.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WireColours {
    /// Up to five `#rrggbb`, the most of the cover first. Empty from a
    /// server before palettes, and for colours an older server worked out
    /// until they're worked out again.
    #[serde(default)]
    palette: Vec<String>,
}

impl WireColours {
    /// The palette as sRGB bytes, in the server's order, leaving out
    /// anything that isn't `#rrggbb`.
    pub(crate) fn palette(&self) -> Vec<[u8; 3]> {
        self.palette.iter().filter_map(|hex| rgb(hex)).collect()
    }
}

/// `#rrggbb` as bytes.
fn rgb(hex: &str) -> Option<[u8; 3]> {
    let digits = hex.strip_prefix('#').filter(|d| d.len() == 6)?;
    let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_palette_is_read_in_order_and_bad_colours_left_out() {
        let colours: WireColours = serde_json::from_value(json!({
            "background": "#00ffff",
            "palette": ["#02fd00", "#FE0100", "red", "#12345", "#0002fd"],
            "text1": "#0002fd"
        }))
        .unwrap();
        assert_eq!(
            colours.palette(),
            [[0x02, 0xfd, 0x00], [0xfe, 0x01, 0x00], [0x00, 0x02, 0xfd]]
        );
    }

    #[test]
    fn an_album_carries_its_palette_and_an_older_server_none() {
        let mut json: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/album.json")).unwrap();
        json["colours"] = json!({"background": "#666666", "palette": ["#040404", "#ae0002"]});
        let album = crate::Item::from(crate::model::WireAlbum::deserialize(&json).unwrap());
        assert_eq!(album.palette, [[4, 4, 4], [0xae, 0, 2]]);
        json.as_object_mut().unwrap().remove("colours");
        let older = crate::Item::from(crate::model::WireAlbum::deserialize(&json).unwrap());
        assert!(older.palette.is_empty());
    }

    #[test]
    fn colours_without_a_palette_have_none() {
        let colours: WireColours =
            serde_json::from_value(json!({"background": "#666666"})).unwrap();
        assert!(colours.palette().is_empty());
        assert_eq!(rgb("#ééééé"), None);
    }
}
