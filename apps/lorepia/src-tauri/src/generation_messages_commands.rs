//! Verified message presentation commands, including bounded terminal refresh.
use lorepia_shell_api::{BranchMessagesPageDto, ListBranchMessagesPageInput, MessageDto};
use serde::Deserialize;
use tauri::State;
use tokio::sync::Semaphore;

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchMessagesRequest {
    pub branch_id: String,
}

static HISTORY_ADMISSIONS: Semaphore = Semaphore::const_new(8);
static HISTORY_WORKERS: Semaphore = Semaphore::const_new(2);

#[tauri::command]
pub async fn list_branch_messages_page(
    state: State<'_, AppState>,
    request: ListBranchMessagesPageInput,
) -> CommandResult<BranchMessagesPageDto> {
    let shell = state.shell()?;
    run_history_page_work(move || shell.list_branch_messages_page(request)).await
}

async fn run_history_page_work<T: Send + 'static>(
    work: impl FnOnce() -> lorepia_shell_api::ShellResult<T> + Send + 'static,
) -> CommandResult<T> {
    let admission = HISTORY_ADMISSIONS
        .try_acquire()
        .map_err(|_| CommandError::busy())?;
    let worker = HISTORY_WORKERS
        .acquire()
        .await
        .map_err(|_| CommandError::internal())?;
    tauri::async_runtime::spawn_blocking(move || {
        // Keep both permits until native work ends even if the caller goes away.
        let (_admission, _worker) = (admission, worker);
        work()
    })
    .await
    .map_err(|_| CommandError::internal())?
    .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_branch_messages(
    state: State<'_, AppState>,
    request: BranchMessagesRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_branch_messages(&request.branch_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    request: ConversationRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_messages(&request.conversation_id)
        .map_err(Into::into)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "route identity fields match the existing IPC naming convention"
)]
pub struct GenerationMessagesRequest {
    pub conversation_id: String,
    pub branch_id: String,
    pub generation_id: String,
}

#[tauri::command]
pub fn list_generation_messages(
    state: State<'_, AppState>,
    request: GenerationMessagesRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_generation_messages(
            &request.conversation_id,
            &request.branch_id,
            &request.generation_id,
        )
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use super::GenerationMessagesRequest;

    #[test]
    fn generation_message_route_is_strict_and_requires_all_identities() {
        let valid = r#"{"conversation_id":"c","branch_id":"b","generation_id":"g"}"#;
        assert!(serde_json::from_str::<GenerationMessagesRequest>(valid).is_ok());
        for invalid in [
            r#"{"conversation_id":"c","branch_id":"b"}"#,
            r#"{"conversation_id":"c","branch_id":"b","generation_id":"g","limit":999}"#,
        ] {
            assert!(serde_json::from_str::<GenerationMessagesRequest>(invalid).is_err());
        }
    }

    #[tokio::test]
    async fn history_work_is_bounded_and_cancelled_callers_keep_native_permits() {
        use super::{HISTORY_ADMISSIONS, HISTORY_WORKERS, run_history_page_work};
        let full = HISTORY_ADMISSIONS
            .try_acquire_many(8)
            .expect("reserve admission");
        assert_eq!(
            run_history_page_work(|| Ok(())).await.unwrap_err().code,
            "busy"
        );
        drop(full);

        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_history_page_work(move || {
            started.send(()).expect("notify worker entry");
            wait.recv().expect("release native work");
            Ok(())
        }));
        ready.await.expect("native worker started");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(HISTORY_ADMISSIONS.available_permits(), 7);
        assert_eq!(HISTORY_WORKERS.available_permits(), 1);
        release.send(()).expect("finish work");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while HISTORY_ADMISSIONS.available_permits() != 8
                || HISTORY_WORKERS.available_permits() != 2
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("finished work releases permits");
    }
}
