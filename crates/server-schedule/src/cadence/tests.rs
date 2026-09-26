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

#[test]
fn weekday_timezone_schedule_tracks_winter_and_summer_offsets() {
    let cadence = cron("0 9 * * 1-5", Some("America/New_York"));
    assert_eq!(
        next(&cadence, time("2026-01-05T13:59:30Z")).unwrap(),
        time("2026-01-05T14:00:00Z")
    );
    assert_eq!(
        next(&cadence, time("2026-07-06T12:59:30Z")).unwrap(),
        time("2026-07-06T13:00:00Z")
    );
    assert_eq!(
        next(&cadence, time("2026-07-10T13:00:00Z")).unwrap(),
        time("2026-07-13T13:00:00Z")
    );
}

#[test]
fn repeated_fall_back_slots_are_distinct_even_with_explicit_day_and_month() {
    let cadence = cron("30 1 1 11 *", Some("America/New_York"));
    let first = next(&cadence, time("2026-11-01T05:29:30Z")).unwrap();
    let second = next(&cadence, first).unwrap();
    assert_eq!(first, time("2026-11-01T05:30:00Z"));
    assert_eq!(second, time("2026-11-01T06:30:00Z"));
}

#[test]
fn extra_step_tokens_and_noncanonical_steps_are_rejected() {
    for expression in [
        "*/5/2 * * * *",
        "* */2/3 * * *",
        "0-10/2/2 * * * *",
        "*/+2 * * * *",
        "*/02 * * * *",
        "*/-2 * * * *",
        "*/4294967296 * * * *",
    ] {
        assert_eq!(
            next(&cron(expression, None), time("2026-01-01T00:00:00Z")),
            Err(Error::Invalid),
            "{expression}"
        );
    }
}

#[test]
fn minute_rounding_handles_pre_epoch_and_subsecond_timestamps() {
    assert_eq!(
        next(&cron("* * * * *", None), time("1969-12-31T23:59:59.999Z")).unwrap(),
        time("1970-01-01T00:00:00Z")
    );
    assert_eq!(
        next(&cron("* * * * *", None), time("2026-01-01T00:00:00.001Z")).unwrap(),
        time("2026-01-01T00:01:00Z")
    );
}

#[test]
fn interval_and_cron_report_calendar_overflow_instead_of_panicking() {
    assert_eq!(
        next(&Cadence::Every { every_ms: 1 }, DateTime::<Utc>::MAX_UTC),
        Err(Error::Invalid)
    );
    assert_eq!(
        next(&cron("* * * * *", None), DateTime::<Utc>::MAX_UTC),
        Err(Error::Invalid)
    );
    assert_eq!(
        next(
            &Cadence::Every { every_ms: -1 },
            time("2026-01-01T00:00:00Z")
        ),
        Err(Error::Invalid)
    );
}

mod calendar_model;
