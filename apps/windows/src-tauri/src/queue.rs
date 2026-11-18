//! Background send queue.
//!
//! The popover closes on focus loss, so an upload cannot belong to it (D19). A job
//! waits [`SEND_DELAY`] (the "Envoyé · Annuler" window, F16) before anything is sent;
//! cancelling during that window means the item never leaves the device.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::oneshot;

use crate::core_api::{ProgressFn, TomiCore};
use crate::model::{now_ms, ItemDraft};
use crate::paths::PathRegistry;

pub const SEND_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum JobState {
    /// Waiting out the cancellation window; `sendAt` is a Unix ms timestamp.
    #[serde(rename_all = "camelCase")]
    Pending {
        send_at: i64,
    },
    #[serde(rename_all = "camelCase")]
    Sending {
        sent_bytes: u64,
        total_bytes: u64,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        item_id: String,
    },
    /// Kept by the core but not accepted by the relay yet (offline): the item is in the
    /// feed as "En attente" and goes out with the next synchronisation, no retry needed.
    #[serde(rename_all = "camelCase")]
    Queued {
        item_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        error: String,
    },
    Cancelled,
}

impl JobState {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Queued { .. } | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub secret: bool,
    pub has_text: bool,
    pub has_url: bool,
    pub file_count: usize,
    #[serde(flatten)]
    pub state: JobState,
}

pub trait JobSink: Send + Sync + 'static {
    fn job_changed(&self, job: &JobView);
}

struct Job {
    view: JobView,
    draft: ItemDraft,
    cancel: Option<oneshot::Sender<()>>,
}

struct Inner {
    core: Arc<dyn TomiCore>,
    sink: Arc<dyn JobSink>,
    delay: Duration,
    /// Knows which files are temporary copies staged by the app: those, and only those,
    /// are deleted once the job ends (sent or cancelled).
    paths: Arc<PathRegistry>,
    jobs: Mutex<HashMap<String, Job>>,
}

#[derive(Clone)]
pub struct SendQueue {
    inner: Arc<Inner>,
}

impl SendQueue {
    pub fn new(
        core: Arc<dyn TomiCore>,
        sink: Arc<dyn JobSink>,
        delay: Duration,
        paths: Arc<PathRegistry>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                core,
                sink,
                delay,
                paths,
                jobs: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.inner.jobs.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_state(&self, id: &str, state: JobState) {
        let view = {
            let mut jobs = self.jobs();
            let Some(job) = jobs.get_mut(id) else { return };
            job.view.state = state;
            let view = job.view.clone();
            if view.state.is_final() {
                jobs.remove(id);
            }
            view
        };
        self.inner.sink.job_changed(&view);
    }

    /// Queues a draft. Nothing is sent before the delay has elapsed.
    pub fn enqueue(&self, mut draft: ItemDraft) -> JobView {
        let id = uuid::Uuid::new_v4().to_string();
        // The item takes the id of the job: what the core stores can be told from it.
        draft.id = Some(id.clone());
        let view = JobView {
            id: id.clone(),
            secret: draft.secret,
            has_text: draft.text.is_some(),
            has_url: draft.url.is_some(),
            file_count: draft.attachments.len(),
            state: JobState::Pending {
                send_at: now_ms() + self.inner.delay.as_millis() as i64,
            },
        };
        self.jobs().insert(
            id.clone(),
            Job {
                view: view.clone(),
                draft,
                cancel: None,
            },
        );
        self.inner.sink.job_changed(&view);
        self.spawn(id);
        view
    }

    fn spawn(&self, id: String) {
        let (tx, rx) = oneshot::channel();
        let draft = {
            let mut jobs = self.jobs();
            let Some(job) = jobs.get_mut(&id) else { return };
            job.cancel = Some(tx);
            job.draft.clone()
        };
        let queue = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(queue.inner.delay) => {}
                _ = rx => {
                    queue.cleanup(&draft);
                    queue.set_state(&id, JobState::Cancelled);
                    return;
                }
            }
            // Past this point the job can no longer be cancelled.
            if let Some(job) = queue.jobs().get_mut(&id) {
                job.cancel = None;
            }
            let total = draft.total_attachment_bytes();
            queue.set_state(
                &id,
                JobState::Sending {
                    sent_bytes: 0,
                    total_bytes: total,
                },
            );
            let progress: ProgressFn = {
                let q = queue.clone();
                let id = id.clone();
                Arc::new(move |p| {
                    q.set_state(
                        &id,
                        JobState::Sending {
                            sent_bytes: p.sent_bytes,
                            total_bytes: p.total_bytes,
                        },
                    )
                })
            };
            match queue.inner.core.send_item(draft.clone(), progress).await {
                Ok(out) => {
                    // The core has read and encrypted the files: the staged copies go.
                    queue.cleanup(&draft);
                    let item_id = out.item_id;
                    let state = if out.queued {
                        JobState::Queued { item_id }
                    } else {
                        JobState::Done { item_id }
                    };
                    queue.set_state(&id, state);
                }
                Err(e) => {
                    // Keep the job (and the staged files) so that it can be retried.
                    let error = crate::error::AppError::from(e).code;
                    queue.set_state(&id, JobState::Failed { error });
                }
            }
        });
    }

    /// Cancels a job still inside its delay window. Returns false when it is too late.
    pub fn cancel(&self, id: &str) -> bool {
        let tx = {
            let mut jobs = self.jobs();
            jobs.get_mut(id).and_then(|j| j.cancel.take())
        };
        match tx {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// Re-queues a failed job (the cancellation window is applied again).
    pub fn retry(&self, id: &str) -> bool {
        {
            let mut jobs = self.jobs();
            match jobs.get_mut(id) {
                Some(job) if matches!(job.view.state, JobState::Failed { .. }) => {
                    // A failed attempt leaves nothing in the core and remembers its item id
                    // as erased: the new attempt gets a new one.
                    job.draft.id = Some(uuid::Uuid::new_v4().to_string());
                    job.view.state = JobState::Pending {
                        send_at: now_ms() + self.inner.delay.as_millis() as i64,
                    };
                    let view = job.view.clone();
                    drop(jobs);
                    self.inner.sink.job_changed(&view);
                }
                _ => return false,
            }
        }
        self.spawn(id.to_string());
        true
    }

    /// Drops a failed job and its staged files.
    pub fn dismiss(&self, id: &str) {
        let job = self.jobs().remove(id);
        if let Some(job) = job {
            self.cleanup(&job.draft);
            let mut view = job.view;
            view.state = JobState::Cancelled;
            self.inner.sink.job_changed(&view);
        }
    }

    pub fn active(&self) -> Vec<JobView> {
        let mut v: Vec<_> = self.jobs().values().map(|j| j.view.clone()).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Removes the staged copies of a finished job. The paths come from the draft, but
    /// whether one may be deleted is decided by the registry (files the app staged
    /// itself, canonical path inside the staging directory, no `..`), never by a
    /// lexical `starts_with` on what the front end sent. Files the user owns are not
    /// forgotten but are never deleted.
    fn cleanup(&self, draft: &ItemDraft) {
        for a in &draft.attachments {
            if let Some(p) = a.path.as_deref() {
                let p = std::path::Path::new(p);
                if !self.inner.paths.delete_staged(p) {
                    self.inner.paths.forget_user_file(p);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_api::{CoreError, SendOutcome};
    use crate::fake_core::FakeCore;
    use crate::model::{Attachment, ReadPolicy, TextBody};

    #[derive(Default)]
    struct Collect(Mutex<Vec<JobView>>);
    impl JobSink for Collect {
        fn job_changed(&self, job: &JobView) {
            self.0.lock().unwrap().push(job.clone());
        }
    }

    fn draft(staged: Option<&std::path::Path>) -> ItemDraft {
        ItemDraft {
            id: None,
            title: None,
            text: Some(TextBody {
                plain: "salut".into(),
                html: None,
                markdown: None,
            }),
            url: None,
            attachments: staged
                .map(|p| {
                    vec![Attachment {
                        name: "s.bin".into(),
                        mime: "application/octet-stream".into(),
                        size: 4,
                        path: Some(p.to_string_lossy().into_owned()),
                        thumbnail: None,
                        available: true,
                        slot: None,
                    }]
                })
                .unwrap_or_default(),
            secret: false,
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            source_format: None,
        }
    }

    struct Env {
        dir: tempfile::TempDir,
        core: Arc<FakeCore>,
        sink: Arc<Collect>,
        q: SendQueue,
        paths: Arc<PathRegistry>,
    }

    fn setup(delay_ms: u64) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let paths = Arc::new(PathRegistry::new(&staging));
        let core = Arc::new(FakeCore::new(dir.path().join("store")).without_delay());
        let sink = Arc::new(Collect::default());
        let q = SendQueue::new(
            core.clone(),
            sink.clone(),
            Duration::from_millis(delay_ms),
            paths.clone(),
        );
        Env {
            dir,
            core,
            sink,
            q,
            paths,
        }
    }

    /// A core whose `send_item` follows a script (what the real core answers offline, or
    /// when the relay refuses); everything else is the in-memory fake.
    struct Scripted {
        inner: FakeCore,
        script: Mutex<Vec<Result<SendOutcome, CoreError>>>,
        /// The item id each attempt was given.
        ids: Mutex<Vec<Option<String>>>,
    }

    impl Scripted {
        fn new(dir: &std::path::Path, script: Vec<Result<SendOutcome, CoreError>>) -> Arc<Self> {
            Arc::new(Self {
                inner: FakeCore::new(dir.join("store")).without_delay(),
                script: Mutex::new(script),
                ids: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl TomiCore for Scripted {
        fn status(&self) -> crate::core_api::CoreStatus {
            self.inner.status()
        }
        fn this_device(&self) -> crate::model::Device {
            self.inner.this_device()
        }
        async fn send_item(
            &self,
            draft: ItemDraft,
            _progress: ProgressFn,
        ) -> Result<SendOutcome, CoreError> {
            self.ids.lock().unwrap().push(draft.id.clone());
            self.script.lock().unwrap().remove(0)
        }
        async fn list_items(&self) -> Result<Vec<crate::model::Item>, CoreError> {
            self.inner.list_items().await
        }
        async fn get_item(&self, id: &str) -> Result<crate::model::Item, CoreError> {
            self.inner.get_item(id).await
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::core_api::CoreEvent> {
            self.inner.subscribe()
        }
        async fn delete_local(&self, id: &str) -> Result<(), CoreError> {
            self.inner.delete_local(id).await
        }
        async fn delete_everywhere(&self, id: &str) -> Result<(), CoreError> {
            self.inner.delete_everywhere(id).await
        }
        async fn mark_read(&self, id: &str) -> Result<(), CoreError> {
            self.inner.mark_read(id).await
        }
        async fn pin(&self, id: &str) -> Result<(), CoreError> {
            self.inner.pin(id).await
        }
        async fn unpin(&self, id: &str) -> Result<(), CoreError> {
            self.inner.unpin(id).await
        }
        async fn archive(&self, id: &str) -> Result<(), CoreError> {
            self.inner.archive(id).await
        }
        async fn unarchive(&self, id: &str) -> Result<(), CoreError> {
            self.inner.unarchive(id).await
        }
        async fn devices(&self) -> Result<Vec<crate::model::Device>, CoreError> {
            self.inner.devices().await
        }
        async fn rename_device(&self, id: &str, name: &str) -> Result<(), CoreError> {
            self.inner.rename_device(id, name).await
        }
        async fn revoke_device(&self, id: &str) -> Result<(), CoreError> {
            self.inner.revoke_device(id).await
        }
        async fn attachment_path(
            &self,
            id: &str,
            slot: usize,
        ) -> Result<std::path::PathBuf, CoreError> {
            self.inner.attachment_path(id, slot).await
        }
        async fn delete_attachment(
            &self,
            id: &str,
            slot: usize,
            everywhere: bool,
        ) -> Result<crate::model::AttachmentRemoval, CoreError> {
            self.inner.delete_attachment(id, slot, everywhere).await
        }
        async fn supports_attachment_removal(&self) -> bool {
            self.inner.supports_attachment_removal().await
        }
        async fn transfers(&self) -> Result<Vec<crate::model::Transfer>, CoreError> {
            self.inner.transfers().await
        }
        async fn send_history(&self, device_id: &str) -> Result<(), CoreError> {
            self.inner.send_history(device_id).await
        }
        async fn answer_history(&self, transfer_id: &str, accept: bool) -> Result<(), CoreError> {
            self.inner.answer_history(transfer_id, accept).await
        }
        async fn cancel_transfer(&self, transfer_id: &str) -> Result<(), CoreError> {
            self.inner.cancel_transfer(transfer_id).await
        }
        async fn pairing_options(&self) -> Result<crate::core_api::PairingOptions, CoreError> {
            self.inner.pairing_options().await
        }
        fn pairing_view(&self) -> crate::core_api::PairingView {
            self.inner.pairing_view()
        }
        async fn pairing_create_space_at(
            &self,
            install_token: &str,
            device_name: Option<&str>,
            relay_url: Option<&str>,
        ) -> Result<(), CoreError> {
            self.inner
                .pairing_create_space_at(install_token, device_name, relay_url)
                .await
        }
        async fn pairing_invite(&self) -> Result<crate::core_api::PairingView, CoreError> {
            self.inner.pairing_invite().await
        }
        async fn pairing_join_at(
            &self,
            code_or_link: &str,
            device_name: Option<&str>,
            relay_url: Option<&str>,
        ) -> Result<crate::core_api::PairingView, CoreError> {
            self.inner
                .pairing_join_at(code_or_link, device_name, relay_url)
                .await
        }
        fn pairing_confirm(&self, import_history: bool) -> Result<(), CoreError> {
            self.inner.pairing_confirm(import_history)
        }
        fn pairing_cancel(&self) {
            self.inner.pairing_cancel();
        }
        fn pairing_leave(&self) {
            self.inner.pairing_leave();
        }
        fn pairing_dismiss(&self) {
            self.inner.pairing_dismiss();
        }
        async fn sync(&self) -> Result<(), CoreError> {
            self.inner.sync().await
        }
        async fn purge_plaintext_cache(&self) -> Result<(), CoreError> {
            self.inner.purge_plaintext_cache().await
        }
        fn departure_available(&self) -> bool {
            self.inner.departure_available()
        }
        async fn leave_space(&self) -> Result<crate::core_api::DepartureOutcome, CoreError> {
            self.inner.leave_space().await
        }
        async fn delete_space(&self) -> Result<crate::core_api::DepartureOutcome, CoreError> {
            self.inner.delete_space().await
        }
        async fn retry_departure(&self) -> Result<crate::core_api::DepartureOutcome, CoreError> {
            self.inner.retry_departure().await
        }
        async fn continue_departure(&self) -> Result<crate::core_api::DepartureOutcome, CoreError> {
            self.inner.continue_departure().await
        }
        async fn cancel_departure(&self) -> Result<bool, CoreError> {
            self.inner.cancel_departure().await
        }
        fn departure(&self) -> Option<crate::core_api::DepartureView> {
            self.inner.departure()
        }
        fn can_erase_without_proof(&self) -> bool {
            self.inner.can_erase_without_proof()
        }
        async fn erase_without_proof(&self) -> Result<(), CoreError> {
            self.inner.erase_without_proof().await
        }
        fn shutdown(&self) {}
    }

    fn scripted_queue(
        dir: &std::path::Path,
        script: Vec<Result<SendOutcome, CoreError>>,
    ) -> (Arc<Scripted>, Arc<Collect>, SendQueue) {
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let core = Scripted::new(dir, script);
        let sink = Arc::new(Collect::default());
        let q = SendQueue::new(
            core.clone(),
            sink.clone(),
            Duration::from_millis(20),
            Arc::new(PathRegistry::new(&staging)),
        );
        (core, sink, q)
    }

    fn outcome(id: &str, queued: bool) -> Result<SendOutcome, CoreError> {
        Ok(SendOutcome {
            item_id: id.into(),
            queued,
        })
    }

    #[tokio::test]
    async fn the_item_takes_the_id_of_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let (core, sink, q) = scripted_queue(dir.path(), vec![outcome("i1", false)]);
        let job = q.enqueue(draft(None));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(*core.ids.lock().unwrap(), vec![Some(job.id.clone())]);
        let last = sink.0.lock().unwrap().last().unwrap().state.clone();
        assert_eq!(
            last,
            JobState::Done {
                item_id: "i1".into()
            }
        );
    }

    #[tokio::test]
    async fn an_item_kept_in_the_outbox_ends_as_queued_not_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let (_core, sink, q) = scripted_queue(dir.path(), vec![outcome("i1", true)]);
        q.enqueue(draft(None));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let last = sink.0.lock().unwrap().last().unwrap().state.clone();
        assert_eq!(
            last,
            JobState::Queued {
                item_id: "i1".into()
            }
        );
        assert!(last.is_final());
        assert!(q.active().is_empty(), "nothing to retry: the core has it");
    }

    #[tokio::test]
    async fn a_retry_gets_a_new_item_id() {
        // A failed attempt leaves nothing in the core and remembers its id as erased: the
        // same id would find nothing to send.
        let dir = tempfile::tempdir().unwrap();
        let (core, sink, q) = scripted_queue(
            dir.path(),
            vec![
                Err(CoreError::Rejected("send_failed".into())),
                outcome("i2", false),
            ],
        );
        let job = q.enqueue(draft(None));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(matches!(
            sink.0.lock().unwrap().last().unwrap().state,
            JobState::Failed { .. }
        ));
        assert!(q.retry(&job.id));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let ids = core.ids.lock().unwrap().clone();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], Some(job.id.clone()));
        assert!(ids[1].is_some() && ids[1] != ids[0], "{ids:?}");
        assert!(matches!(
            sink.0.lock().unwrap().last().unwrap().state,
            JobState::Done { .. }
        ));
    }
    /// A real staged file, as `stage_blob` would leave it.
    fn stage(env: &Env, name: &str) -> std::path::PathBuf {
        let dir = env.paths.staging_dir().join("job1");
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join(name);
        std::fs::write(&staged, b"data").unwrap();
        assert!(env.paths.register_staged(&staged));
        staged
    }

    #[tokio::test]
    async fn cancel_inside_the_window_sends_nothing() {
        let env = setup(300);
        let staged = stage(&env, "s.bin");

        let job = env.q.enqueue(draft(Some(&staged)));
        assert!(matches!(job.state, JobState::Pending { .. }));
        assert!(env.q.cancel(&job.id));
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(env.core.list_items().await.unwrap().is_empty());
        assert_eq!(
            env.sink.0.lock().unwrap().last().unwrap().state,
            JobState::Cancelled
        );
        assert!(!staged.exists(), "staged copy must be removed on cancel");
        assert!(env.q.active().is_empty());
    }

    #[tokio::test]
    async fn nothing_is_sent_before_the_delay() {
        let env = setup(400);
        env.q.enqueue(draft(None));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(env.core.list_items().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn job_is_sent_after_the_delay_and_cannot_be_cancelled_then() {
        let env = setup(50);
        let job = env.q.enqueue(draft(None));
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert_eq!(env.core.list_items().await.unwrap().len(), 1);
        let states = env.sink.0.lock().unwrap().clone();
        assert!(matches!(
            states.first().unwrap().state,
            JobState::Pending { .. }
        ));
        assert!(states
            .iter()
            .any(|v| matches!(v.state, JobState::Sending { .. })));
        assert!(matches!(
            states.last().unwrap().state,
            JobState::Done { .. }
        ));
        assert!(!env.q.cancel(&job.id));
    }

    #[tokio::test]
    async fn staged_files_are_removed_after_a_successful_send() {
        let env = setup(20);
        let staged = stage(&env, "s.bin");
        env.q.enqueue(draft(Some(&staged)));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!staged.exists());
        assert!(!staged.parent().unwrap().exists(), "per-file folder too");
        assert!(env.paths.staging_dir().exists());
    }

    #[tokio::test]
    async fn user_files_outside_staging_are_never_deleted() {
        let env = setup(20);
        let user_file = env.dir.path().join("mine.bin");
        std::fs::write(&user_file, b"data").unwrap();
        env.paths.register_user_file(&user_file);
        env.q.enqueue(draft(Some(&user_file)));

        // Same file named through `staging\..\mine.bin`: lexically under staging.
        let via_dotdot = env.paths.staging_dir().join("..").join("mine.bin");
        assert!(via_dotdot.starts_with(env.paths.staging_dir()));
        env.q.enqueue(draft(Some(&via_dotdot)));

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(user_file.exists());
    }

    /// `staging\..\..\X` starts with `staging` lexically but is outside it.
    #[tokio::test]
    async fn dotdot_paths_out_of_staging_are_never_deleted() {
        let env = setup(20);
        // The victim is two levels above the staging directory's child: reachable by
        // `staging/job1/../../victim.txt` (= dir/victim.txt) and `staging/../victim.txt`.
        let victim = env.dir.path().join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let real_staged = stage(&env, "ok.bin");
        let sneaky_a = env
            .paths
            .staging_dir()
            .join("job1")
            .join("..")
            .join("..")
            .join("victim.txt");
        let sneaky_b = env.paths.staging_dir().join("..").join("victim.txt");
        assert!(
            sneaky_a.starts_with(env.paths.staging_dir()),
            "lexical trap"
        );

        let mut d = draft(Some(&sneaky_a));
        d.attachments
            .push(draft(Some(&sneaky_b)).attachments.remove(0));
        d.attachments
            .push(draft(Some(&real_staged)).attachments.remove(0));
        // FakeCore cannot copy a `..` path? It can (the OS resolves it): the point is
        // only that cleanup must not delete it afterwards.
        env.q.enqueue(d);
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(victim.exists(), "a `..` path must never be deleted");
        assert!(!real_staged.exists(), "the genuine staged file is cleaned");
    }

    #[tokio::test]
    async fn cancelling_with_dotdot_paths_keeps_the_target_too() {
        let env = setup(300);
        let victim = env.dir.path().join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let sneaky = env.paths.staging_dir().join("..").join("victim.txt");
        let job = env.q.enqueue(draft(Some(&sneaky)));
        assert!(env.q.cancel(&job.id));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(victim.exists());
    }

    #[tokio::test]
    async fn a_file_in_staging_the_app_did_not_stage_is_kept() {
        let env = setup(20);
        let planted = env.paths.staging_dir().join("planted.txt");
        std::fs::write(&planted, b"x").unwrap();
        env.q.enqueue(draft(Some(&planted)));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(planted.exists(), "only registered staged files are deleted");
    }
}
