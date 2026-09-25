use super::{AgentExecution, CancellationToken, ErrorCode, ExecutionState, Value, json, oneshot};

impl AgentExecution {
    /// Admit a voice-owned turn while preserving cancellation across the worker queue.
    ///
    /// `agent` identifies the target, `text` is its prompt, and `cancel` belongs to the voice
    /// operation. Returns its admission receipt including the native turn ID. Cancellation
    /// during admission interrupts only that accepted turn, even if the caller stops waiting.
    /// # Errors
    /// Returns validation, queue, provider or cancellation errors using safe shared codes.
    pub async fn send_voice(
        &self,
        agent: &str,
        text: &str,
        cancel: CancellationToken,
    ) -> Result<Value, ErrorCode> {
        self.call_cancellable(
            "internal.voice.send",
            json!({"agentId":agent,"text":text}),
            Some(cancel),
        )
        .await
    }
}

type Reply = oneshot::Sender<Result<Value, ErrorCode>>;

pub(super) async fn dispatch(
    state: &mut ExecutionState,
    method: &str,
    params: Value,
    context: (Reply, Option<CancellationToken>),
) {
    let (reply, cancel) = context;
    let abandoned = || {
        cancel.as_ref().is_some_and(CancellationToken::is_cancelled)
            || cancel.is_some() && reply.is_closed()
    };
    let result = if abandoned() {
        Err(ErrorCode::AgentIo)
    } else {
        state.execute(method, params).await
    };
    if abandoned() {
        cancel_accepted(state, &result).await;
    }
    if let Err(result) = reply.send(result)
        && cancel.is_some()
    {
        cancel_accepted(state, &result).await;
    }
}

async fn cancel_accepted(state: &mut ExecutionState, result: &Result<Value, ErrorCode>) {
    if let Ok(receipt) = result
        && receipt["accepted"] == true
    {
        let _ = state
            .execute(
                "internal.voice.cancel",
                json!({"agentId":receipt["agentId"],"turnId":receipt["turnId"]}),
            )
            .await;
    }
}
