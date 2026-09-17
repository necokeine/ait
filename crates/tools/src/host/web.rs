use super::{HostTools, MAX_BYTES, failed, string};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::ToolInvocation;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::OnceLock,
    time::Duration,
};
use url::{Host, Url};

const FETCH_BYTES: usize = 262_144;
const TEXT_BYTES: usize = 48_000;
const SEARCH_RESULTS: usize = 8;

fn web_failure(retryable: bool) -> DomainError {
    if retryable {
        DomainError::transient(
            ErrorCode::ToolExecutionFailed,
            "web request failed or timed out",
        )
    } else {
        failed()
    }
}

fn safe_url(value: &str) -> Result<Url, DomainError> {
    if value.len() > 2_048 {
        return Err(failed());
    }
    let url = Url::parse(value).map_err(|_| failed())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(failed());
    }
    let forbidden = match url.host().ok_or_else(failed)? {
        Host::Domain(host) => {
            host.eq_ignore_ascii_case("localhost")
                || host.rsplit_once('.').is_some_and(|(_, suffix)| {
                    suffix.eq_ignore_ascii_case("localhost") || suffix.eq_ignore_ascii_case("local")
                })
        }
        Host::Ipv4(address) => !public_ip(IpAddr::V4(address)),
        Host::Ipv6(address) => !public_ip(IpAddr::V6(address)),
    };
    if forbidden {
        return Err(failed());
    }
    Ok(url)
}

fn ipv4_prefix(address: Ipv4Addr, network: Ipv4Addr, bits: u32) -> bool {
    let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
    u32::from(address) & mask == u32::from(network) & mask
}

fn ipv6_prefix(address: Ipv6Addr, network: Ipv6Addr, bits: u32) -> bool {
    let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
    u128::from(address) & mask == u128::from(network) & mask
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    const DENIED: &[(Ipv4Addr, u32)] = &[
        (Ipv4Addr::UNSPECIFIED, 8),
        (Ipv4Addr::new(10, 0, 0, 0), 8),
        (Ipv4Addr::new(100, 64, 0, 0), 10),
        (Ipv4Addr::new(127, 0, 0, 0), 8),
        (Ipv4Addr::new(169, 254, 0, 0), 16),
        (Ipv4Addr::new(172, 16, 0, 0), 12),
        (Ipv4Addr::new(192, 0, 0, 0), 24),
        (Ipv4Addr::new(192, 0, 2, 0), 24),
        (Ipv4Addr::new(192, 88, 99, 0), 24),
        (Ipv4Addr::new(192, 168, 0, 0), 16),
        (Ipv4Addr::new(198, 18, 0, 0), 15),
        (Ipv4Addr::new(198, 51, 100, 0), 24),
        (Ipv4Addr::new(203, 0, 113, 0), 24),
        (Ipv4Addr::new(224, 0, 0, 0), 4),
        (Ipv4Addr::new(240, 0, 0, 0), 4),
    ];
    !DENIED
        .iter()
        .any(|(network, bits)| ipv4_prefix(address, *network, *bits))
}

fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => public_ipv4(address),
        IpAddr::V6(address) => {
            const GLOBAL_UNICAST: Ipv6Addr = Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0);
            const DENIED: &[(Ipv6Addr, u32)] = &[
                (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
                (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
                (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
                (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
            ];
            // Cover both IPv4-mapped and deprecated IPv4-compatible forms.
            if let Some(embedded) = address.to_ipv4() {
                return public_ipv4(embedded);
            }
            // Only native global unicast is eligible. Deny special-use ranges
            // within it, including transition mechanisms that can encode a
            // private IPv4 destination.
            ipv6_prefix(address, GLOBAL_UNICAST, 3)
                && !DENIED
                    .iter()
                    .any(|(network, bits)| ipv6_prefix(address, *network, *bits))
        }
    }
}

fn public_addresses(addresses: &[SocketAddr]) -> bool {
    !addresses.is_empty() && addresses.iter().all(|address| public_ip(address.ip()))
}

fn redirect_target(base: &Url, location: &str) -> Result<Url, DomainError> {
    safe_url(base.join(location).map_err(|_| failed())?.as_str())
}

async fn resolve(url: &Url) -> Result<(String, Vec<SocketAddr>), DomainError> {
    let host = url.host_str().ok_or_else(failed)?.to_owned();
    let port = url.port_or_known_default().ok_or_else(failed)?;
    let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| web_failure(true))?
        .take(16)
        .collect();
    if !public_addresses(&addresses) {
        return Err(web_failure(false));
    }
    Ok((host, addresses))
}

fn client(host: &str, addresses: &[SocketAddr]) -> Result<reqwest::Client, DomainError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, addresses)
        .user_agent("Ait/0.0.3 web tools")
        .build()
        .map_err(|_| web_failure(false))
}

async fn fetch(mut url: Url) -> Result<(Url, u16, String, Vec<u8>, bool), DomainError> {
    let mut redirects = 0_u8;
    let mut response = loop {
        let (host, addresses) = resolve(&url).await?;
        let response = client(&host, &addresses)?
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,application/json,text/plain;q=0.9,*/*;q=0.1",
            )
            .send()
            .await
            .map_err(|_| web_failure(true))?;
        if !response.status().is_redirection() {
            break response;
        }
        if redirects >= 5 {
            return Err(web_failure(false));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| web_failure(false))?;
        url = redirect_target(&url, location)?;
        redirects = redirects.saturating_add(1);
    };
    let final_url = safe_url(response.url().as_str())?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(web_failure(response.status().is_server_error()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_ascii_lowercase();
    if !content_type.starts_with("text/")
        && !content_type.contains("json")
        && !content_type.contains("xml")
    {
        return Err(web_failure(false));
    }
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|_| web_failure(true))? {
        let remaining = FETCH_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == FETCH_BYTES {
            truncated = response
                .content_length()
                .is_none_or(|length| length > FETCH_BYTES as u64);
            break;
        }
    }
    Ok((final_url, status, content_type, bytes, truncated))
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn html_text(value: &str) -> String {
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static SPACE: OnceLock<Regex> = OnceLock::new();
    let value = SCRIPT
        .get_or_init(|| {
            Regex::new("(?is)<(?:script|style|noscript)[^>]*>.*?</(?:script|style|noscript)>")
                .unwrap()
        })
        .replace_all(value, " ");
    let value = TAG
        .get_or_init(|| Regex::new("(?s)<[^>]+>").unwrap())
        .replace_all(&value, " ");
    let value = decode_entities(&value);
    SPACE
        .get_or_init(|| Regex::new(r"[\t\r\n ]+").unwrap())
        .replace_all(&value, " ")
        .trim()
        .to_owned()
}

fn bounded_text(mut text: String, already_truncated: bool) -> (String, bool) {
    if text.len() <= TEXT_BYTES {
        return (text, already_truncated);
    }
    let mut boundary = TEXT_BYTES;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    (text, true)
}

fn result_url(href: &str) -> Option<String> {
    let decoded = decode_entities(href);
    let decoded = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else {
        decoded
    };
    let url = Url::parse(&decoded).ok()?;
    if url
        .host_str()
        .is_some_and(|host| host.ends_with("duckduckgo.com"))
    {
        return url
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.into_owned());
    }
    Some(decoded)
}

fn parse_search(query: &str, html: &str) -> Vec<Value> {
    static RESULT: OnceLock<Regex> = OnceLock::new();
    static SNIPPET: OnceLock<Regex> = OnceLock::new();
    let pattern = RESULT.get_or_init(|| {
        Regex::new(
            r#"(?is)<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>(.*?)</a>(.*?)(?:<a[^>]*class="[^"]*result__a|$)"#,
        )
        .unwrap()
    });
    let snippet = SNIPPET.get_or_init(|| {
        Regex::new(r#"(?is)class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</(?:a|div)>"#).unwrap()
    });
    pattern
        .captures_iter(html)
        .filter_map(|capture| {
            let url = result_url(capture.get(1)?.as_str())?;
            safe_url(&url).ok()?;
            let title = html_text(capture.get(2)?.as_str());
            let body = capture.get(3).map_or("", |value| value.as_str());
            let summary = snippet
                .captures(body)
                .and_then(|capture| capture.get(1))
                .map_or_else(String::new, |value| html_text(value.as_str()));
            Some(json!({"query":query,"title":title,"url":url,"snippet":summary}))
        })
        .take(SEARCH_RESULTS)
        .collect()
}

impl HostTools {
    pub(super) async fn web(&self, request: &ToolInvocation) -> Result<Value, DomainError> {
        self.check(request)?;
        match request.tool_name.as_str() {
            "webfetch" => {
                let url = safe_url(string(&request.arguments, "url")?)?;
                let (url, status, content_type, bytes, truncated) = fetch(url).await?;
                self.check(request)?;
                let raw = String::from_utf8_lossy(&bytes);
                let text = if content_type.contains("html") {
                    html_text(&raw)
                } else {
                    raw.into_owned()
                };
                let (text, truncated) = bounded_text(text, truncated);
                Ok(json!({
                    "url": url.as_str(),
                    "status": status,
                    "content_type": content_type,
                    "text": text,
                    "truncated": truncated,
                    "external_untrusted": true,
                }))
            }
            "websearch" => {
                let queries = request.arguments["queries"].as_array().ok_or_else(failed)?;
                let mut results = Vec::new();
                for query in queries {
                    self.check(request)?;
                    let query = query.as_str().ok_or_else(failed)?;
                    let mut url = Url::parse("https://html.duckduckgo.com/html/").unwrap();
                    url.query_pairs_mut().append_pair("q", query);
                    let (_, _, _, bytes, _) = fetch(url).await?;
                    results.extend(parse_search(query, &String::from_utf8_lossy(&bytes)));
                }
                results.truncate(queries.len().saturating_mul(SEARCH_RESULTS));
                let output = json!({"results":results,"external_untrusted":true});
                if output.to_string().len() > MAX_BYTES {
                    return Err(failed());
                }
                Ok(output)
            }
            _ => Err(failed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_reject_credentials_and_private_hosts() {
        for value in [
            "file:///etc/passwd",
            "http://localhost/a",
            "http://127.0.0.1/a",
            "http://[::1]/a",
            "http://[::127.0.0.1]/a",
            "https://user:password@example.com/",
        ] {
            assert!(safe_url(value).is_err(), "{value}");
        }
        assert_eq!(safe_url("https://example.com/a").unwrap().scheme(), "https");
    }

    #[test]
    fn ip_policy_rejects_special_ipv6_and_embedded_private_destinations() {
        for address in [
            "64:ff9b::10.0.0.1",
            "64:ff9b:1::a00:1",
            "100::1",
            "2001:2::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "3fff::1",
            "5f00::1",
            "::ffff:10.0.0.1",
            "::10.0.0.1",
        ] {
            assert!(!public_ip(address.parse().unwrap()), "{address}");
        }
        for address in ["1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn dns_and_redirect_checks_reject_any_private_target() {
        let addresses = [
            "93.184.216.34:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ];
        assert!(!public_addresses(&addresses));
        assert!(
            redirect_target(
                &Url::parse("https://example.com/start").unwrap(),
                "http://169.254.169.254/latest/meta-data"
            )
            .is_err()
        );
    }

    #[test]
    fn search_parser_returns_structured_external_urls() {
        let html = r#"<div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdoc">Example &amp; docs</a><a class="result__snippet">Useful <b>answer</b>.</a></div>"#;
        let results = parse_search("ait", html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["url"], "https://example.com/doc");
        assert_eq!(results[0]["title"], "Example & docs");
        assert_eq!(results[0]["snippet"], "Useful answer .");
    }

    #[test]
    fn html_conversion_is_bounded_and_removes_active_content() {
        let text =
            html_text("<style>secret</style><h1>Hello</h1><script>bad()</script><p>world</p>");
        assert_eq!(text, "Hello world");
        let (text, truncated) = bounded_text("é".repeat(TEXT_BYTES), false);
        assert!(truncated);
        assert!(text.len() <= TEXT_BYTES);
    }
}
