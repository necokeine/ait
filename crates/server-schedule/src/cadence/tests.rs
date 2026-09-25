use super::*;
fn time(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}
fn cron(expression: &str, timezone: Option<&str>) -> Cadence {
    Cadence::Cron {
        expression: expression.into(),
        timezone: timezone.map(str::to_owned),
    }
}
#[test]
fn interval_and_cron_round_to_the_next_slot() {
    let now = time("2026-01-01T12:01:23Z");
    assert_eq!(
        next(&Cadence::Every { every_ms: 1500 }, now).unwrap(),
        time("2026-01-01T12:01:24.500Z")
    );
    assert_eq!(
        next(&cron("*/15 * * * *", None), now).unwrap(),
        time("2026-01-01T12:15:00Z")
    );
    assert_eq!(
        next(&cron("2,7-11/2 12 * * *", None), now).unwrap(),
        time("2026-01-01T12:02:00Z")
    );
    assert_eq!(
        next(&cron("5/2 * * * *", None), now).unwrap(),
        time("2026-01-01T12:05:00Z")
    );
}
#[test]
fn calendar_fields_use_intersection_and_timezone() {
    assert_eq!(
        next(&cron("0 8 2 * 5", None), time("2026-01-01T00:00:00Z")).unwrap(),
        time("2026-01-02T08:00:00Z")
    );
    assert_eq!(
        next(
            &cron("0 8 * * *", Some("Asia/Shanghai")),
            time("2026-01-01T00:00:00Z")
        )
        .unwrap(),
        time("2026-01-02T00:00:00Z")
    );
}
#[test]
fn dst_skips_nonexistent_time_and_handles_repeated_hour() {
    assert_eq!(
        next(
            &cron("30 2 * * *", Some("America/New_York")),
            time("2026-03-08T06:00:00Z")
        )
        .unwrap(),
        time("2026-03-09T06:30:00Z")
    );
    assert_eq!(
        next(
            &cron("30 1 * * *", Some("America/New_York")),
            time("2026-11-01T05:30:00Z")
        )
        .unwrap(),
        time("2026-11-01T06:30:00Z")
    );
}
#[test]
fn rejects_bad_fields_steps_zone_and_impossible_dates() {
    let now = time("2026-01-01T00:00:00Z");
    for expression in [
        "* * * *",
        "* * * * * *",
        "60 * * * *",
        "* 24 * * *",
        "* * 0 * *",
        "* * * 13 *",
        "* * * * 7",
        "*/0 * * * *",
        "a * * * *",
        "9-1 * * * *",
        "* * * * MON",
        "1,,2 * * * *",
        "0 0 31 2 *",
    ] {
        assert!(next(&cron(expression, None), now).is_err(), "{expression}");
    }
    assert!(next(&cron("* * * * *", Some("bad/timezone")), now).is_err());
    assert!(next(&Cadence::Every { every_ms: 0 }, now).is_err());
}
