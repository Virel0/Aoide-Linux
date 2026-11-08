//! What a collection's page says about its filter and its songs.

use aoide_melete::Collection;

/// "Jazz, Soul · 1955–1969 · Songs you like".
pub fn summary(collection: &Collection) -> String {
    let mut parts = Vec::new();
    if !collection.genres.is_empty() {
        parts.push(collection.genres.join(", "));
    }
    match (collection.year_from, collection.year_to) {
        (Some(from), Some(to)) if from == to => parts.push(from.to_string()),
        (Some(from), Some(to)) => parts.push(format!("{from}–{to}")),
        (Some(from), None) => parts.push(format!("From {from}")),
        (None, Some(to)) => parts.push(format!("Up to {to}")),
        (None, None) => {}
    }
    if collection.liked {
        parts.push("Songs you like".to_owned());
    }
    if parts.is_empty() {
        "Every song".to_owned()
    } else {
        parts.join(" · ")
    }
}

/// Said under the list when the filter finds more songs than one page
/// lists; `None` when they are all there.
pub fn more(listed: usize, total: u32) -> Option<String> {
    let total = usize::try_from(total).unwrap_or(usize::MAX);
    (total > listed)
        .then(|| format!("The first {listed} of {total} songs. Play and Shuffle play these."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(genres: &[&str], years: (Option<i32>, Option<i32>), liked: bool) -> Collection {
        Collection {
            id: 1,
            name: "Evenings".into(),
            genres: genres.iter().map(|g| (*g).to_owned()).collect(),
            year_from: years.0,
            year_to: years.1,
            liked,
            created_at: 0,
        }
    }

    #[test]
    fn a_summary_names_each_part_of_the_filter() {
        assert_eq!(
            summary(&collection(
                &["Jazz", "Soul"],
                (Some(1955), Some(1969)),
                true
            )),
            "Jazz, Soul · 1955–1969 · Songs you like"
        );
        assert_eq!(
            summary(&collection(&[], (Some(1990), None), false)),
            "From 1990"
        );
        assert_eq!(
            summary(&collection(&[], (None, Some(1979)), false)),
            "Up to 1979"
        );
        assert_eq!(
            summary(&collection(&["Folk"], (Some(2001), Some(2001)), false)),
            "Folk · 2001"
        );
        assert_eq!(summary(&collection(&[], (None, None), false)), "Every song");
    }

    #[test]
    fn more_is_said_only_past_one_page() {
        assert_eq!(more(12, 12), None);
        assert_eq!(more(0, 0), None);
        assert_eq!(
            more(500, 1234).as_deref(),
            Some("The first 500 of 1234 songs. Play and Shuffle play these.")
        );
    }
}
