//! Which URLs may leave the machine in something other people see.

use reqwest::Url;

/// Query parameters that carry a credential (Jellyfin- and Emby-style, generic
/// OAuth). Compared case-insensitively.
const CREDENTIAL_PARAMS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "access_token",
    "auth",
    "key",
    "x-emby-token",
    "x-mediabrowser-token",
    "signature",
    "sig",
];

/// Query parameters that only exist to track who clicked. A link carrying
/// one is not shown at all rather than shown with it.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "twclid",
    "ttclid",
    "igshid",
    "igsh",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "si",
    "ref",
    "ref_src",
    "ref_url",
    "spm",
    "trk",
    "s_cid",
    "wt_mc",
    "vero_id",
    "oly_anon_id",
    "oly_enc_id",
];

fn is_tracking(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_") || TRACKING_PARAMS.contains(&name.as_str())
}

/// `url` if it is `https://`, names a host, and carries no user info and no
/// credential in its query. Otherwise `None`.
pub(crate) fn public_https(url: &str) -> Option<Url> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let has_credential = parsed
        .query_pairs()
        .any(|(name, _)| CREDENTIAL_PARAMS.contains(&name.to_ascii_lowercase().as_str()));
    if has_credential { None } else { Some(parsed) }
}

/// `url` if [`public_https`] accepts it and it adds no tracking parameters.
pub(crate) fn clean_link(url: &str) -> Option<String> {
    let parsed = public_https(url)?;
    if parsed.query_pairs().any(|(name, _)| is_tracking(&name)) {
        return None;
    }
    Some(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_https_urls_pass() {
        let url = "https://coverartarchive.org/release/abc/front-500.jpg";
        assert_eq!(public_https(url).unwrap().as_str(), url);
        assert!(clean_link("https://musicbrainz.org/recording/abc").is_some());
    }

    #[test]
    fn other_schemes_are_refused() {
        for url in [
            "http://example.org/a.jpg",
            "file:///tmp/a.jpg",
            "ftp://example.org/a",
            "not a url",
            "",
        ] {
            assert!(public_https(url).is_none(), "{url}");
        }
    }

    #[test]
    fn credentials_are_refused() {
        for url in [
            "https://user:pass@example.org/a.jpg",
            "https://user@example.org/a.jpg",
            "https://example.org/Items/1/Images/Primary?api_key=abc",
            "https://example.org/a?API_KEY=abc",
            "https://example.org/a?x=1&ApiKey=abc",
            "https://example.org/a?X-Emby-Token=abc",
            "https://example.org/a?access_token=abc",
            "https://example.org/a?api%5Fkey=abc",
        ] {
            assert!(public_https(url).is_none(), "{url}");
        }
    }

    #[test]
    fn tracking_parameters_drop_the_link() {
        for url in [
            "https://example.org/a?utm_source=aoide",
            "https://example.org/a?UTM_Campaign=x",
            "https://open.example.org/track/1?si=abcdef",
            "https://example.org/a?fbclid=1",
            "https://example.org/a?q=1&gclid=2",
        ] {
            assert!(clean_link(url).is_none(), "{url}");
        }
    }

    #[test]
    fn ordinary_query_parameters_are_kept() {
        let url = "https://example.org/search?q=song&page=2";
        assert_eq!(clean_link(url).as_deref(), Some(url));
    }
}
