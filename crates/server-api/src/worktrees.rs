use serde_json::Value;
use server_filesystem::rpc::worktrees::Dispatched;
use server_protocol::ErrorCode;

use crate::Shared;
pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Dispatched, ErrorCode> {
    let method = method.to_owned();
    let dispatched = crate::jobs::run(
        state,
        state.worktrees.clone(),
        ErrorCode::RegistryIo,
        move |worktrees| {
            server_filesystem::rpc::worktrees::execute(worktrees, &method, params)
                .map_err(Into::into)
        },
    )
    .await?;
    if let Some(workspace_id) = dispatched.created_workspace_id.clone() {
        let _ = crate::jobs::run(
            state,
            state.workspace_automation.clone(),
            ErrorCode::RegistryIo,
            move |automation| {
                automation
                    .start_created_setup(&workspace_id)
                    .map(|_| ())
                    .map_err(|_| ErrorCode::RegistryIo)
            },
        )
        .await;
    }
    Ok(dispatched)
}
