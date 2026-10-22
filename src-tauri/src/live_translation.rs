//! Cancellable, single-flight translation. Recording never waits for this module.
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::{
    process::{Command, CommandChild, CommandEvent},
    ShellExt,
};
use tokio_util::sync::CancellationToken;

const MAX_SOURCE_BYTES: usize = 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_INFERENCE_TIME: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TargetLanguage {
    Ja,
    En,
}

impl TargetLanguage {
    fn code(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }
}

#[derive(Clone)]
struct Activity {
    epoch: u64,
    session_id: String,
    target: TargetLanguage,
    cancel: CancellationToken,
    current: Option<(u64, CancellationToken)>,
    last_started: u64,
    cancelled_through: u64,
}

#[derive(Default)]
struct Control {
    epoch: u64,
    active: Option<Activity>,
}

impl Control {
    fn begin(&mut self, session_id: String, target: TargetLanguage) -> Activity {
        if let Some(old) = self.active.take() {
            old.cancel.cancel();
        }
        self.epoch += 1;
        let activity = Activity {
            epoch: self.epoch,
            session_id,
            target,
            cancel: CancellationToken::new(),
            current: None,
            last_started: 0,
            cancelled_through: 0,
        };
        self.active = Some(activity.clone());
        activity
    }

    fn request(
        &mut self,
        epoch: u64,
        id: u64,
        session: &str,
    ) -> Result<(Activity, CancellationToken), String> {
        let active = self
            .active
            .as_mut()
            .filter(|a| a.epoch == epoch && a.session_id == session)
            .ok_or("translation.cancelled")?;
        if id == 0
            || id <= active.last_started
            || id <= active.cancelled_through
            || active.cancel.is_cancelled()
        {
            return Err("translation.cancelled".into());
        }
        if let Some((_, old)) = active.current.take() {
            old.cancel();
        }
        let token = active.cancel.child_token();
        active.current = Some((id, token.clone()));
        active.last_started = id;
        Ok((active.clone(), token))
    }

    fn cancel_request(&mut self, epoch: u64, id: u64) {
        if let Some(active) = self.active.as_mut().filter(|a| a.epoch == epoch) {
            active.cancelled_through = active.cancelled_through.max(id);
            if let Some((current, token)) = &active.current {
                if *current <= active.cancelled_through {
                    token.cancel();
                }
            }
        }
    }

    fn end(&mut self, epoch: u64) {
        if self.active.as_ref().is_some_and(|a| a.epoch == epoch) {
            if let Some(active) = self.active.take() {
                active.cancel.cancel();
            }
        }
    }

    fn end_session(&mut self, session: &str) {
        if let Some(epoch) = self
            .active
            .as_ref()
            .filter(|a| a.session_id == session)
            .map(|a| a.epoch)
        {
            self.end(epoch);
        }
    }
}

#[derive(Clone, PartialEq)]
struct ModelStamp {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn model_stamp(path: &Path) -> Option<ModelStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(ModelStamp {
        path: path.to_owned(),
        size: metadata.len(),
        modified: metadata.modified().ok()?,
    })
}

#[derive(Default)]
pub(crate) struct LiveTranslationState {
    control: Mutex<Control>,
    work: tokio::sync::Mutex<()>,
    verified_model: Mutex<Option<ModelStamp>>,
}

pub(crate) fn cancel_session(app: &AppHandle, session: &str) {
    if let Some(state) = app.try_state::<LiveTranslationState>() {
        state.control.lock().unwrap().end_session(session);
    }
}

#[derive(Serialize)]
pub(crate) struct TranslationActivity {
    epoch: u64,
    session_id: String,
    model_bytes: u64,
}

#[derive(Serialize)]
pub(crate) struct TranslationResult {
    epoch: u64,
    request_id: u64,
    text: String,
    elapsed_ms: u64,
}

#[derive(Clone, Serialize)]
struct Progress {
    stage: &'static str,
    done: u64,
    total: Option<u64>,
}

fn progress(app: &AppHandle, event: &str, stage: &'static str, done: u64, total: Option<u64>) {
    let _ = app.emit(event, Progress { stage, done, total });
}

fn translation_memory_supported(bytes: Option<u64>) -> bool {
    bytes.is_some_and(|bytes| bytes >= 16 * 1024 * 1024 * 1024)
}

#[tauri::command]
pub(crate) fn begin_live_translation(
    app: AppHandle,
    state: State<'_, LiveTranslationState>,
    target: TargetLanguage,
) -> Result<TranslationActivity, String> {
    if !translation_memory_supported(mojiroku_core::hardware::total_memory_bytes()) {
        return Err("translation.requires_16gb".into());
    }
    let session = crate::live_stt::session_id(&app.state::<crate::live_stt::LiveSttState>())
        .ok_or("translation.no_session")?;
    let activity = state.control.lock().unwrap().begin(session, target);
    Ok(TranslationActivity {
        epoch: activity.epoch,
        session_id: activity.session_id,
        model_bytes: mojiroku_core::models::TRANSLATION_MODEL_BYTES,
    })
}

#[tauri::command]
pub(crate) fn end_live_translation(state: State<'_, LiveTranslationState>, epoch: u64) {
    state.control.lock().unwrap().end(epoch);
}

#[tauri::command]
pub(crate) fn cancel_live_translation_request(
    state: State<'_, LiveTranslationState>,
    epoch: u64,
    request_id: u64,
) {
    state
        .control
        .lock()
        .unwrap()
        .cancel_request(epoch, request_id);
}

/// A sidecar prompt in a fresh temp file (`create_new`, 0600), removed on drop.
/// Shared with the local summary path (`commands::transcription::run_local_llm`).
pub(crate) struct PromptFile(pub(crate) PathBuf);

impl PromptFile {
    pub(crate) fn create(text: &str) -> Result<Self, String> {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("mojiroku-prompt-{}.txt", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|_| "translation.input_file")?;
        let owned = Self(path);
        file.write_all(text.as_bytes())
            .map_err(|_| "translation.input_file")?;
        Ok(owned)
    }
}

impl Drop for PromptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Kills the child on drop unless it has already terminated (set the inner value to `None` then).
pub(crate) struct ChildGuard(pub(crate) Option<CommandChild>);
impl ChildGuard {
    pub(crate) fn kill(&mut self) {
        if let Some(child) = self.0.take() {
            let _ = child.kill();
        }
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

pub(crate) async fn run_child(
    command: Command,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<String, String> {
    if cancel.is_cancelled() {
        return Err("translation.cancelled".into());
    }
    let (mut events, child) = command
        .set_raw_out(true)
        .spawn()
        .map_err(|_| "translation.start_failed")?;
    let mut child = ChildGuard(Some(child));
    let mut output = Vec::new();
    let mut failure: Option<&str> = None;
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = cancel.cancelled(), if failure.is_none() => {
                failure = Some("translation.cancelled");
                child.kill();
            }
            _ = &mut deadline, if failure.is_none() => {
                failure = Some("translation.timeout");
                child.kill();
            }
            event = events.recv() => {
                match event {
                    Some(CommandEvent::Stdout(bytes)) => {
                        if output.len() + bytes.len() > MAX_OUTPUT_BYTES {
                            failure = Some("translation.output_limit");
                            child.kill();
                        } else if failure.is_none() { output.extend(bytes); }
                    }
                    Some(CommandEvent::Stderr(_)) => {}
                    Some(CommandEvent::Error(_)) => { failure = Some("translation.failed"); child.kill(); }
                    Some(CommandEvent::Terminated(status)) => {
                        child.0 = None;
                        // Terminated arrives after pipe readers finish and the process is reaped.
                        // Only then may the caller release the heavy-job permit.
                        if let Some(error) = failure { return Err(error.into()); }
                        if status.code != Some(0) { return Err("translation.failed".into()); }
                        let text = String::from_utf8(output).map_err(|_| "translation.invalid_output")?;
                        if text.trim().is_empty() { return Err("translation.empty_output".into()); }
                        return Ok(text.trim().to_string());
                    }
                    None => return Err(failure.unwrap_or("translation.failed").into()),
                    _ => {}
                }
            }
        }
    }
}

#[tauri::command]
pub(crate) async fn translate_live_line(
    app: AppHandle,
    state: State<'_, LiveTranslationState>,
    epoch: u64,
    request_id: u64,
    text: String,
) -> Result<TranslationResult, String> {
    if text.trim().is_empty() || text.len() > MAX_SOURCE_BYTES {
        return Err("translation.input_too_long".into());
    }
    let session = crate::live_stt::session_id(&app.state::<crate::live_stt::LiveSttState>())
        .ok_or("translation.cancelled")?;
    let (activity, cancel) = state
        .control
        .lock()
        .unwrap()
        .request(epoch, request_id, &session)?;
    let _work = tokio::select! { _ = cancel.cancelled() => return Err("translation.cancelled".into()), work = state.work.lock() => work };
    let event = format!("translation://progress/{epoch}/{request_id}");
    let models = crate::commands::resolve_models_dir(&app)?;
    let path = models.join(mojiroku_core::models::TRANSLATION_MODEL_FILE);
    let stamp = model_stamp(&path);
    let verified = stamp.is_some() && *state.verified_model.lock().unwrap() == stamp;
    let model_path = if verified {
        path
    } else {
        let path = crate::model_downloads::ensure_download(
            &app,
            mojiroku_core::models::TRANSLATION_MODEL_FILE,
            &cancel,
            |done, total| progress(&app, &event, "download", done, total),
        )
        .await?;
        if activity.cancel.is_cancelled() {
            return Err("translation.cancelled".into());
        }
        *state.verified_model.lock().unwrap() = model_stamp(&path);
        path
    };
    if cancel.is_cancelled() {
        return Err("translation.cancelled".into());
    }
    let permit = tokio::select! {
        _ = cancel.cancelled() => return Err("translation.cancelled".into()),
        permit = crate::commands::acquire_heavy_job(&app, &event) => permit,
    };
    if cancel.is_cancelled() {
        return Err("translation.cancelled".into());
    }
    let file = PromptFile::create(&text)?;
    progress(&app, &event, "translate", 0, None);
    let started = Instant::now();
    let command = app
        .shell()
        .sidecar("mojiroku-llm")
        .map_err(|_| "translation.start_failed")?
        .args([
            "--translate".to_string(),
            model_path.to_string_lossy().into_owned(),
            file.0.to_string_lossy().into_owned(),
            activity.target.code().into(),
            "--no-think".into(),
        ]);
    let result = run_child(command, &cancel, MAX_INFERENCE_TIME).await;
    drop(permit);
    if result
        .as_ref()
        .is_err_and(|error| error != "translation.cancelled")
    {
        *state.verified_model.lock().unwrap() = None;
    }
    let translated = result?;
    if cancel.is_cancelled() {
        return Err("translation.cancelled".into());
    }
    Ok(TranslationResult {
        epoch,
        request_id,
        text: translated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
pub(crate) fn test_prompt(text: &str) -> PromptFile {
    PromptFile::create(text).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    #[test]
    fn translation_rejects_low_or_unknown_memory_before_model_work() {
        for bytes in [
            None,
            Some(0),
            Some(8 * 1024 * 1024 * 1024),
            Some(16 * 1024 * 1024 * 1024 - 1),
        ] {
            assert!(!translation_memory_supported(bytes));
        }
        assert!(translation_memory_supported(Some(16 * 1024 * 1024 * 1024)));
    }

    #[test]
    fn cancellation_before_request_registration_is_remembered() {
        let mut state = Control::default();
        let activity = state.begin("session".into(), TargetLanguage::Ja);
        state.cancel_request(activity.epoch, 1);
        assert!(state.request(activity.epoch, 1, "session").is_err());
        assert!(state.request(activity.epoch, 2, "session").is_ok());
    }

    #[test]
    fn old_stop_and_results_cannot_cancel_a_new_target_or_session() {
        let mut state = Control::default();
        let first = state.begin("one".into(), TargetLanguage::Ja);
        let (_, token) = state.request(first.epoch, 1, "one").unwrap();
        let second = state.begin("two".into(), TargetLanguage::En);
        assert!(token.is_cancelled());
        state.end(first.epoch);
        state.end_session("one");
        assert!(state.request(second.epoch, 1, "two").is_ok());
        assert!(state.request(first.epoch, 2, "one").is_err());
    }

    #[test]
    fn prompt_files_are_private_and_removed() {
        let prompt = PromptFile::create("test caption").unwrap();
        let path = prompt.0.clone();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "test caption");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(prompt);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn live_inference_and_translation_share_one_slot() {
        let live = crate::commands::try_acquire_live_job().unwrap();
        assert!(crate::commands::try_acquire_live_job().is_none());
        let waiting = crate::commands::acquire_heavy_job_permit();
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        drop(live);
        let translation = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap();
        assert!(crate::commands::try_acquire_live_job().is_none());
        drop(translation);
        assert!(crate::commands::try_acquire_live_job().is_some());
    }

    #[tokio::test]
    async fn cancellation_kills_and_reaps_the_child_before_returning() {
        let app = test_app();
        let marker = std::env::temp_dir().join(format!("mojiroku-child-{}", uuid::Uuid::new_v4()));
        let command = app.shell().command("/bin/sh").args([
            "-c",
            "echo $$ > \"$1\"; exec sleep 30",
            "sh",
            marker.to_str().unwrap(),
        ]);
        let token = CancellationToken::new();
        let cancellation = token.clone();
        let task =
            tokio::spawn(async move { run_child(command, &token, Duration::from_secs(60)).await });
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if std::fs::read_to_string(&marker).is_ok_and(|s| !s.trim().is_empty()) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let pid = std::fs::read_to_string(&marker).unwrap();
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err(), "translation.cancelled");
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap();
        assert!(
            !alive.status.success(),
            "the child must be reaped before its permit is released"
        );
        std::fs::remove_file(marker).unwrap();
    }

    #[tokio::test]
    async fn output_and_timeout_are_bounded() {
        let app = test_app();
        let token = CancellationToken::new();
        let result = run_child(
            app.shell().command("/bin/sh").args(["-c", "printf hello"]),
            &token,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(result, "hello");
        let result = run_child(
            app.shell()
                .command("/usr/bin/awk")
                .args(["BEGIN { printf \"%20000s\", \"x\" }"]),
            &token,
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(result.unwrap_err(), "translation.output_limit");
        let result = run_child(
            app.shell().command("/bin/sh").args(["-c", "exec sleep 30"]),
            &token,
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(result.unwrap_err(), "translation.timeout");
    }
}
