use reqwest::Url;

pub fn parse_public_origin(value: &str) -> Option<String> {
    let value = value.trim();
    // URL parsing normalizes dot segments, backslashes, and some whitespace.
    // Reject those inputs before parsing rather than accepting a hidden path.
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || value.contains(['\\', '@'])
    {
        return None;
    }
    let (_, remainder) = value.split_once("://")?;
    if let Some(path_start) = remainder.find('/') {
        if &remainder[path_start..] != "/" {
            return None;
        }
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_canonicalize_case_default_ports_and_ipv6() {
        for (input, expected) in [
            (
                "https://Doughbox.Example.COM/",
                "https://doughbox.example.com",
            ),
            (
                "HTTPS://Doughbox.Example.COM:443",
                "https://doughbox.example.com",
            ),
            ("HTTP://LOCALHOST:80/", "http://localhost"),
            (" http://127.0.0.1:8084/ ", "http://127.0.0.1:8084"),
            (
                "https://doughbox.example.com:8443",
                "https://doughbox.example.com:8443",
            ),
            ("http://[::1]:8084/", "http://[::1]:8084"),
            (
                "HTTPS://[2001:0DB8:0:0:0:0:0:1]:443/",
                "https://[2001:db8::1]",
            ),
        ] {
            assert_eq!(parse_public_origin(input).as_deref(), Some(expected));
        }
    }

    #[test]
    fn origins_reject_credentials_paths_queries_fragments_and_non_http_urls() {
        for input in [
            "",
            "doughbox.example.com",
            "//doughbox.example.com",
            "https:doughbox.example.com",
            "https:///doughbox.example.com",
            "javascript:alert(1)",
            "ftp://doughbox.example.com",
            "https://",
            "https://user:pass@doughbox.example.com",
            "https://user@doughbox.example.com",
            "https://@doughbox.example.com",
            "https://doughbox.example.com/trading",
            "https://doughbox.example.com//",
            "https://doughbox.example.com/a/..",
            "https://doughbox.example.com/%2e/",
            "https://doughbox.example.com?",
            "https://doughbox.example.com?next=https://evil.example",
            "https://doughbox.example.com/#",
            "https://doughbox.example.com#fragment",
            "https://dough box.example.com",
            "https://doughbox.\nexample.com",
            "https://doughbox.example.com\\trading",
            "http://::1:8084",
            "https://doughbox.example.com:invalid",
        ] {
            assert!(
                parse_public_origin(input).is_none(),
                "accepted invalid origin: {input}"
            );
        }
    }
}
