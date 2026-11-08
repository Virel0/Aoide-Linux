//! Pictures beside Search's "Ask for it" results (Melete 0.8.0): an album's
//! or song's cover, an artist's portrait, each a path on the server read
//! with the token like a cover, and kept in the artwork cache under a key
//! holding that path. A server with them off, or an older one, sends none,
//! and the result is drawn without one.
//!
//! A portrait's licence asks for its author and licence to be shown with
//! it: [`credit`] puts them in words, each part with the link it opens.

use aoide_melete::{CoverSize, PosterCredit};

use crate::artwork::ArtKey;

/// The item every poster is kept under; no album has it as an id.
const ITEM: &str = "poster";

/// The width asked of the server: a row's.
pub const SIZE: CoverSize = crate::artwork::ROW;

/// The artwork key for the picture at `path` (a result's `poster`).
pub fn key(path: &str) -> ArtKey {
    ArtKey::sized(ITEM, path, SIZE)
}

/// The server path a key from [`key`] stands for, and its width; `None`
/// for anything else.
pub fn path_of(key: &ArtKey) -> Option<(&str, CoverSize)> {
    (key.item == ITEM).then_some((key.tag.as_str(), key.size.unwrap_or(SIZE)))
}

/// A portrait's credit as its parts, "Photo: {author}" and the licence's
/// name, each with the `https` link it opens (the picture's page, the
/// licence's), if any; empty when the credit names neither.
pub fn credit(credit: &PosterCredit) -> Vec<(String, Option<String>)> {
    let source = link(credit.source_url.as_ref());
    let mut parts = Vec::new();
    if let Some(author) = named(credit.author.as_ref()) {
        parts.push((format!("Photo: {author}"), source.clone()));
    }
    if let Some(licence) = named(credit.licence.as_ref()) {
        let words = if parts.is_empty() {
            format!("Photo: {licence}")
        } else {
            licence.to_owned()
        };
        parts.push((words, link(credit.licence_url.as_ref())));
    }
    if parts.is_empty()
        && let Some(source) = source
    {
        parts.push(("Photo: Wikimedia Commons".to_owned(), Some(source)));
    }
    parts
}

/// A part of a credit with words in it, trimmed.
fn named(part: Option<&String>) -> Option<&str> {
    part.map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// A credit's link, if it is one the browser is given (`https`).
fn link(url: Option<&String>) -> Option<String> {
    named(url)
        .filter(|s| s.starts_with("https://"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The credit's words as one line, as drawn.
    fn credit_line(credit: &PosterCredit) -> String {
        let parts: Vec<String> = super::credit(credit).into_iter().map(|(w, _)| w).collect();
        parts.join(", ")
    }

    fn credit_of(author: Option<&str>, licence: Option<&str>) -> PosterCredit {
        PosterCredit {
            author: author.map(Into::into),
            licence: licence.map(Into::into),
            licence_url: Some("https://creativecommons.org/licenses/by-sa/4.0/".into()),
            source_url: Some("https://commons.wikimedia.org/wiki/File:Band.jpg".into()),
        }
    }

    #[test]
    fn a_credit_names_the_author_and_the_licence() {
        let full = credit_of(Some("A. Photographer"), Some("CC BY-SA 4.0"));
        assert_eq!(credit_line(&full), "Photo: A. Photographer, CC BY-SA 4.0");
        assert_eq!(
            credit(&full),
            [
                (
                    "Photo: A. Photographer".to_owned(),
                    Some("https://commons.wikimedia.org/wiki/File:Band.jpg".to_owned())
                ),
                (
                    "CC BY-SA 4.0".to_owned(),
                    Some("https://creativecommons.org/licenses/by-sa/4.0/".to_owned())
                ),
            ]
        );
        assert_eq!(credit_line(&credit_of(None, Some("CC0"))), "Photo: CC0");
        assert_eq!(
            credit_line(&credit_of(Some(" Someone "), None)),
            "Photo: Someone"
        );
    }

    #[test]
    fn a_credit_without_names_points_at_the_picture_or_says_nothing() {
        assert_eq!(
            credit(&credit_of(Some(""), None)),
            [(
                "Photo: Wikimedia Commons".to_owned(),
                Some("https://commons.wikimedia.org/wiki/File:Band.jpg".to_owned())
            )]
        );
        assert!(credit(&PosterCredit::default()).is_empty());
    }

    #[test]
    fn only_https_links_are_offered() {
        let plain = PosterCredit {
            author: Some("A".into()),
            licence: Some("CC BY 4.0".into()),
            licence_url: Some("http://creativecommons.org/licenses/by/4.0/".into()),
            source_url: Some("javascript:alert(1)".into()),
        };
        assert!(credit(&plain).iter().all(|(_, link)| link.is_none()));
        assert_eq!(credit_line(&plain), "Photo: A, CC BY 4.0");
    }

    #[test]
    fn a_poster_key_gives_back_its_path_and_a_cover_key_none() {
        let path = "/api/v1/music/release-groups/0b6b4ba0-d36f-47bd-b4ea-6a5b91842d29/cover";
        assert_eq!(path_of(&key(path)), Some((path, SIZE)));
        assert_eq!(path_of(&ArtKey::new("al1", aoide_melete::COVER_TAG)), None);
        assert_eq!(
            path_of(&crate::avatars::key("/api/v1/users/1/avatar")),
            None
        );
        let file = key(path).file_name();
        assert!(file.contains("0b6b4ba0d36f47bdb4ea6a5b91842d29"), "{file}");
        assert_ne!(file, key("/api/v1/music/artist-portraits/7").file_name());
    }
}
