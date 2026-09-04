//! Scheduler scan-planning baseline.
#![allow(missing_docs)]

use std::hint::black_box;

use ait_domain::TimestampMs;
use ait_scheduler::next_occurrence;
use criterion::{Criterion, criterion_group, criterion_main};

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

criterion_group!(benches, scheduler_scan);
criterion_main!(benches);
