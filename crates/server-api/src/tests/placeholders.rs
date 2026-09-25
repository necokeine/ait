use std::collections::BTreeMap;

use super::*;
use server_protocol::methods::{InboundKind, PASEO_METHODS};

#[tokio::test]
async fn every_catalog_placeholder_uses_its_canonical_envelope_and_explicit_error() {
    let fixture = Fixture::start().await;
    let implemented = fixture.api.shared.info.implemented_capabilities.clone();
    let catalog = PASEO_METHODS
        .iter()
        .map(|spec| (spec.canonical_name, spec.kind))
        .collect::<BTreeMap<_, _>>();
    let placeholders = catalog
        .into_iter()
        .filter(|(method, _)| !implemented.iter().any(|ready| ready == method))
        .collect::<Vec<_>>();
    assert_eq!(placeholders.len(), 184);
    for batch in placeholders.chunks(64) {
        let mut socket = fixture.socket().await;
        let mut offer = hello();
        offer["capabilities"] = json!(batch.iter().map(|(method, _)| method).collect::<Vec<_>>());
        send(&mut socket, offer).await;
        let negotiated = receive(&mut socket).await;
        assert_eq!(
            negotiated["negotiated_capabilities"]
                .as_array()
                .unwrap()
                .len(),
            batch.len()
        );
        for (method, kind) in batch {
            let result = match kind {
                InboundKind::Request => request(&mut socket, method, Value::Null).await,
                InboundKind::Event => {
                    send(
                        &mut socket,
                        json!({"type":"event","method":method,"params":{}}),
                    )
                    .await;
                    receive(&mut socket).await
                }
                InboundKind::Response => {
                    send(
                        &mut socket,
                        json!({
                            "type":"response",
                            "request_id":"browser-1",
                            "method":method,
                            "params":{},
                        }),
                    )
                    .await;
                    receive(&mut socket).await
                }
            };
            assert_eq!(result["code"], "not_implemented", "{method}");
            assert_eq!(result["retryable"], false, "{method}");
            assert_eq!(
                result["request_id"],
                match kind {
                    InboundKind::Request => json!("r1"),
                    InboundKind::Event => Value::Null,
                    InboundKind::Response => json!("browser-1"),
                },
                "{method}"
            );
        }
    }
    fixture.stop().await;
}

#[tokio::test]
async fn placeholder_negotiation_wrong_kind_and_legacy_names_are_distinct() {
    let fixture = Fixture::start().await;
    let mut socket = fixture.socket().await;
    send(&mut socket, hello()).await;
    receive(&mut socket).await;
    assert_eq!(
        request(&mut socket, "schedule.list.request", Value::Null).await["code"],
        "unsupported_capability"
    );
    assert_eq!(
        request(&mut socket, "schedule/list", Value::Null).await["code"],
        "method_not_found"
    );
    send(
        &mut socket,
        json!({"type":"event","method":"schedule.list.request"}),
    )
    .await;
    assert_eq!(receive(&mut socket).await["code"], "invalid_message");
    assert_eq!(
        request(&mut socket, "terminal.input", Value::Null).await["code"],
        "invalid_message"
    );
    assert_eq!(
        request(
            &mut socket,
            "connection.ping",
            json!({"nonce":"still-open"})
        )
        .await["result"]["nonce"],
        "still-open"
    );
    fixture.stop().await;
}
