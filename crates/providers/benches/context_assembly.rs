//! Long provider-context validation baseline.

use std::{collections::BTreeMap, hint::black_box};

use ait_providers::{
    ContentPart, ProviderCapabilities, ProviderMessage, ProviderParameters, ProviderRequest, Role,
    validate_request,
};
use criterion::{Criterion, criterion_main};

fn context_assembly(c: &mut Criterion) {
    let request = ProviderRequest {
        messages: (0..2_000)
            .map(|index| ProviderMessage {
                role: if index % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentPart::Text {
                    text: "context token ".repeat(64),
                }],
            })
            .collect(),
        tools: Vec::new(),
        parameters: ProviderParameters {
            max_output_tokens: Some(1_024),
            temperature: Some(0.0),
            extra: BTreeMap::new(),
        },
        required_capabilities: ProviderCapabilities::default(),
    };
    c.bench_function("context_assembly/2k_messages", |b| {
        b.iter(|| {
            let assembled = black_box(request.clone());
            validate_request(
                black_box(&assembled),
                black_box(ProviderCapabilities::default()),
            )
            .unwrap();
            black_box(assembled);
        });
    });
}

/// Runs the provider-context validation benchmark group.
pub fn benches() {
    let mut criterion = Criterion::default().configure_from_args();
    context_assembly(&mut criterion);
}
criterion_main!(benches);
