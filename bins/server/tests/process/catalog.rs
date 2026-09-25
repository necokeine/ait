use std::collections::{BTreeMap, BTreeSet};

use futures_util::SinkExt;
use serde_json::json;
use server_protocol::methods::{InboundKind, PASEO_METHODS};
use tokio_tungstenite::tungstenite::Message;

use super::{TOKEN, ready, start, transport};

#[tokio::test]
async fn production_registers_every_canonical_method_and_routes_each_placeholder() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("server.log");
    let mut server = start(&root.path().join("state"), &log);
    let address = ready(&mut server, &log).await;
    let info: server_protocol::ServerInfo = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/v1/server/info"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let published = info.capabilities.into_iter().collect::<BTreeSet<_>>();
    let implemented = info
        .implemented_capabilities
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(published.len(), 195);
    assert_eq!(implemented.len(), 122);
    assert!(implemented.is_subset(&published));
    for retired in [
        "project.open",
        "project.list",
        "project.get",
        "project.close",
    ] {
        assert!(!published.contains(retired));
        assert!(!implemented.contains(retired));
    }
    let catalog = PASEO_METHODS
        .iter()
        .map(|spec| (spec.canonical_name, spec.kind))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(catalog.len(), 188);
    assert!(catalog.keys().all(|method| published.contains(*method)));
    let placeholders = catalog
        .into_iter()
        .filter(|(method, _)| !implemented.contains(*method))
        .collect::<Vec<_>>();
    assert_eq!(placeholders.len(), 73);
    for batch in placeholders.chunks(64) {
        let methods = batch.iter().map(|(method, _)| *method).collect::<Vec<_>>();
        let mut socket = transport::connect(&address, &methods).await;
        for (method, kind) in batch {
            let reply = match kind {
                InboundKind::Request => {
                    transport::request(&mut socket, method, serde_json::Value::Null).await
                }
                InboundKind::Event => {
                    socket
                        .send(Message::Text(
                            json!({"type":"event","method":method,"params":{}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    transport::receive(&mut socket).await
                }
                InboundKind::Response => {
                    socket
                        .send(Message::Text(
                            json!({
                                "type":"response",
                                "request_id":"browser-1",
                                "method":method,
                                "params":{},
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                    transport::receive(&mut socket).await
                }
            };
            assert_eq!(reply["code"], "not_implemented", "{method}");
            assert_eq!(reply["retryable"], false, "{method}");
        }
    }
}
