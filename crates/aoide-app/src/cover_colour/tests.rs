//! Every derived colour, for a spread of covers, in both modes, against what
//! it is drawn with; and which origin's colours a page takes.

use iced::Color;
use iced::theme::palette::deviate;

use super::derive::Derived;
use super::maths::{Hsv, contrast};
use super::*;

/// Covers with a colour: saturated, light, dark, pastel and muted.
pub(crate) const COLOURFUL: [(&str, [u8; 3]); 12] = [
    ("red", [220, 20, 30]),
    ("yellow", [250, 220, 0]),
    ("blue", [20, 60, 230]),
    ("green", [30, 180, 60]),
    ("teal", [0, 150, 150]),
    ("purple", [120, 30, 200]),
    ("orange", [250, 120, 10]),
    ("pink pastel", [250, 200, 210]),
    ("blue pastel", [200, 225, 250]),
    ("brown", [120, 70, 30]),
    ("deep navy", [10, 20, 60]),
    ("olive", [128, 128, 40]),
];

/// Covers with no colour to speak of.
const COLOURLESS: [(&str, [u8; 3]); 4] = [
    ("near black", [12, 10, 14]),
    ("near white", [248, 246, 250]),
    ("grey", [128, 128, 130]),
    ("warm grey", [150, 140, 130]),
];

/// Each colourful cover's colours in each mode, with that mode's grounds.
fn derived() -> Vec<(String, Derived, Surfaces)> {
    let mut all = Vec::new();
    for (name, bytes) in COLOURFUL {
        let look = Look::of(&[bytes]).unwrap_or_else(|| panic!("{name} has a colour"));
        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            let surfaces = crate::theme::artwork::surfaces(dark);
            all.push((format!("{name}, {mode}"), look.in_mode(dark), surfaces));
        }
    }
    all
}

/// The wash's strengths, top to foot.
fn strengths() -> impl Iterator<Item = f32> {
    (0..=16).map(|i| WASH_TOP * (1.0 - i as f32 / 16.0))
}

#[test]
fn a_grey_cover_leaves_the_app_as_it_is() {
    for (name, bytes) in COLOURLESS {
        assert!(Look::of(&[bytes]).is_none(), "{name}");
        assert!(
            Look::of(&[bytes, [0, 0, 0], [255, 255, 255]]).is_none(),
            "{name}"
        );
    }
}

#[test]
fn the_pages_words_read_on_the_wash_all_the_way_down() {
    for (name, derived, surfaces) in derived() {
        for strength in strengths() {
            let ground = derived.wash_over(surfaces.page, strength);
            let text = contrast(surfaces.text, ground);
            assert!(text >= 4.5, "{name} text at {strength}: {text}");
            let secondary = contrast(surfaces.secondary, ground);
            assert!(
                secondary >= 4.5,
                "{name} secondary at {strength}: {secondary}"
            );
        }
    }
}

#[test]
fn the_shells_words_read_on_its_tint() {
    for (name, derived, surfaces) in derived() {
        for ground in &surfaces.shell {
            let tinted = derived.tinted(*ground, surfaces.dark);
            assert_ne!(tinted, *ground, "{name}: the tint shows");
            let text = contrast(surfaces.text, tinted);
            assert!(text >= 4.5, "{name} text: {text}");
            let secondary = contrast(surfaces.secondary, tinted);
            assert!(secondary >= 4.5, "{name} secondary: {secondary}");
        }
    }
}

#[test]
fn the_accent_reads_as_text_on_every_ground() {
    for (name, derived, surfaces) in derived() {
        for (i, ground) in derived.accent_grounds(&surfaces).iter().enumerate() {
            let ratio = contrast(derived.accent, *ground);
            assert!(ratio >= 4.5, "{name} ground {i} {ground:?}: {ratio}");
        }
    }
}

#[test]
fn a_filled_buttons_label_reads_resting_and_hovered() {
    for (name, derived, _) in derived() {
        let resting = contrast(derived.label, derived.accent);
        assert!(resting >= 4.5, "{name} resting: {resting}");
        let hovered = contrast(derived.label, deviate(derived.accent, 0.1));
        assert!(hovered >= 4.5, "{name} hovered: {hovered}");
        assert!(derived.label == Color::WHITE || derived.label == Color::BLACK);
    }
}

/// The colours stay the cover's: its hue, where they have one left.
#[test]
fn the_colours_keep_the_covers_hue() {
    let hue_of = |c: Color| {
        let bytes = [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8);
        Hsv::of(bytes)
    };
    for (name, bytes) in COLOURFUL {
        let hue = Hsv::of(bytes).h;
        let look = Look::of(&[bytes]).expect("a colour");
        for derived in [look.light, look.dark] {
            for (part, colour) in [
                ("wash", derived.wash),
                ("tint", derived.tint),
                ("accent", derived.accent),
            ] {
                let found = hue_of(colour);
                if found.s < 0.1 || found.v < 0.1 {
                    continue;
                }
                let apart = (found.h - hue).abs();
                let apart = apart.min(1.0 - apart);
                assert!(apart < 0.03, "{name} {part}: {} against {hue}", found.h);
            }
        }
    }
}

#[test]
fn the_wash_is_strong_enough_to_see() {
    for (name, derived, surfaces) in derived() {
        let top = derived.wash_over(surfaces.page, WASH_TOP);
        let ratio = contrast(top, surfaces.page);
        assert!(ratio >= 1.15, "{name}: {ratio}");
    }
}

fn album(id: &str) -> Origin {
    Origin::Album(id.into())
}

fn picture(id: &str) -> Origin {
    Origin::Picture(crate::artwork::ArtKey::new(id, "cover"))
}

const RED: [u8; 3] = [220, 20, 30];
const BLUE: [u8; 3] = [20, 60, 230];

#[test]
fn the_first_origin_with_colours_wins() {
    let mut colours = CoverColours::default();
    let origins = [album("1"), picture("1")];
    assert_eq!(colours.next_to_ask(&origins), Some(&origins[0]));
    colours.asking(album("1"));
    assert_eq!(colours.next_to_ask(&origins), None, "waiting for the album");
    assert_eq!(colours.look(&origins), None);
    colours.arrived(album("1"), vec![RED]);
    assert_eq!(colours.look(&origins), Look::of(&[RED]));
    assert_eq!(colours.next_to_ask(&origins), None);
    assert_eq!(colours.album_palette("1"), Some([RED].as_slice()));
}

#[test]
fn an_album_with_no_colours_passes_to_its_picture() {
    let mut colours = CoverColours::default();
    let origins = [album("1"), picture("1")];
    colours.arrived(album("1"), Vec::new());
    assert_eq!(colours.album_palette("1"), None);
    assert_eq!(colours.next_to_ask(&origins), Some(&origins[1]));
    assert_eq!(colours.look(&origins), None, "the picture isn't read yet");
    colours.arrived(picture("1"), vec![BLUE]);
    assert_eq!(colours.look(&origins), Look::of(&[BLUE]));
}

#[test]
fn a_grey_album_stays_without_a_look() {
    let mut colours = CoverColours::default();
    let origins = [album("1"), picture("1")];
    colours.arrived(album("1"), vec![[128, 128, 128]]);
    assert_eq!(colours.look(&origins), None);
    assert_eq!(
        colours.next_to_ask(&origins),
        None,
        "its colours are its colours"
    );
}

#[test]
fn what_a_page_brought_is_kept_but_never_over_an_answer() {
    let mut colours = CoverColours::default();
    colours.knows("1", &[RED]);
    assert_eq!(colours.look(&[album("1")]), Look::of(&[RED]));
    colours.knows("1", &[BLUE]);
    assert_eq!(colours.look(&[album("1")]), Look::of(&[RED]));
    // Asked for already: the page's answer is as good.
    colours.asking(album("2"));
    colours.knows("2", &[BLUE]);
    assert_eq!(colours.look(&[album("2")]), Look::of(&[BLUE]));
}

#[test]
fn an_album_cover_is_tried_as_the_album_first() {
    let cover = crate::artwork::ArtKey::sized("9", aoide_melete::COVER_TAG, crate::artwork::ROW);
    assert_eq!(
        origins_of(&cover),
        [Origin::Album("9".into()), Origin::Picture(cover.clone())]
    );
    let chosen = crate::artwork::ArtKey::new("aoidecover", "abc123");
    assert_eq!(origins_of(&chosen), [Origin::Picture(chosen.clone())]);
}
