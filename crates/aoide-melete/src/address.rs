use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use reqwest::Url;

use crate::Error;

/// A server's base address, normalised from what a person typed.
///
/// Accepts `melete.example.com`, `192.168.1.10:8100` and
/// `https://example.com/melete/`. When no scheme is typed one is guessed: a
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

    /// A path the server handed out (`/api/v1/music/stream/…`) beneath the
    /// address, so a reverse proxy's prefix is kept. `None` for anything but a
    /// plain absolute path: another host (`//…`), a dot segment, a query.
    pub(crate) fn join_path(&self, path: &str) -> Option<Url> {
        let rest = path.strip_prefix('/')?;
        let segments: Vec<&str> = rest.split('/').collect();
        let plain = |segment: &&str| {
            !segment.is_empty()
                && *segment != "."
                && *segment != ".."
                && !segment.contains(['?', '#', '\\'])
        };
        segments
            .iter()
            .all(plain)
            .then(|| self.join(&segments, &[]))
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a typed host (with an optional port) looks like it is on the local
/// network: loopback, `.local` (mDNS), a single-label name like `melete`, or a
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
            ("192.168.1.10:8100", "http://192.168.1.10:8100"),
            ("10.0.0.5:8100", "http://10.0.0.5:8100"),
            ("172.16.3.1", "http://172.16.3.1"),
            ("169.254.10.2", "http://169.254.10.2"),
            ("127.0.0.1:8100", "http://127.0.0.1:8100"),
            ("melete.local:8100", "http://melete.local:8100"),
            ("localhost:8100", "http://localhost:8100"),
            ("melete", "http://melete"),
            ("[::1]:8100", "http://[::1]:8100"),
            ("[fd00::5]:8100", "http://[fd00::5]:8100"),
        ] {
            assert_eq!(parsed(input), expected, "{input}");
        }
    }

    #[test]
    fn a_bare_public_host_gets_https() {
        for (input, expected) in [
            ("music.example.net", "https://music.example.net"),
            ("melete.example.com:8100", "https://melete.example.com:8100"),
            ("example.com/melete", "https://example.com/melete"),
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
            parsed("https://melete.example.com"),
            "https://melete.example.com"
        );
        assert_eq!(parsed("https://192.168.1.10"), "https://192.168.1.10");
    }

    #[test]
    fn a_reverse_proxy_subpath_survives() {
        assert_eq!(
            parsed("https://example.com/melete/"),
            "https://example.com/melete"
        );
        assert_eq!(
            parsed("https://example.com/media/melete//"),
            "https://example.com/media/melete"
        );
    }

    #[test]
    fn surrounding_whitespace_is_forgiven() {
        assert_eq!(parsed("  melete.local  "), "http://melete.local");
    }

    #[test]
    fn host_case_doesnt_change_the_guess() {
        assert_eq!(parsed("MELETE.LOCAL"), "http://melete.local");
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
        let address = ServerAddress::parse("http://host:8100").unwrap();
        assert_eq!(
            address.join(&["api", "v1", "health"], &[]).as_str(),
            "http://host:8100/api/v1/health"
        );
    }

    #[test]
    fn paths_append_beneath_a_proxy_prefix_rather_than_replacing_it() {
        let address = ServerAddress::parse("https://example.com/melete").unwrap();
        assert_eq!(
            address.join(&["api", "v1", "health"], &[]).as_str(),
            "https://example.com/melete/api/v1/health"
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
        let address = ServerAddress::parse("http://host/melete").unwrap();
        let url = address.join(&["Items", "../../admin"], &[]);
        assert_eq!(url.path(), "/melete/Items/..%2F..%2Fadmin");
    }

    #[test]
    fn a_path_the_server_gives_keeps_the_proxy_prefix() {
        let address = ServerAddress::parse("https://example.com/melete").unwrap();
        assert_eq!(
            address
                .join_path("/api/v1/music/stream/abc")
                .unwrap()
                .as_str(),
            "https://example.com/melete/api/v1/music/stream/abc"
        );
    }

    #[test]
    fn a_path_the_server_gives_cant_leave_the_server() {
        let address = ServerAddress::parse("https://example.com").unwrap();
        for path in [
            "//elsewhere.example/x",
            "https://elsewhere.example/x",
            "api/v1/relative",
            "/api/../admin",
            "/api/./x",
            "/api/x?y=1",
            "/",
        ] {
            assert_eq!(address.join_path(path), None, "{path}");
        }
    }
}
