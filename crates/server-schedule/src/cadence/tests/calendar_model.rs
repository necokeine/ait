//! Independent local-calendar oracle for the DST cases in Paseo schedule/cron.test.ts.
use super::*;
use chrono::{LocalResult, TimeZone};

fn local_candidates(zone: Tz, after: DateTime<Utc>, hour: u32, minute: u32) -> Vec<DateTime<Utc>> {
    let date = after.with_timezone(&zone).date_naive();
    let mut matches = Vec::new();
    for days in 0..16 {
        let local = date
            .checked_add_signed(Duration::days(days))
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap();
        match zone.from_local_datetime(&local) {
            LocalResult::Single(at) => matches.push(at.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                matches.push(first.with_timezone(&Utc));
                matches.push(second.with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    matches.sort_unstable();
    matches.into_iter().filter(|at| *at > after).collect()
}

#[test]
fn daily_cron_matches_independent_local_calendar_across_dst_and_skipped_dates() {
    let cases = [
        ("America/New_York", "2026-03-07T00:00:00Z", 2, 30),
        ("America/New_York", "2026-10-31T00:00:00Z", 1, 30),
        ("Australia/Lord_Howe", "2026-04-03T00:00:00Z", 1, 45),
        ("Australia/Lord_Howe", "2026-10-02T00:00:00Z", 2, 15),
        ("Pacific/Apia", "2011-12-28T00:00:00Z", 9, 0),
        ("Asia/Kathmandu", "2026-01-01T00:00:00Z", 9, 15),
    ];
    for (name, start, hour, minute) in cases {
        let zone: Tz = name.parse().unwrap();
        let mut cursor = time(start);
        let expected = local_candidates(zone, cursor, hour, minute);
        let cadence = cron(&format!("{minute} {hour} * * *"), Some(name));
        for expected in expected.into_iter().take(12) {
            let actual = next(&cadence, cursor).unwrap();
            assert_eq!(actual, expected, "{name} after {cursor}");
            assert!(actual > cursor);
            cursor = actual;
        }
    }
}

#[test]
fn day_and_weekday_intersection_matches_calendar_dates_across_leap_year() {
    let mut cursor = time("2027-12-01T00:00:00Z");
    let expected: Vec<_> = (0..366)
        .map(|offset| time("2027-12-01T09:00:00Z") + Duration::days(offset))
        .filter(|candidate| candidate.day() == 13 && candidate.weekday() == chrono::Weekday::Fri)
        .collect();
    assert_eq!(expected.len(), 1);
    for expected in expected {
        let actual = next(&cron("0 9 13 * 5", None), cursor).unwrap();
        assert_eq!(actual, expected);
        cursor = actual;
    }
    assert_eq!(
        next(&cron("0 9 29 2 *", None), time("2028-02-28T09:00:00Z")).unwrap(),
        time("2028-02-29T09:00:00Z")
    );
}
