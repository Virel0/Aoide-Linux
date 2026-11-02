//! Keeping only what the typed words say, as the iPhone app does
//! (`MusicQuery+Grounding.swift`).
//!
//! A model asked to turn "Karma Police Radiohead" into filters may skip the
//! name and answer with a genre it associates with the band; run, that lists
//! every song in the genre, which reads as random results for a song that
//! simply isn't in the library. So a genre is kept only when the phrase
//! names it, years only when it has a number or a decade, "liked only" only
//! when it says so, and a search term only when it's in the phrase.

use aoide_melete::MusicFilters;

const DECADE_WORDS: [&str; 11] = [
    "twenties",
    "thirties",
    "forties",
    "fifties",
    "sixties",
    "seventies",
    "eighties",
    "nineties",
    "noughties",
    "decade",
    "century",
];

const LIKE_WORDS: [&str; 9] = [
    "favourite",
    "favourites",
    "favorite",
    "favorites",
    "loved",
    "liked",
    "likes",
    "starred",
    "hearted",
];

/// Lower-cased words without accents or punctuation.
fn words(text: &str) -> Vec<String> {
    aoide_store::normalise(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The words of a phrase, and whether it says a name by whole words.
struct Phrase {
    words: Vec<String>,
    padded: String,
}

impl Phrase {
    fn new(text: &str) -> Self {
        let words = words(text);
        let padded = format!(" {} ", words.join(" "));
        Self { words, padded }
    }

    /// "Hip-Hop" is said by "hip hop" and by "hiphop"; "Rock" isn't said by
    /// "rockstar".
    fn says(&self, name: &str) -> bool {
        let wanted = words(name);
        !wanted.is_empty()
            && (self.padded.contains(&format!(" {} ", wanted.join(" ")))
                || self.words.contains(&wanted.concat()))
    }

    fn has_any(&self, of: &[&str]) -> bool {
        self.words.iter().any(|word| of.contains(&word.as_str()))
    }
}

/// Only the parts of `filters` that `phrase`'s own words say.
pub fn grounded(filters: &MusicFilters, phrase: &str) -> MusicFilters {
    let said = Phrase::new(phrase);
    let dated = phrase.chars().any(char::is_numeric) || said.has_any(&DECADE_WORDS);
    MusicFilters {
        search_term: filters.search_term.clone().filter(|term| said.says(term)),
        genres: filters
            .genres
            .iter()
            .filter(|genre| said.says(genre))
            .cloned()
            .collect(),
        from_year: filters.from_year.filter(|_| dated),
        to_year: filters.to_year.filter(|_| dated),
        liked_only: filters.liked_only && said.has_any(&LIKE_WORDS),
        ordering: filters.ordering,
    }
}

#[cfg(test)]
mod tests {
    use aoide_melete::MusicOrdering;

    use super::*;

    fn genres(names: &[&str]) -> MusicFilters {
        MusicFilters {
            genres: names.iter().map(|n| (*n).to_owned()).collect(),
            ..MusicFilters::default()
        }
    }

    #[test]
    fn a_song_name_is_not_a_genre() {
        let guessed = genres(&["Alternative"]);
        assert!(grounded(&guessed, "Karma Police Radiohead").is_empty());
    }

    #[test]
    fn what_the_words_say_is_kept_whole() {
        let described = MusicFilters {
            from_year: Some(1990),
            to_year: Some(1999),
            ordering: MusicOrdering::NewestFirst,
            ..genres(&["Rock"])
        };
        assert_eq!(
            grounded(&described, "upbeat rock from the nineties"),
            described
        );
        let year = MusicFilters {
            from_year: Some(1994),
            to_year: Some(1994),
            ..MusicFilters::default()
        };
        assert_eq!(grounded(&year, "songs from 1994"), year);
    }

    #[test]
    fn years_need_a_number_or_a_decade() {
        let guessed = MusicFilters {
            from_year: Some(1990),
            to_year: Some(1999),
            ..genres(&["Grunge"])
        };
        let kept = grounded(&guessed, "grunge like it used to be");
        assert_eq!((kept.from_year, kept.to_year), (None, None));
        assert_eq!(kept.genres, ["Grunge"]);
    }

    #[test]
    fn genres_match_by_whole_words_accents_and_punctuation_aside() {
        let hip_hop = genres(&["Hip-Hop"]);
        assert_eq!(grounded(&hip_hop, "old school hip hop").genres, ["Hip-Hop"]);
        assert_eq!(grounded(&hip_hop, "some hiphop please").genres, ["Hip-Hop"]);
        assert_eq!(
            grounded(&genres(&["Électronique"]), "electronique").genres,
            ["Électronique"]
        );
        assert!(
            grounded(&genres(&["Rock"]), "rockstar vibes")
                .genres
                .is_empty(),
            "a word, not a substring"
        );
    }

    #[test]
    fn a_term_or_likes_the_words_dont_say_are_dropped() {
        let invented = MusicFilters {
            search_term: Some("Paranoid Android".into()),
            liked_only: true,
            ..MusicFilters::default()
        };
        assert!(grounded(&invented, "something sad by radiohead").is_empty());
        let liked = MusicFilters {
            liked_only: true,
            ..MusicFilters::default()
        };
        assert!(grounded(&liked, "my favourite chill songs").liked_only);
        let named = MusicFilters {
            search_term: Some("Radiohead".into()),
            ..MusicFilters::default()
        };
        assert_eq!(
            grounded(&named, "something sad by radiohead").search_term,
            named.search_term
        );
    }
}
