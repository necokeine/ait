use serde_json::{Value, json};

use super::transport::{Socket, connect, request};
use super::{CREDENTIAL_SENTINEL, ready, start, terminate};

fn configuration(name: &str, enabled: bool) -> Value {
    json!({"name":name,"driver_type":"codex","model":"offline-test-model","credential_ref":"env:AIT_SERVER_CREDENTIAL_TEST","enabled":enabled})
}

#[tokio::test]
async fn binary_agent_revisions_defaults_reconnect_and_secret_exclusion() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start(&directory, &log);
    let address = ready(&mut process, &log).await;
    let mut unnegotiated = connect(&address, &[]).await;
    assert_eq!(
        request(&mut unnegotiated, "agent.list", json!({})).await["code"],
        "unsupported_capability"
    );
    drop(unnegotiated);
    let mut client = connect(&address, server_provider::protocol::agent::CAPABILITIES).await;
    assert_eq!(
        request(&mut client, "agent.default.get", json!({})).await["result"],
        json!({"agent_id":null,"version":0})
    );
    let create = json!({"config":configuration("original",true),"idempotency_key":"create"});
    let created = request(&mut client, "agent.configure", create.clone()).await;
    assert_eq!(created["type"], "response", "{created}");
    let original = created["result"].clone();
    let id = &original["agent_id"];
    assert_eq!(original["revision"], 1);
    assert_eq!(
        request(&mut client, "agent.default.get", json!({})).await["result"]["agent_id"],
        Value::Null
    );
    let select = json!({"agent_id":id,"expected_version":0,"idempotency_key":"select"});
    let selected =
        request(&mut client, "agent.default.set", select.clone()).await["result"].clone();
    let update = json!({"agent_id":id,"expected_revision":1,"config":configuration("edited",true),"idempotency_key":"update"});
    let updated = request(&mut client, "agent.configure", update.clone()).await["result"].clone();
    assert_eq!(updated["revision"], 2);
    check_history(&mut client, id, &update).await;
    let (clear, cleared) = disable_default(&mut client, id).await;
    let page = request(&mut client, "agent.list", json!({"limit":1})).await["result"].clone();
    assert_eq!(page["agents"].as_array().unwrap().len(), 1);
    assert_eq!(page["agents"][0]["revision"], 3);
    assert_eq!(
        request(
            &mut client,
            "agent.list",
            json!({"after":page["next_after"]})
        )
        .await["result"]["agents"],
        json!([])
    );
    drop(client);
    let mut client = connect(&address, server_provider::protocol::agent::CAPABILITIES).await;
    assert_eq!(
        request(&mut client, "agent.configure", create.clone()).await["result"],
        original
    );
    terminate(&mut process).await;
    let mut process = start(&directory, &log);
    let mut client = connect(
        &ready(&mut process, &log).await,
        server_provider::protocol::agent::CAPABILITIES,
    )
    .await;
    assert_eq!(
        request(&mut client, "agent.configure", update).await["result"],
        updated
    );
    assert_eq!(
        request(&mut client, "agent.default.set", select).await["result"],
        selected
    );
    assert_eq!(
        request(&mut client, "agent.default.set", clear).await["result"],
        cleared
    );
    let current = request(&mut client, "agent.default.get", json!({})).await;
    assert_eq!(current["result"], cleared["selection"]);
    assert_eq!(
        request(&mut client, "agent.get", json!({"agent_id":id})).await["result"]["revision"],
        3
    );
    assert_eq!(
        request(&mut client, "provider.models", json!({})).await["code"],
        "method_not_found"
    );
    terminate(&mut process).await;
    for path in [directory.join("catalog.sqlite3"), log] {
        let bytes = std::fs::read(path).unwrap();
        assert!(
            !bytes
                .windows(CREDENTIAL_SENTINEL.len())
                .any(|window| window == CREDENTIAL_SENTINEL.as_bytes())
        );
    }
}

#[tokio::test]
async fn agent_rpc_rejects_unsafe_or_ambiguous_parameters_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("server.log");
    let mut process = start(&root.path().join("server"), &log);
    let mut client = connect(
        &ready(&mut process, &log).await,
        server_provider::protocol::agent::CAPABILITIES,
    )
    .await;
    let valid = json!({"config":configuration("original",true),"idempotency_key":"create"});
    reject_configurations(&mut client, &valid).await;
    assert_eq!(
        request(&mut client, "agent.list", json!({})).await["result"]["agents"],
        json!([])
    );
    for (method, params) in [
        ("agent.list", json!({"limit":0})),
        ("agent.list", json!({"limit":51})),
        ("agent.list", json!({"after":"bad"})),
        ("agent.get", json!({"agent_id":"bad"})),
        (
            "agent.get",
            json!({"agent_id":uuid::Uuid::new_v4().to_string(),"revision":0}),
        ),
        ("agent.default.get", json!({"unexpected":true})),
        (
            "agent.default.set",
            json!({"expected_version":0,"idempotency_key":"missing-target"}),
        ),
        (
            "agent.default.set",
            json!({"agent_id":"bad","expected_version":0,"idempotency_key":"bad"}),
        ),
        (
            "agent.default.set",
            json!({"agent_id":null,"expected_version":u64::MAX,"idempotency_key":"bad"}),
        ),
        (
            "agent.default.set",
            json!({"agent_id":null,"expected_version":0,"idempotency_key":""}),
        ),
    ] {
        assert_eq!(
            request(&mut client, method, params).await["code"],
            "invalid_message"
        );
    }
    let created = request(&mut client, "agent.configure", valid.clone()).await["result"].clone();
    assert_eq!(
        request(
            &mut client,
            "agent.get",
            json!({"agent_id":created["agent_id"],"revision":2})
        )
        .await["code"],
        "agent_revision_not_found"
    );
    assert_eq!(
        request(
            &mut client,
            "agent.get",
            json!({"agent_id":uuid::Uuid::new_v4().to_string()})
        )
        .await["code"],
        "agent_not_found"
    );
    let mut conflict = valid;
    conflict["config"]["model"] = json!("different");
    assert_eq!(
        request(&mut client, "agent.configure", conflict).await["code"],
        "idempotency_conflict"
    );
    terminate(&mut process).await;
}

async fn check_history(client: &mut Socket, id: &Value, update: &Value) {
    assert_eq!(
        request(client, "agent.get", json!({"agent_id":id,"revision":1})).await["result"]["config"]
            ["name"],
        "original"
    );
    assert_eq!(
        request(client, "agent.get", json!({"agent_id":id})).await["result"]["config"]["name"],
        "edited"
    );
    let mut stale = update.clone();
    stale["idempotency_key"] = json!("stale");
    assert_eq!(
        request(client, "agent.configure", stale).await["code"],
        "agent_revision_conflict"
    );
}

async fn disable_default(client: &mut Socket, id: &Value) -> (Value, Value) {
    let disable = json!({"agent_id":id,"expected_revision":2,"config":configuration("disabled",false),"idempotency_key":"disable"});
    assert_eq!(
        request(client, "agent.configure", disable.clone()).await["code"],
        "agent_is_default"
    );
    let clear = json!({"agent_id":null,"expected_version":1,"idempotency_key":"clear"});
    let cleared = request(client, "agent.default.set", clear.clone()).await["result"].clone();
    assert_eq!(cleared["selection"]["version"], 2);
    assert_eq!(
        request(client, "agent.configure", disable).await["result"]["revision"],
        3
    );
    assert_eq!(
        request(
            client,
            "agent.default.set",
            json!({"agent_id":id,"expected_version":2,"idempotency_key":"disabled"})
        )
        .await["code"],
        "agent_disabled"
    );
    assert_eq!(
        request(
            client,
            "agent.default.set",
            json!({"agent_id":null,"expected_version":0,"idempotency_key":"stale"})
        )
        .await["code"],
        "agent_default_conflict"
    );
    (clear, cleared)
}

async fn reject_configurations(client: &mut Socket, valid: &Value) {
    for (field, value) in [
        ("api_key", json!(CREDENTIAL_SENTINEL)),
        ("credential_ref", json!(CREDENTIAL_SENTINEL)),
        ("credential_ref", json!("env:AIT_SERVER_TOKEN")),
        ("driver_type", json!("fake")),
        ("model", json!("bad?credential=value")),
        ("name", json!(" \n")),
    ] {
        let mut params = valid.clone();
        params["config"][field] = value;
        let response = request(client, "agent.configure", params).await;
        assert_eq!(response["code"], "invalid_message", "{response}");
        assert!(!response.to_string().contains(CREDENTIAL_SENTINEL));
    }
    for changes in [
        json!({"agent_id":uuid::Uuid::new_v4().to_string()}),
        json!({"expected_revision":1}),
        json!({"agent_id":"bad-id","expected_revision":1}),
        json!({"agent_id":uuid::Uuid::new_v4().to_string(),"expected_revision":0}),
        json!({"idempotency_key":""}),
        json!({"idempotency_key":"bad key"}),
        json!({"idempotency_key":"x".repeat(129)}),
    ] {
        let mut params = valid.clone();
        params
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        assert_eq!(
            request(client, "agent.configure", params).await["code"],
            "invalid_message"
        );
    }
}
