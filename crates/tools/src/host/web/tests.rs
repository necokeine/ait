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
        "2200::1",
        "2d00::1",
        "3000::1",
        "::ffff:10.0.0.1",
        "::10.0.0.1",
    ] {
        assert!(!public_ip(address.parse().unwrap()), "{address}");
    }
    for address in [
        "1.1.1.1",
        "2001:4860:4860::8888",
        "2003::1",
        "2400::1",
        "2606:4700:4700::1111",
        "2800::1",
        "2a00::1",
        "2c00::1",
    ] {
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
    let text = html_text("<style>secret</style><h1>Hello</h1><script>bad()</script><p>world</p>");
    assert_eq!(text, "Hello world");
    let (text, truncated) = bounded_text("é".repeat(TEXT_BYTES), false);
    assert!(truncated);
    assert!(text.len() <= TEXT_BYTES);
}
