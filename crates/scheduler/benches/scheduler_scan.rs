//! Scheduler scan-planning baseline.

use std::hint::black_box;

use ait_domain::TimestampMs;
use ait_scheduler::next_occurrence;
use criterion::{Criterion, criterion_main};

fn scheduler_scan(c: &mut Criterion) {
    let after = TimestampMs(1_788_480_000_000);
    c.bench_function("scheduler_scan/1k_due_plans", |b| {
        b.iter(|| {
            for _ in 0..1_000 {
                black_box(next_occurrence("*/5 * * * *", "UTC", after).unwrap());
            }
        });
    });
}

/// Runs the scheduler-scanning benchmark group.
pub fn benches() {
    let mut criterion = Criterion::default().configure_from_args();
    scheduler_scan(&mut criterion);
}
criterion_main!(benches);
