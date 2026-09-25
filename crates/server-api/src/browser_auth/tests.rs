use super::*;

const ORIGIN: &str = "http://localhost:8081";

fn headers(ticket: &str, origin: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("origin", origin.parse().unwrap());
    headers.insert(
        "sec-websocket-protocol",
        format!("{TICKET_PROTOCOL}{ticket}").parse().unwrap(),
    );
    headers
}

#[test]
fn only_explicit_loopback_origins_are_configurable() {
    for origin in [
        ORIGIN,
        "http://localhost",
        "http://127.0.0.1:1234",
        "http://[::1]:8081",
    ] {
        assert!(validate_browser_origin(origin).is_ok(), "{origin}");
    }
    for origin in [
        "null",
        "*",
        "https://localhost:8081",
        "http://evil.test",
        "http://localhost:0",
        "http://localhost:80",
        "http://localhost:99999",
        "http://localhost/",
        "http://user@localhost",
        "http://localhost?token=secret",
        "http://localhost#fragment",
    ] {
        assert!(validate_browser_origin(origin).is_err(), "{origin}");
    }
    let auth = BrowserAuth::new(vec![ORIGIN.to_owned()]).unwrap();
    assert!(auth.permits(ORIGIN));
    assert!(!auth.permits("http://localhost:8082"));
    assert!(BrowserAuth::new(vec!["*".to_owned()]).is_err());
}

#[test]
fn tickets_are_single_use_bound_to_origin_and_redacted() {
    let auth = BrowserAuth::default();
    let ticket = auth.issue(ORIGIN).unwrap();
    assert!(!format!("{auth:?}").contains(&ticket));
    assert!(auth.consume(&headers(&ticket, ORIGIN)).is_ok());
    assert!(auth.consume(&headers(&ticket, ORIGIN)).is_err());
    let ticket = auth.issue(ORIGIN).unwrap();
    assert!(
        auth.consume(&headers(&ticket, "http://localhost:8082"))
            .is_err()
    );
    assert!(auth.consume(&headers(&ticket, ORIGIN)).is_err());
    let ticket = auth.issue(ORIGIN).unwrap();
    let mut absent = headers(&ticket, ORIGIN);
    absent.remove("origin");
    assert!(auth.consume(&absent).is_err());
    let mut duplicate = headers(&ticket, ORIGIN);
    duplicate.append(
        "sec-websocket-protocol",
        "ait.ticket.another".parse().unwrap(),
    );
    assert!(auth.consume(&duplicate).is_err());
    assert!(auth.consume(&headers("unknown", ORIGIN)).is_err());
}

#[test]
fn tickets_expire_and_outstanding_credentials_are_bounded() {
    let auth = BrowserAuth::default();
    let ticket = auth.issue(ORIGIN).unwrap();
    auth.tickets
        .lock()
        .unwrap()
        .get_mut(&ticket)
        .unwrap()
        .expires = Instant::now();
    assert!(auth.consume(&headers(&ticket, ORIGIN)).is_err());
    for _ in 0..MAX_TICKETS {
        auth.issue(ORIGIN).unwrap();
    }
    assert_eq!(
        auth.issue(ORIGIN).err().unwrap().0,
        StatusCode::TOO_MANY_REQUESTS
    );
    for ticket in auth.tickets.lock().unwrap().values_mut() {
        ticket.expires = Instant::now();
    }
    assert!(auth.issue(ORIGIN).is_ok());
    assert_eq!(auth.tickets.lock().unwrap().len(), 1);
}
