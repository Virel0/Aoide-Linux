use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use reqwest::Url;

use crate::Error;

/// A server's base address, normalised from what a person typed.
///
/// Accepts `jellyfin.example.com`, `192.168.1.10:8096` and
/// `https://example.com/jellyfin/`. When no scheme is typed one is guessed: a
/// LAN-looking host gets `http://` (home servers rarely have a certificate) and
/// anything else gets `https://`. A typed scheme always wins. A reverse-proxy
/// subpath is kept, and request paths are joined beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerAddress {
    url: Url,
}

impl ServerAddress {
    /// Normalises a typed address. Fails on anything that isn't an http(s) URL
    /// with a host.
    ///
    /// # Errors
    ///
    /// [`Error::MalformedAddress`], carrying `raw` as typed, for blank text, a
    /// scheme other than http or https, or no host.
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let malformed = || Error::MalformedAddress(raw.to_owned());
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(malformed());
        }

        let with_scheme = if trimmed.contains("://") {
            trimmed.to_owned()
        } else {
            let host = trimmed.split('/').next().unwrap_or(trimmed);
            let scheme = if is_local_host(host) { "http" } else { "https" };
            format!("{scheme}://{trimmed}")
        };

        // Trailing slashes stripped so joining paths stays predictable.
        let url = Url::parse(with_scheme.trim_end_matches('/')).map_err(|_| malformed())?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
            return Err(malformed());
        }
        Ok(Self { url })
    }

    /// The address as a URL.
    pub fn as_url(&self) -> &Url {
        &self.url
    }

    /// The address as text, without the trailing slash a bare origin would get.
    pub fn as_str(&self) -> &str {
        let s = self.url.as_str();
        if self.url.path() == "/" && self.url.query().is_none() && self.url.fragment().is_none() {
            s.strip_suffix('/').unwrap_or(s)
        } else {
            s
        }
    }

    /// The host, for messages.
    pub fn host(&self) -> &str {
        self.url.host_str().unwrap_or_default()
    }

    /// `segments` appended beneath the address's own path (never replacing it),
    /// each percent-encoded as one segment, then `query` appended.
    pub(crate) fn join(&self, segments: &[&str], query: &[(&str, String)]) -> Url {
        let mut url = self.url.clone();
        // Fails only for cannot-be-a-base URLs, which `parse` never produces.
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(segments);
        }
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        url
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a typed host (with an optional port) looks like it is on the local
/// network: loopback, `.local` (mDNS), a single-label name like `jellyfin`, or a
/// private, loopback or link-local IP.
fn is_local_host(host_and_port: &str) -> bool {
    if let Some(rest) = host_and_port.strip_prefix('[') {
        let host = rest.split(']').next().unwrap_or_default();
        return host.parse::<Ipv6Addr>().is_ok_and(|ip| {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        });
    }

    let host = host_and_port
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if host == "localhost" || host.ends_with(".local") || !host.contains('.') {
        return true;
    }
    host.parse::<Ipv4Addr>()
        .is_ok_and(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(raw: &str) -> String {
        ServerAddress::parse(raw)
            .unwrap_or_else(|e| panic!("{raw}: {e}"))
            .to_string()
    }

    #[test]
    fn a_bare_lan_host_gets_http() {
        for (input, expected) in [
            ("192.168.1.10:8096", "http://192.168.1.10:8096"),
            ("10.0.0.5:8096", "http://10.0.0.5:8096"),
            ("172.16.3.1", "http://172.16.3.1"),
            ("169.254.10.2", "http://169.254.10.2"),
            ("127.0.0.1:8096", "http://127.0.0.1:8096"),
            ("jellyfin.local:8096", "http://jellyfin.local:8096"),
            ("localhost:8096", "http://localhost:8096"),
            ("jellyfin", "http://jellyfin"),
            ("[::1]:8096", "http://[::1]:8096"),
            ("[fd00::5]:8096", "http://[fd00::5]:8096"),
        ] {
            assert_eq!(parsed(input), expected, "{input}");
        }
    }

    #[test]
    fn a_bare_public_host_gets_https() {
        for (input, expected) in [
            ("music.example.net", "https://music.example.net"),
            (
                "jellyfin.example.com:8096",
                "https://jellyfin.example.com:8096",
            ),
            ("example.com/jellyfin", "https://example.com/jellyfin"),
            ("172.32.0.1", "https://172.32.0.1"),
            ("203.0.113.7:8920", "https://203.0.113.7:8920"),
            ("[2001:db8::1]:8920", "https://[2001:db8::1]:8920"),
        ] {
            assert_eq!(parsed(input), expected, "{input}");
        }
    }

    #[test]
    fn a_typed_scheme_always_wins_over_the_guess() {
        assert_eq!(
            parsed("http://music.example.net"),
            "http://music.example.net"
        );
        assert_eq!(
            parsed("https://jellyfin.example.com"),
            "https://jellyfin.example.com"
        );
        assert_eq!(parsed("https://192.168.1.10"), "https://192.168.1.10");
    }

    #[test]
    fn a_reverse_proxy_subpath_survives() {
        assert_eq!(
            parsed("https://example.com/jellyfin/"),
            "https://example.com/jellyfin"
        );
        assert_eq!(
            parsed("https://example.com/media/jellyfin//"),
            "https://example.com/media/jellyfin"
        );
    }

    #[test]
    fn surrounding_whitespace_is_forgiven() {
        assert_eq!(parsed("  jellyfin.local  "), "http://jellyfin.local");
    }

    #[test]
    fn host_case_doesnt_change_the_guess() {
        assert_eq!(parsed("JELLYFIN.LOCAL"), "http://jellyfin.local");
    }

    #[test]
    fn garbage_is_rejected() {
        for raw in ["", "   ", "ftp://example.com", "http://", "://nope"] {
            assert_eq!(
                ServerAddress::parse(raw),
                Err(Error::MalformedAddress(raw.to_owned())),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn paths_append_to_a_bare_origin() {
        let address = ServerAddress::parse("http://host:8096").unwrap();
        assert_eq!(
            address.join(&["System", "Info", "Public"], &[]).as_str(),
            "http://host:8096/System/Info/Public"
        );
    }

    #[test]
    fn paths_append_beneath_a_proxy_prefix_rather_than_replacing_it() {
        let address = ServerAddress::parse("https://example.com/jellyfin").unwrap();
        assert_eq!(
            address.join(&["System", "Info", "Public"], &[]).as_str(),
            "https://example.com/jellyfin/System/Info/Public"
        );
    }

    #[test]
    fn query_items_are_attached_and_encoded() {
        let address = ServerAddress::parse("http://host").unwrap();
        let url = address.join(
            &["Items"],
            &[("userId", "abc".into()), ("searchTerm", "a&b c".into())],
        );
        assert_eq!(
            url.as_str(),
            "http://host/Items?userId=abc&searchTerm=a%26b+c"
        );
    }

    #[test]
    fn a_segment_cant_escape_its_place_in_the_path() {
        let address = ServerAddress::parse("http://host/jellyfin").unwrap();
        let url = address.join(&["Items", "../../admin"], &[]);
        assert_eq!(url.path(), "/jellyfin/Items/..%2F..%2Fadmin");
    }
}
