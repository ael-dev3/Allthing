// SPDX-License-Identifier: AGPL-3.0-or-later
//! An account's website: the URL its profile sets (Farcaster user data of
//! type URL), made into a link a profile can show safely.
//!
//! The account signs whatever it likes there, up to 256 bytes, and the
//! profile renders it as a link to another site, so only a plain web address
//! passes ([`Website::parse`]): http or https (a bare `example.com` gets
//! `https://`), with a host that has a dot or is an IP address. Nothing that
//! could run, mislead or point elsewhere than it says gets through: no other
//! scheme (`javascript:`, `data:`, `mailto:`, `geo:`, `file:`), no name and
//! password before the host, no spaces, control characters or invisible
//! ones (zero-width, bidirectional overrides). A host in another script is
//! linked and shown as its ASCII form (`xn--…`, as IDNA writes it), which
//! can't pass for a look-alike of a familiar name.

use serde::{Deserialize, Serialize};

/// The longest value read, in bytes: Farcaster's limit for a profile's URL.
pub const MAX_BYTES: usize = 256;

/// The longest `href` [`Website::checked`] takes: a value of [`MAX_BYTES`]
/// with the `https://` and punycode a build may add to it.
const MAX_HREF_BYTES: usize = 1024;

/// A profile's link to the account's website.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Website {
    /// The address the link opens: `http` or `https`, the host in lowercase
    /// ASCII, then the port, path, query and fragment as the account wrote
    /// them.
    pub href: String,
    /// What the link says: the host and path, without the scheme, a leading
    /// `www.` or a trailing slash (`danromero.org`).
    pub label: String,
}

impl Website {
    /// The website a profile's URL names, or `None` unless it is a plain web
    /// address (see the module's notes). Whitespace around it is trimmed; an
    /// empty value is an account that set none, or cleared it.
    pub fn parse(value: &str) -> Option<Website> {
        read(value, MAX_BYTES, any_host)
    }

    /// This website as a payload brings it, checked again where a profile
    /// shows it: its `href` read as [`Website::parse`] reads a profile's URL,
    /// and its label made anew from that. So a payload from anywhere but
    /// Allthing's own build of it (an old copy, a stand-in) can neither link
    /// to anything but a web address nor say it goes somewhere it doesn't.
    /// A build writes a host in another script in ASCII, so here a host must
    /// be ASCII already, which keeps the other scripts' tables and punycode
    /// out of the page's code.
    pub fn checked(&self) -> Option<Website> {
        read(&self.href, MAX_HREF_BYTES, ascii_host)
    }
}

/// A website from `value`, at most `max` bytes once trimmed, its host read
/// by `host`: its link with the scheme and host made plain, and its label.
fn read(value: &str, max: usize, host: fn(&str) -> Option<String>) -> Option<Website> {
    let value = value.trim();
    if value.is_empty() || value.len() > max || !value.chars().all(allowed) {
        return None;
    }
    let (scheme, rest) = match value.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("https".to_owned(), value),
    };
    if scheme != "http" && scheme != "https" {
        return None;
    }
    // What follows the host. A scheme-like `name:` without `//`
    // (`javascript:`, `mailto:`) is read as a host and a port here, and
    // refused as neither.
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let (host, port) = host_and_port(authority, host)?;
    let port = port.map(|port| format!(":{port}")).unwrap_or_default();
    let path = tail.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    let shown = match host.strip_prefix("www.") {
        Some(rest) if rest.contains('.') => rest,
        _ => host.as_str(),
    };
    let label = format!("{shown}{port}{path}");
    Some(Website { href: format!("{scheme}://{host}{port}{tail}"), label })
}

/// Whether `c` may stand in a website's address: not a space, a control
/// character or an invisible one, and not one a URL can't hold as written.
fn allowed(c: char) -> bool {
    !(c.is_whitespace() || c.is_control() || invisible(c) || matches!(c, '<' | '>' | '"' | '\\' | '`'))
}

/// Characters that show nothing, or change how what's around them shows:
/// zero-width spaces and joiners, bidirectional marks, embeddings and
/// overrides (which can turn a label around), soft hyphens, fillers,
/// variation selectors and tags.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{34f}'
            | '\u{61c}'
            | '\u{115f}'
            | '\u{1160}'
            | '\u{17b4}'
            | '\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fff8}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// The host of `authority`, in lowercase ASCII (a name read by `host`), and
/// its port. Refused: a name and password before the host (`user@`), which
/// can make a link read as one site and open another, and anything that
/// isn't a host.
fn host_and_port(authority: &str, host: fn(&str) -> Option<String>) -> Option<(String, Option<u16>)> {
    if authority.contains('@') {
        return None;
    }
    let (host, after) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (address, after) = bracketed.split_once(']')?;
            let address = address.to_ascii_lowercase();
            (ipv6(&address).then(|| format!("[{address}]"))?, after)
        }
        None => {
            let (name, after) = authority.split_at(authority.find(':').unwrap_or(authority.len()));
            (host(name)?, after)
        }
    };
    let port = match after {
        "" => None,
        after => Some(port(after.strip_prefix(':')?)?),
    };
    Some((host, port))
}

fn port(digits: &str) -> Option<u16> {
    if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|port| *port > 0)
}

/// A domain name with at least one dot, in lowercase ASCII, or an IPv4
/// address: a name whose last label is a number is one, as browsers read
/// it, and must be a valid one.
fn ascii_host(name: &str) -> Option<String> {
    if !name.is_ascii() || !name.contains('.') {
        return None;
    }
    let name = name.to_ascii_lowercase();
    let numeric =
        name.rsplit('.').next().is_some_and(|last| !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()));
    if numeric {
        return ipv4(&name).then_some(name);
    }
    let labels_valid = name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    });
    (labels_valid && name.len() <= 253).then_some(name)
}

/// [`ascii_host`] for a name as an account wrote it: a label in another
/// script, letters and digits of any script and hyphens, in its punycode.
/// Not symbols, nor the full stops of other scripts, which browsers read as
/// dots.
fn any_host(name: &str) -> Option<String> {
    let name = name.to_lowercase();
    let mut ascii = Vec::new();
    for label in name.split('.') {
        if label.is_ascii() {
            ascii.push(label.to_owned());
        } else if !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        {
            ascii.push(format!("xn--{}", punycode(label)));
        } else {
            return None;
        }
    }
    ascii_host(&ascii.join("."))
}

/// Four decimal numbers from 0 to 255, without leading zeros.
fn ipv4(address: &str) -> bool {
    let parts: Vec<&str> = address.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
                && part.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// Eight groups of up to four hex digits, `::` standing for one or more
/// groups of zeros, the last two perhaps written as an IPv4 address.
fn ipv6(address: &str) -> bool {
    // The groups in `part`, or `None` if one is malformed.
    fn groups(part: &str, last: bool) -> Option<usize> {
        if part.is_empty() {
            return Some(0);
        }
        let pieces: Vec<&str> = part.split(':').collect();
        let mut count = 0;
        for (i, piece) in pieces.iter().enumerate() {
            if last && i + 1 == pieces.len() && piece.contains('.') {
                count += ipv4(piece).then_some(2)?;
            } else if (1..=4).contains(&piece.len()) && piece.bytes().all(|b| b.is_ascii_hexdigit()) {
                count += 1;
            } else {
                return None;
            }
        }
        Some(count)
    }
    match address.split_once("::") {
        Some((head, tail)) => {
            !tail.contains("::")
                && groups(head, false).zip(groups(tail, true)).is_some_and(|(head, tail)| head + tail <= 7)
        }
        None => groups(address, true) == Some(8),
    }
}

/// RFC 3492's encoding of a label with characters beyond ASCII, which IDNA
/// writes after `xn--`: the label's ASCII characters, then where each other
/// character goes, as base-36 digits.
fn punycode(label: &str) -> String {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    let digit = |d: u32| char::from(if d < 26 { b'a' + d as u8 } else { b'0' + (d - 26) as u8 });
    let adapt = |delta: u32, points: u32, first: bool| {
        let mut delta = delta / if first { DAMP } else { 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > (BASE - T_MIN) * T_MAX / 2 {
            delta /= BASE - T_MIN;
            k += BASE;
        }
        k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
    };
    let points: Vec<u32> = label.chars().map(u32::from).collect();
    let mut out: String = label.chars().filter(char::is_ascii).collect();
    let basic = out.len() as u32;
    if basic > 0 {
        out.push('-');
    }
    // A label is at most 256 bytes here, so none of these can overflow.
    let (mut n, mut delta, mut bias, mut handled) = (128_u32, 0_u32, 72_u32, basic);
    while (handled as usize) < points.len() {
        let next = points.iter().copied().filter(|&c| c >= n).min().unwrap_or(n);
        delta += (next - n) * (handled + 1);
        n = next;
        for &c in &points {
            if c < n {
                delta += 1;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        T_MIN
                    } else if k >= bias + T_MAX {
                        T_MAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(value: &str) -> Option<(String, String)> {
        Website::parse(value).map(|w| (w.href, w.label))
    }

    fn pair(href: &str, label: &str) -> Option<(String, String)> {
        Some((href.to_owned(), label.to_owned()))
    }

    #[test]
    fn web_addresses_link_as_written_and_read_without_their_scheme() {
        assert_eq!(site("https://danromero.org"), pair("https://danromero.org", "danromero.org"));
        assert_eq!(site("https://allthing.social/"), pair("https://allthing.social/", "allthing.social"));
        // http stays http: the site may not serve https.
        assert_eq!(site("http://example.com/blog/"), pair("http://example.com/blog/", "example.com/blog"));
        assert_eq!(site("https://www.senpi.ai"), pair("https://www.senpi.ai", "senpi.ai"));
        // The query and fragment go with the link, not its label.
        assert_eq!(
            site("https://example.com/a/b?ref=farcaster#top"),
            pair("https://example.com/a/b?ref=farcaster#top", "example.com/a/b")
        );
        assert_eq!(site("https://example.com:8443/x"), pair("https://example.com:8443/x", "example.com:8443/x"));
        // The scheme and host in lowercase; the path as written.
        assert_eq!(site("HTTPS://WWW.Example.COM/Read"), pair("https://www.example.com/Read", "example.com/Read"));
        // "www." goes only where a name is left after it.
        assert_eq!(site("https://www.com"), pair("https://www.com", "www.com"));
        assert_eq!(site("https://my_site.example.com"), pair("https://my_site.example.com", "my_site.example.com"));
    }

    #[test]
    fn a_bare_host_gets_https() {
        assert_eq!(site("example.com"), pair("https://example.com", "example.com"));
        assert_eq!(site("www.example.com/about/"), pair("https://www.example.com/about/", "example.com/about"));
        assert_eq!(site("example.com:8080/x"), pair("https://example.com:8080/x", "example.com:8080/x"));
        assert_eq!(site("  nf.td/varun \n"), pair("https://nf.td/varun", "nf.td/varun"));
    }

    #[test]
    fn nothing_is_a_website_when_nothing_is_set() {
        for value in ["", "   ", "\n\t"] {
            assert_eq!(site(value), None, "{value:?}");
        }
    }

    #[test]
    fn other_schemes_never_link() {
        for value in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(document.cookie) ",
            "javascript://example.com/%0Aalert(1)",
            "javascript:void(0)//example.com",
            "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "geo:34.05,-118.24",
            "mailto:dan@example.com",
            "tel:5551234567",
            "sms:5551234567",
            "file:///etc/passwd",
            "ftp://example.com/file",
            "ws://example.com/socket",
            "chrome://settings",
            "about:blank",
            "blob:https://example.com/0f5d",
            "intent://scan/#Intent;scheme=zxing;end",
            "https:example.com",
            "//example.com",
            "/relative/path",
        ] {
            assert_eq!(site(value), None, "{value:?}");
        }
    }

    #[test]
    fn spaces_controls_and_invisible_characters_are_refused() {
        for value in [
            "https://exa mple.com",
            "https://example.com/a page",
            "https://example.com/\nalert(1)",
            "java\nscript:alert(1)",
            "https://example.com\r\nSet-Cookie: x=1",
            "https://example.com/\u{0}",
            "\u{7f}https://example.com",
            "https://example.com/a\u{a0}b",
            // Zero-width and bidirectional characters can make a label read
            // as a different site.
            "https://exam\u{200b}ple.com",
            "https://example.com/\u{202e}moc.live",
            "https://\u{2066}example.com",
            "https://example.com\u{feff}",
            "https://exam\u{ad}ple.com",
            "https://example.com/\"onmouseover=\"alert(1)",
            "https://example.com/<script>",
            "https://example.com\\@evil.com",
            "https://example.com/`",
        ] {
            assert_eq!(site(value), None, "{value:?}");
        }
    }

    #[test]
    fn a_name_and_password_before_the_host_are_refused() {
        for value in [
            "https://user:pass@example.com",
            "https://danromero.org@evil.com",
            "https://danromero.org%2F@evil.com",
            "mailto:me@example.com",
            "me@example.com",
        ] {
            assert_eq!(site(value), None, "{value:?}");
        }
        // After the host, an "@" is the path's or the fragment's: the label
        // names the host the link opens.
        assert_eq!(site("https://evil.com#@danromero.org"), pair("https://evil.com#@danromero.org", "evil.com"));
        assert_eq!(
            site("https://evil.com/@danromero.org"),
            pair("https://evil.com/@danromero.org", "evil.com/@danromero.org")
        );
    }

    #[test]
    fn a_host_needs_a_dot_or_to_be_an_ip_address() {
        for value in [
            "localhost",
            "https://localhost:3000",
            "https://intranet/",
            "https://",
            "https:///path",
            "https://.com",
            "https://example..com",
            "https://example.com.",
            "https://-example.com",
            "https://example-.com",
            "https://exa%6dple.com",
            "https://exa+mple.com",
            "https://example.123",
            "https://256.1.1.1",
            "https://1.2.3",
            "https://01.2.3.4",
            "https://0x7f.0.0.1",
            "https://example.com:",
            "https://example.com:0",
            "https://example.com:65536",
            "https://example.com:443:80",
            "https://example.com:http",
            "https://[::1",
            "https://[example.com]",
            "https://[1:2:3:4:5:6:7:8:9]",
            "https://[1::2::3]",
            "https://[12345::1]",
            "https://[::1]x",
        ] {
            assert_eq!(site(value), None, "{value:?}");
        }
        assert_eq!(site("http://192.168.1.10:8080/"), pair("http://192.168.1.10:8080/", "192.168.1.10:8080"));
        assert_eq!(site("https://[2001:DB8::1]/x"), pair("https://[2001:db8::1]/x", "[2001:db8::1]/x"));
        assert_eq!(
            site("https://[::ffff:192.0.2.1]:8443"),
            pair("https://[::ffff:192.0.2.1]:8443", "[::ffff:192.0.2.1]:8443")
        );
        assert_eq!(site("https://[1:2:3:4:5:6:7:8]"), pair("https://[1:2:3:4:5:6:7:8]", "[1:2:3:4:5:6:7:8]"));
    }

    #[test]
    fn values_longer_than_a_profile_can_hold_are_refused() {
        let path = "a".repeat(MAX_BYTES - "https://example.com/".len());
        let longest = format!("https://example.com/{path}");
        assert_eq!(longest.len(), MAX_BYTES);
        assert_eq!(site(&longest), pair(&longest, &format!("example.com/{path}")));
        assert_eq!(site(&format!("{longest}a")), None);
        assert_eq!(site(&format!("https://{}.com", "a".repeat(64))), None);
    }

    #[test]
    fn hosts_in_other_scripts_link_and_read_as_ascii() {
        assert_eq!(site("https://bücher.de/"), pair("https://xn--bcher-kva.de/", "xn--bcher-kva.de"));
        assert_eq!(site("MÜNCHEN.de"), pair("https://xn--mnchen-3ya.de", "xn--mnchen-3ya.de"));
        // A Cyrillic "а" in a familiar name shows as what it is.
        assert_eq!(site("https://\u{430}pple.com"), pair("https://xn--pple-43d.com", "xn--pple-43d.com"));
        // A path in another script stays as written; the browser encodes it.
        assert_eq!(site("https://example.com/über"), pair("https://example.com/über", "example.com/über"));
        // Symbols and other scripts' full stops aren't letters of a name.
        for value in ["https://i\u{2764}.ws", "https://example\u{3002}com", "https://example\u{ff0e}com"] {
            assert_eq!(site(value), None, "{value:?}");
        }
    }

    #[test]
    fn punycode_matches_rfc_3492() {
        assert_eq!(punycode("bücher"), "bcher-kva");
        assert_eq!(punycode("münchen"), "mnchen-3ya");
        // RFC 3492 §7.1 (B), (L) and (A).
        assert_eq!(punycode("他们为什么不说中文"), "ihqwcrb4cv8a8dqg056pqjye");
        assert_eq!(punycode("3年B組金八先生"), "3B-ww4c5e180e575a65lsy2b");
        assert_eq!(
            punycode(
                "\u{644}\u{64a}\u{647}\u{645}\u{627}\u{628}\u{62a}\u{643}\u{644}\u{645}\u{648}\u{634}\u{639}\u{631}\u{628}\u{64a}\u{61f}"
            ),
            "egbpdaj6bu4bxfgehfvwxn"
        );
    }

    #[test]
    fn a_payloads_website_is_checked_again_where_it_shows() {
        // What a build made reads back the same, the longest value too, whose
        // link is longer for the https:// it gained.
        let longest = format!("example.com/{}", "a".repeat(MAX_BYTES - "example.com/".len()));
        for value in ["https://danromero.org", "bücher.de/über/", "http://[::1]:8080/x?y#z", longest.as_str()] {
            let website = Website::parse(value).unwrap();
            assert_eq!(website.checked(), Some(website.clone()), "{value}");
        }
        let given = |href: &str, label: &str| Website { href: href.into(), label: label.into() };
        // Not a web address: no link, whatever its label says.
        for href in ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "data:text/html,hi", "", "https://user@evil.com"] {
            assert_eq!(given(href, "danromero.org").checked(), None, "{href:?}");
        }
        // A label names where the link goes.
        assert_eq!(
            given("https://evil.example/login", "danromero.org").checked(),
            Some(given("https://evil.example/login", "evil.example/login"))
        );
        // A build writes a host in another script in ASCII: a host that
        // isn't comes from something else.
        assert_eq!(given("https://\u{430}pple.com", "apple.com").checked(), None);
        assert_eq!(given("https://xn--pple-43d.com", "apple.com").checked().unwrap().label, "xn--pple-43d.com");
    }

    #[test]
    fn websites_serialize_as_href_and_label() {
        let website = Website::parse("danromero.org").unwrap();
        let json = serde_json::to_string(&website).unwrap();
        assert_eq!(json, r#"{"href":"https://danromero.org","label":"danromero.org"}"#);
        assert_eq!(serde_json::from_str::<Website>(&json).unwrap(), website);
    }
}
