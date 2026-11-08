//! An artist's albums by kind, as Melete's web shows them: Albums, Singles &
//! EPs, Live, then Compilations, each kind there is under its heading; with
//! albums alone the page is one "Albums" grid, as it always was. A kind
//! comes from the album's MusicBrainz release group (Melete 0.8.0); an
//! album of no known type (an older server's, or one not matched) is told
//! by its track count, as the iPhone app does.

use aoide_melete::Item;

use crate::library::Tile;

/// The most tracks an album of unknown type has and is still a single or
/// an EP.
const SINGLE_TRACKS: i32 = 3;

/// What an album is, in the order an artist's page shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Albums,
    Singles,
    Live,
    Compilations,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Albums, Kind::Singles, Kind::Live, Kind::Compilations];

    /// Its heading.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Albums => "Albums",
            Kind::Singles => "Singles & EPs",
            Kind::Live => "Live",
            Kind::Compilations => "Compilations",
        }
    }
}

/// An album's kind. A known type wins: Live, then a compilation (by its tags
/// or its release group), then a Single or EP; any other type is an album.
/// With no type at all, three tracks or fewer is a single.
pub fn kind_of(album: &Item) -> Kind {
    let secondary = |name: &str| album.secondary_types.iter().any(|s| s == name);
    if secondary("Live") {
        return Kind::Live;
    }
    if album.compilation || secondary("Compilation") {
        return Kind::Compilations;
    }
    match album.primary_type.as_deref() {
        Some("Single" | "EP") => Kind::Singles,
        None if album.secondary_types.is_empty()
            && album.child_count.is_some_and(|n| n <= SINGLE_TRACKS) =>
        {
            Kind::Singles
        }
        _ => Kind::Albums,
    }
}

/// The albums as tiles under each kind that has any, in the kinds' order,
/// each kind keeping the albums' own order.
pub fn group(albums: &[Item]) -> Vec<(Kind, Vec<Tile>)> {
    Kind::ALL
        .into_iter()
        .map(|kind| {
            let tiles = albums
                .iter()
                .filter(|a| kind_of(a) == kind)
                .map(Tile::album)
                .collect::<Vec<_>>();
            (kind, tiles)
        })
        .filter(|(_, tiles)| !tiles.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(name: &str, primary: Option<&str>, secondary: &[&str], tracks: i32) -> Item {
        Item {
            id: name.into(),
            name: name.into(),
            kind: aoide_melete::ItemKind::Album,
            child_count: Some(tracks),
            primary_type: primary.map(Into::into),
            secondary_types: secondary.iter().map(|s| (*s).to_owned()).collect(),
            ..Item::default()
        }
    }

    #[test]
    fn each_kind_by_its_type() {
        assert_eq!(kind_of(&album("a", Some("Album"), &[], 10)), Kind::Albums);
        assert_eq!(kind_of(&album("s", Some("Single"), &[], 2)), Kind::Singles);
        assert_eq!(kind_of(&album("e", Some("EP"), &[], 5)), Kind::Singles);
        assert_eq!(
            kind_of(&album("l", Some("Album"), &["Live"], 12)),
            Kind::Live
        );
        assert_eq!(
            kind_of(&album("c", Some("Album"), &["Compilation"], 20)),
            Kind::Compilations
        );
        let tagged = Item {
            compilation: true,
            ..album("t", None, &[], 14)
        };
        assert_eq!(kind_of(&tagged), Kind::Compilations);
    }

    #[test]
    fn live_wins_over_compilation_and_compilation_over_single() {
        assert_eq!(
            kind_of(&album("lc", Some("Album"), &["Compilation", "Live"], 9)),
            Kind::Live
        );
        let live_tagged = Item {
            compilation: true,
            ..album("lt", Some("Album"), &["Live"], 9)
        };
        assert_eq!(kind_of(&live_tagged), Kind::Live);
        assert_eq!(
            kind_of(&album("cs", Some("Single"), &["Compilation"], 2)),
            Kind::Compilations
        );
        let single_tagged = Item {
            compilation: true,
            ..album("st", Some("EP"), &[], 4)
        };
        assert_eq!(kind_of(&single_tagged), Kind::Compilations);
    }

    #[test]
    fn an_unknown_type_is_told_by_its_track_count() {
        assert_eq!(kind_of(&album("u3", None, &[], 3)), Kind::Singles);
        assert_eq!(kind_of(&album("u4", None, &[], 4)), Kind::Albums);
        let unknown_length = Item {
            child_count: None,
            ..album("u", None, &[], 0)
        };
        assert_eq!(kind_of(&unknown_length), Kind::Albums);
        // A compilation of three songs stays a compilation.
        let short_compilation = Item {
            compilation: true,
            ..album("c3", None, &[], 3)
        };
        assert_eq!(kind_of(&short_compilation), Kind::Compilations);
        // A known type isn't second-guessed by its length.
        assert_eq!(kind_of(&album("a2", Some("Album"), &[], 2)), Kind::Albums);
        assert_eq!(kind_of(&album("o2", Some("Other"), &[], 2)), Kind::Albums);
        assert_eq!(
            kind_of(&album("sx", None, &["Soundtrack"], 2)),
            Kind::Albums,
            "a secondary type is a known type"
        );
    }

    #[test]
    fn kinds_come_in_order_and_empty_ones_are_left_out() {
        let albums = [
            album("comp", Some("Album"), &["Compilation"], 20),
            album("first", Some("Album"), &[], 10),
            album("ep", Some("EP"), &[], 5),
            album("second", None, &[], 11),
        ];
        let groups = group(&albums);
        let kinds: Vec<Kind> = groups.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, [Kind::Albums, Kind::Singles, Kind::Compilations]);
        let albums_shown: Vec<&str> = groups[0].1.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            albums_shown,
            ["first", "second"],
            "in the albums' own order"
        );
        assert_eq!(Kind::Singles.title(), "Singles & EPs");
    }

    #[test]
    fn albums_alone_are_one_group_as_the_page_was() {
        let groups = group(&[album("a", Some("Album"), &[], 10), album("b", None, &[], 9)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, Kind::Albums);
        assert_eq!(groups[0].1.len(), 2);
        assert!(group(&[]).is_empty());
        let singles = group(&[album("s", Some("Single"), &[], 1)]);
        assert_eq!(singles.len(), 1);
        assert_eq!(singles[0].0, Kind::Singles);
    }
}
