//! Provider-owned observations from Paseo's owned-subscriptions.e2e.test.ts.

use server_model::outbound::{Frame, Outbound, Queued};
use server_model::{Context, Lifecycle, Limits, Request, Runtime, ServerInfo, VERSION};
use tokio::sync::mpsc;

use crate::connection::Connection;
use crate::dispatch::State;

use super::*;

struct Peer {
    connection: Connection,
    outbound: Outbound,
    receiver: mpsc::Receiver<Queued>,
    state: State,
}

impl Peer {
    fn new(execution: &AgentExecution) -> Self {
        let (outbound, receiver) = Outbound::new();
        Self {
            connection: Connection::new("shared-logical-client"),
            outbound,
            receiver,
            state: State {
                runtime: Arc::new(Runtime::new(ServerInfo {
                    server_id: "server".to_owned(),
                    instance_id: "instance".to_owned(),
                    listen: "127.0.0.1:0".to_owned(),
                    lifecycle: Lifecycle::Ready,
                    protocol: VERSION,
                    capabilities: Vec::new(),
                    implemented_capabilities: Vec::new(),
                    limits: Limits::default(),
                })),
                agents: None,
                agent_runtime: None,
                agent_execution: Some(execution.clone()),
                has_terminals: false,
            },
        }
    }

    async fn subscribe(&mut self, params: Value, budget: usize) -> Value {
        self.connection
            .subscribe(
                Context {
                    request: Request {
                        id: "source-request".to_owned(),
                        method: "agent.timeline.set_subscription.request".to_owned(),
                        params,
                    },
                    runtime: &self.state.runtime,
                    outbound: &self.outbound,
                    available_subscriptions: budget,
                },
                &self.state,
            )
            .await
            .unwrap();
        self.next()
    }

    fn next(&mut self) -> Value {
        let queued = self.receiver.try_recv().unwrap();
        let Frame::Text(text) = &queued.message else {
            panic!("expected a JSON observation");
        };
        serde_json::from_str(text).unwrap()
    }
}

#[tokio::test]
async fn identical_timeline_queries_have_independent_ids_and_release_lifetimes() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    let first = peer.subscribe(json!({"agentIds":[id]}), 8).await;
    let second = peer.subscribe(json!({"agentIds":[id]}), 7).await;
    let first = first["result"]["subscriptionId"].as_str().unwrap();
    let second = second["result"]["subscriptionId"].as_str().unwrap();
    assert_ne!(first, second);
    assert_eq!(peer.connection.len(), 2);
    peer.connection.release(first);
    assert_eq!(peer.connection.len(), 1);
    execution.timeline().events().publish(
        id,
        "agent_stream",
        &json!({"agentId":id,"event":{"type":"turn_completed"}}),
    );
    let event = peer.next();
    assert_eq!(event["params"]["subscriptionId"], second);
    assert_eq!(event["params"]["agentId"], id);
    assert!(peer.receiver.try_recv().is_err());
    peer.connection.release(second);
    assert!(peer.connection.is_empty());
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn same_logical_client_connections_cannot_release_each_others_observers() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut first = Peer::new(&execution);
    let mut second = Peer::new(&execution);
    let subscribed = first.subscribe(json!({"agentIds":[id]}), 8).await;
    let owned_id = subscribed["result"]["subscriptionId"].as_str().unwrap();
    second.connection.release(owned_id);
    execution
        .timeline()
        .events()
        .publish(id, "agent_stream", &json!({"agentId":id}));
    assert_eq!(first.next()["params"]["subscriptionId"], owned_id);
    assert!(second.receiver.try_recv().is_err());
    assert_eq!(first.connection.len(), 1);
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropping_one_connection_stops_only_its_observers() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut first = Peer::new(&execution);
    let mut second = Peer::new(&execution);
    first.subscribe(json!({"agentIds":[id]}), 8).await;
    let surviving = second.subscribe(json!({"agentIds":[id]}), 8).await;
    drop(first.connection);
    execution
        .timeline()
        .events()
        .publish(id, "agent_stream", &json!({"agentId":id}));
    assert!(first.receiver.try_recv().is_err());
    assert_eq!(
        second.next()["params"]["subscriptionId"],
        surviving["result"]["subscriptionId"]
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn timeline_subscription_budget_rejection_keeps_existing_observers_active() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    let subscribed = peer.subscribe(json!({"agentIds":[id]}), 1).await;
    let exhausted = peer.subscribe(json!({"agentIds":[id]}), 0).await;
    assert_eq!(exhausted["code"], "resource_exhausted");
    assert_eq!(peer.connection.len(), 1);
    execution
        .timeline()
        .events()
        .publish(id, "agent_stream", &json!({"agentId":id}));
    assert_eq!(
        peer.next()["params"]["subscriptionId"],
        subscribed["result"]["subscriptionId"]
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn duplicated_agent_ids_create_one_delivery_per_observer() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    let response = peer.subscribe(json!({"agentIds":[id,id]}), 8).await;
    assert_eq!(response["result"]["agentIds"], json!([id]));
    execution
        .timeline()
        .events()
        .publish(id, "agent_stream", &json!({"agentId":id}));
    peer.next();
    assert!(peer.receiver.try_recv().is_err());
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn timeline_observer_does_not_receive_an_unselected_agents_events() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    peer.subscribe(json!({"agentIds":[id]}), 8).await;
    execution.timeline().events().publish(
        "different-agent",
        "agent_stream",
        &json!({"agentId":"different-agent"}),
    );
    assert!(peer.receiver.try_recv().is_err());
    execution
        .timeline()
        .events()
        .publish(id, "agent_stream", &json!({"agentId":id}));
    assert_eq!(peer.next()["params"]["agentId"], id);
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_timeline_query_does_not_retain_a_partial_observer() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    let invalid = peer.subscribe(json!({"agentIds":vec![id;33]}), 8).await;
    assert_eq!(invalid["code"], "invalid_message");
    let missing = peer
        .subscribe(
            json!({"agentIds":[id,"00000000-0000-4000-8000-000000000000"]}),
            8,
        )
        .await;
    assert_eq!(missing["code"], "agent_not_found");
    assert!(peer.connection.is_empty());
    execution
        .timeline()
        .events()
        .publish(id, "agent_stream", &json!({"agentId":id}));
    assert!(peer.receiver.try_recv().is_err());
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_subscription_response_delivery_does_not_activate_the_observer() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    peer.receiver.close();
    assert!(
        peer.connection
            .subscribe(
                Context {
                    request: Request {
                        id: "closed".to_owned(),
                        method: "agent.timeline.set_subscription.request".to_owned(),
                        params: json!({"agentIds":[id]})
                    },
                    runtime: &peer.state.runtime,
                    outbound: &peer.outbound,
                    available_subscriptions: 8,
                },
                &peer.state
            )
            .await
            .is_err()
    );
    assert!(peer.connection.is_empty());
    assert!(peer.outbound.failure().is_cancelled());
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn provider_children_publish_to_parent_observers_but_keep_separate_durable_timelines() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let mut peer = Peer::new(&execution);
    peer.subscribe(json!({"agentIds":[id]}), 8).await;
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"live-subagent"}),
        )
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    let mut updates = Vec::new();
    while let Ok(queued) = peer.receiver.try_recv() {
        let Frame::Text(text) = &queued.message else {
            panic!("expected JSON");
        };
        let value: Value = serde_json::from_str(text).unwrap();
        if value["method"] == "agent.provider_subagents.update" {
            updates.push(value["params"].clone());
        }
    }
    assert!(updates.iter().any(|update| update["kind"] == "upsert" && update["subagent"]["status"] == "completed"));
    assert!(updates.iter().any(
        |update| update["kind"] == "timeline" && update["item"]["text"] == "Independent child"
    ));
    assert!(
        updates
            .iter()
            .any(|update| update["kind"] == "timeline" && update["item"]["text"] == " answer")
    );
    let children = execution
        .execute(
            "agent.provider_subagents.list.request",
            json!({"parentAgentId":id}),
        )
        .await
        .unwrap();
    let child = children["subagents"][0]["id"].as_str().unwrap();
    let page = execution
        .execute(
            "agent.provider_subagents.timeline.get.request",
            json!({"parentAgentId":id,"subagentId":child}),
        )
        .await
        .unwrap();
    assert_eq!(
        page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["item"]["text"].as_str())
            .collect::<String>(),
        "Independent child answer"
    );
    let (_, parent) = execution.timeline().read(id).unwrap();
    assert!(
        !parent
            .iter()
            .any(|row| row.entry.item["text"] == "Independent child answer")
    );
    execution.shutdown().await.unwrap();
    let (execution, _) = worker(&fixture);
    let children = execution
        .execute(
            "agent.provider_subagents.list.request",
            json!({"parentAgentId":id}),
        )
        .await
        .unwrap();
    assert_eq!(children["subagents"][0]["id"], child);
    let page = execution
        .execute(
            "agent.provider_subagents.timeline.get.request",
            json!({"parentAgentId":id,"subagentId":child}),
        )
        .await
        .unwrap();
    assert_eq!(
        page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["item"]["text"].as_str())
            .collect::<String>(),
        "Independent child answer"
    );
    execution.shutdown().await.unwrap();
}
