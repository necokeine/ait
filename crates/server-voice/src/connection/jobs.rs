use std::{sync::Arc, time::Duration};

use server_model::Runtime;
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    Error,
    audio::Audio,
    ports::Transcriber,
    service::{Speech, Target},
};

use super::Completion;

pub(super) struct DictationJob {
    pub id: String,
    pub generation: String,
    pub final_result: bool,
    pub audio: Audio,
    pub stt: Arc<dyn Transcriber>,
    pub cancel: CancellationToken,
    pub sender: mpsc::Sender<Completion>,
    pub permit: OwnedSemaphorePermit,
}

pub(super) fn dictation(runtime: &Arc<Runtime>, job: DictationJob) {
    let shutdown = runtime.cancellation.clone();
    runtime.tasks.spawn(async move {
        let _permit = job.permit;
        let result = supervise(
            job.cancel.clone(),
            shutdown,
            job.stt.transcribe(job.audio, job.cancel.clone()),
        )
        .await;
        if !job.cancel.is_cancelled() || matches!(result, Err(Error::Timeout)) {
            let _ = job
                .sender
                .send(Completion::Dictation {
                    id: job.id,
                    generation: job.generation,
                    final_result: job.final_result,
                    result,
                })
                .await;
        }
    });
}

pub(super) struct VoiceJob {
    pub generation: String,
    pub audio: Audio,
    pub target: Option<Arc<Target>>,
    pub service: Speech,
    pub cancel: CancellationToken,
    pub sender: mpsc::Sender<Completion>,
    pub permit: OwnedSemaphorePermit,
}

pub(super) fn voice(runtime: &Arc<Runtime>, mut job: VoiceJob) {
    let shutdown = runtime.cancellation.clone();
    runtime.tasks.spawn(async move {
        let result = supervise(job.cancel.clone(), shutdown, voice_turn(&mut job)).await;
        if !job.cancel.is_cancelled() || matches!(result, Err(Error::Timeout)) {
            let _ = job
                .sender
                .send(Completion::VoiceFinished {
                    generation: job.generation,
                    result,
                })
                .await;
        }
        drop(job.permit);
    });
}

async fn voice_turn(job: &mut VoiceJob) -> Result<(), Error> {
    let stt = job.service.stt.as_ref().ok_or(Error::Unavailable)?;
    let audio = Audio {
        bytes: std::mem::take(&mut job.audio.bytes),
        format: job.audio.format,
    };
    let transcript = stt.transcribe(audio, job.cancel.clone()).await?;
    if job.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    job.sender
        .send(Completion::Transcript {
            generation: job.generation.clone(),
            transcript: transcript.clone(),
        })
        .await
        .map_err(|_| Error::Cancelled)?;
    let Some(target) = &job.target else {
        return Ok(());
    };
    if transcript.text.trim().is_empty() {
        return Ok(());
    }
    let agents = job.service.agents.as_ref().ok_or(Error::Unavailable)?;
    let tts = job.service.tts.as_ref().ok_or(Error::Unavailable)?;
    // A replacement utterance cannot enter the Agent until the previous cancellation settles.
    let _lane = tokio::select! { biased;
        () = job.cancel.cancelled() => return Err(Error::Cancelled),
        guard = target.lane.lock() => guard,
    };
    let text = agents
        .turn(&target.id, &transcript.text, job.cancel.clone())
        .await?;
    if job.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !text.trim().is_empty() {
        let audio = tts.synthesize(&text, job.cancel.clone()).await?;
        job.sender
            .send(Completion::Audio {
                generation: job.generation.clone(),
                audio,
            })
            .await
            .map_err(|_| Error::Cancelled)?;
    }
    Ok(())
}

async fn supervise<T>(
    cancel: CancellationToken,
    shutdown: CancellationToken,
    work: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::pin!(work);
    let reason = tokio::select! { biased;
        () = cancel.cancelled() => Error::Cancelled,
        () = shutdown.cancelled() => Error::Cancelled,
        () = tokio::time::sleep(Duration::from_secs(120)) => Error::Timeout,
        result = &mut work => return result,
    };
    cancel.cancel();
    // Give local child/Agent adapters time to explicitly terminate and reap accepted work.
    let _ = tokio::time::timeout(Duration::from_secs(5), &mut work).await;
    Err(reason)
}
