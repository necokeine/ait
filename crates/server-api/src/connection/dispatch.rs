use server_model::outbound::QueueError;
use server_model::{Context, ErrorCode};

use super::ConnectionSubscriptions;
use crate::Shared;
use crate::capabilities::Group;

pub(super) async fn request(
    group: Group,
    context: Context<'_>,
    state: &Shared,
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<(), QueueError> {
    let outbound = context.outbound;
    match group {
        Group::Schedule(group) => {
            server_schedule::dispatch::dispatch(group, context, &state.schedule).await
        }
        Group::Browser(group) => server_browser::dispatch::dispatch(
            group,
            context,
            &state.browser,
            &mut subscriptions.browser,
        ),
        Group::Voice(group) => {
            server_voice::dispatch::dispatch(group, context, &state.voice, &mut subscriptions.voice)
                .await
        }
        Group::Metadata(group) => {
            match server_metadata::dispatch::dispatch(
                group,
                context,
                &state.metadata,
                &mut subscriptions.metadata,
            )
            .await?
            {
                server_metadata::dispatch::Completion::Complete => Ok(()),
                server_metadata::dispatch::Completion::DaemonSnapshot {
                    request_id,
                    method,
                    params,
                } => {
                    let result = daemon_snapshot(state, method, params).await;
                    outbound.respond(request_id, result)
                }
                server_metadata::dispatch::Completion::Release {
                    request_id,
                    subscription_id,
                } => {
                    subscriptions.release(&subscription_id);
                    let value = serde_json::to_value(
                        server_model::subscription::SubscriptionReleaseResult { subscription_id },
                    )
                    .map_err(|_| ErrorCode::InvalidMessage);
                    outbound.respond(request_id, value)
                }
            }
        }
        Group::Filesystem(group) => {
            server_filesystem::dispatch::dispatch(
                group,
                context,
                &state.filesystem,
                &mut subscriptions.filesystem,
            )
            .await
        }
        Group::Provider(group) => {
            match server_provider::dispatch::dispatch(
                group,
                context,
                &state.provider,
                &mut subscriptions.provider,
            )
            .await?
            {
                server_provider::dispatch::Completion::Complete => Ok(()),
                server_provider::dispatch::Completion::CloseTerminals {
                    request_id,
                    mut value,
                    terminal_ids,
                } => {
                    let result =
                        server_terminal::dispatch::close_many(&state.terminal, terminal_ids)
                            .await
                            .map(|terminals| {
                                value["terminals"] = terminals;
                                value
                            });
                    outbound.respond(request_id, result)
                }
            }
        }
        Group::Terminal(group) => {
            server_terminal::dispatch::dispatch(
                group,
                context,
                &state.terminal,
                &mut subscriptions.terminals,
            )
            .await
        }
    }
}

async fn daemon_snapshot(
    state: &Shared,
    method: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, ErrorCode> {
    let providers = match &state.provider.agent_execution {
        Some(execution) => {
            let value = execution
                .execute("provider.available.list.request", serde_json::json!({}))
                .await?;
            serde_json::from_value(value["providers"].clone()).map_err(|_| ErrorCode::AgentIo)?
        }
        None => Vec::new(),
    };
    server_metadata::dispatch::daemon_snapshot(&state.metadata, method, params, providers).await
}
