use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mezon_client::transport::{
    ApiMessage, ChannelLinkMeta, ContentToken, OutgoingEmoji, OutgoingHashtag, OutgoingMention,
};
use mezon_client::{AttachmentUploadOutcome, ResumableUpload};
use serde::{Deserialize, Serialize};

use crate::ids::UserId;
use crate::presign::PRESIGN_PENDING_MAX_AGE_SEC;

const PRESIGNED_URL_LIFETIME_SEC: i64 = 15 * 60;
const RETENTION_SEC: i64 = 7 * 24 * 60 * 60;
const MAX_FAILED_UPLOADS: usize = 200;
const MAX_SYNC_FAILURES: u32 = 3;
const FILE_NAME: &str = "upload_jobs.json";

pub type UploadJobId = (i64, i64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadJob {
    pub user_id: UserId,
    pub clan_id: i64,
    pub channel_id: i64,
    pub parent_channel_id: i64,
    pub topic_id: i64,
    pub message_id: i64,
    pub mode: i32,
    pub is_public: bool,
    pub content: String,
    pub mentions: Vec<OutgoingMention>,
    pub hashtags: Vec<OutgoingHashtag>,
    #[serde(default)]
    pub hashtag_channels: Vec<ChannelLinkMeta>,
    pub emojis: Vec<OutgoingEmoji>,
    pub create_time_seconds: u32,
    pub started_at: i64,
    pub finished: Vec<String>,
    pub pending: Vec<PendingUpload>,
    #[serde(default)]
    pub sync_failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpload {
    pub key: String,
    pub upload: ResumableUpload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailedUpload {
    pub user_id: UserId,
    pub key: String,
    pub path: PathBuf,
    pub failed_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedUploads {
    #[serde(default)]
    pub jobs: Vec<UploadJob>,
    #[serde(default)]
    pub failed: Vec<FailedUpload>,
}

#[derive(Serialize)]
struct SavedUploadsRef<'a> {
    jobs: Vec<&'a UploadJob>,
    failed: Vec<&'a FailedUpload>,
}

impl UploadJob {
    pub fn id(&self) -> UploadJobId {
        (self.channel_id, self.message_id)
    }

    pub fn is_topic(&self) -> bool {
        self.topic_id != 0
    }

    pub fn pending_keys(&self) -> Vec<String> {
        self.pending.iter().map(|p| p.key.clone()).collect()
    }

    pub fn local_sources(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.pending
            .iter()
            .map(|p| (p.key.as_str(), p.upload.local_path()))
    }

    pub fn resumable_at(&self, now: i64) -> bool {
        now - self.started_at < PRESIGN_PENDING_MAX_AGE_SEC
    }

    pub fn urls_valid_at(&self, now: i64) -> bool {
        now - self.started_at < PRESIGNED_URL_LIFETIME_SEC
    }

    pub fn worth_keeping_at(&self, now: i64) -> bool {
        now - self.started_at < RETENTION_SEC
    }

    pub fn note_sync_failure(&mut self) -> bool {
        self.sync_failures += 1;
        self.sync_failures < MAX_SYNC_FAILURES
    }

    pub fn has_upload_urls(&self) -> bool {
        self.pending.iter().any(|p| p.upload.has_urls())
    }

    pub fn drop_upload_urls(&mut self) {
        for pending in self.pending.iter_mut() {
            pending.upload = pending.upload.without_urls();
        }
    }

    pub fn failed_upload(&self, key: &str, now: i64) -> Option<FailedUpload> {
        self.pending
            .iter()
            .find(|p| p.key == key)
            .map(|p| FailedUpload {
                user_id: self.user_id,
                key: p.key.clone(),
                path: p.upload.local_path().to_path_buf(),
                failed_at: now,
            })
    }

    pub fn record(&mut self, outcome: &AttachmentUploadOutcome) {
        let (key, uploaded) = match outcome {
            AttachmentUploadOutcome::Uploaded(key) => (key, true),
            AttachmentUploadOutcome::Failed(key) => (key, false),
        };
        self.pending.retain(|p| &p.key != key);
        if uploaded && !self.finished.contains(key) {
            self.finished.push(key.clone());
        }
    }

    pub fn adopt_current_message(&mut self, message: &ApiMessage) {
        let tokens = &message.content_tokens;
        self.content = tokens.t.clone();
        self.mentions = message
            .entity_mentions
            .iter()
            .map(|m| OutgoingMention {
                user_id: id_or_empty(m.user_id),
                role_id: id_or_empty(m.role_id),
                username: m.username.clone(),
                s: m.s,
                e: m.e,
            })
            .collect();
        self.hashtags = tokens
            .hg
            .iter()
            .map(|t| OutgoingHashtag {
                channel_id: t.channel_id.clone().unwrap_or_default(),
                s: token_start(t),
                e: token_end(t),
            })
            .collect();
        self.hashtag_channels = channel_link_metas(tokens.hg.iter().chain(&tokens.mk));
        self.emojis = tokens
            .ej
            .iter()
            .map(|t| OutgoingEmoji {
                emoji_id: t.emojiid.clone().unwrap_or_default(),
                s: token_start(t),
                e: token_end(t),
            })
            .collect();
    }
}

fn channel_link_metas<'a>(tokens: impl Iterator<Item = &'a ContentToken>) -> Vec<ChannelLinkMeta> {
    let mut metas: Vec<ChannelLinkMeta> = Vec::new();
    for token in tokens {
        let (Some(channel_id), Some(clan_id), Some(channel_label), Some(channel_type)) = (
            token.channel_id.as_ref(),
            token.clan_id.as_ref(),
            token.channel_label.as_ref(),
            token.channel_type,
        ) else {
            continue;
        };
        if metas.iter().any(|meta| &meta.channel_id == channel_id) {
            continue;
        }
        metas.push(ChannelLinkMeta {
            channel_id: channel_id.clone(),
            channel_label: channel_label.clone(),
            clan_id: clan_id.clone(),
            parent_id: token.parent_id.clone(),
            channel_type: u32::try_from(channel_type).unwrap_or_default(),
            private: token.channel_private.is_some_and(|private| private != 0),
        });
    }
    metas
}

fn id_or_empty(id: i64) -> String {
    if id == 0 {
        String::new()
    } else {
        id.to_string()
    }
}

fn token_start(token: &ContentToken) -> i32 {
    token.s.unwrap_or_default() as i32
}

fn token_end(token: &ContentToken) -> i32 {
    token.e.unwrap_or_default() as i32
}

#[derive(Debug, PartialEq, Eq)]
pub enum MessagePresence {
    Present,
    Deleted,
    Unknown,
}

pub fn message_presence(message_id: i64, page: &[ApiMessage]) -> MessagePresence {
    if page.iter().any(|m| m.message_id == message_id) {
        return MessagePresence::Present;
    }
    let older = page.iter().any(|m| m.message_id < message_id);
    let newer = page.iter().any(|m| m.message_id > message_id);
    if older && newer {
        MessagePresence::Deleted
    } else {
        MessagePresence::Unknown
    }
}

#[derive(Debug, Default)]
pub struct RestorePlan {
    pub replaced: Vec<UploadJobId>,
    pub keep: Vec<UploadJob>,
    pub resume: Vec<UploadJob>,
    pub expired: Vec<UploadJob>,
}

pub fn plan_restore(
    saved: Vec<UploadJob>,
    user_id: UserId,
    now: i64,
    is_running: impl Fn(&UploadJob) -> bool,
) -> RestorePlan {
    let mut plan = RestorePlan::default();
    for job in saved {
        if is_running(&job) || !job.worth_keeping_at(now) {
            continue;
        }
        plan.replaced.push(job.id());
        if job.user_id != user_id {
            plan.keep.push(job);
        } else if job.resumable_at(now) {
            plan.resume.push(job);
        } else {
            plan.expired.push(job);
        }
    }
    plan
}

pub fn prune(jobs: &mut Vec<Arc<UploadJob>>, now: i64, is_running: impl Fn(&UploadJob) -> bool) {
    jobs.retain(|job| job.worth_keeping_at(now) || is_running(job));
    for job in jobs.iter_mut() {
        if !job.urls_valid_at(now) && !is_running(job) && job.has_upload_urls() {
            Arc::make_mut(job).drop_upload_urls();
        }
    }
}

pub fn prune_failed(failed: &mut Vec<FailedUpload>, now: i64) -> Vec<String> {
    let mut removed = Vec::new();
    failed.retain(|f| {
        let keep = now - f.failed_at < RETENTION_SEC;
        if !keep {
            removed.push(f.key.clone());
        }
        keep
    });
    if failed.len() > MAX_FAILED_UPLOADS {
        let excess = failed.len() - MAX_FAILED_UPLOADS;
        removed.extend(failed.drain(..excess).map(|f| f.key));
    }
    removed
}

fn persistable_path(path: &Path) -> bool {
    path.to_str().is_some()
}

fn persistable<'a>(jobs: &'a [Arc<UploadJob>], failed: &'a [FailedUpload]) -> SavedUploadsRef<'a> {
    SavedUploadsRef {
        jobs: jobs
            .iter()
            .map(AsRef::as_ref)
            .filter(|job| job.local_sources().all(|(_, path)| persistable_path(path)))
            .collect(),
        failed: failed
            .iter()
            .filter(|f| persistable_path(&f.path))
            .collect(),
    }
}

fn jobs_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mezon")
        .join(FILE_NAME)
}

pub fn load() -> SavedUploads {
    if cfg!(test) {
        return SavedUploads::default();
    }
    let Ok(bytes) = std::fs::read(jobs_path()) else {
        return SavedUploads::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(%error, "saved upload jobs are unreadable; starting without them");
        SavedUploads::default()
    })
}

static LAST_WRITTEN: Mutex<u64> = Mutex::new(0);

pub fn save(jobs: &[Arc<UploadJob>], failed: &[FailedUpload], generation: u64) {
    if cfg!(test) {
        return;
    }
    let mut last_written = LAST_WRITTEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation <= *last_written {
        return;
    }
    *last_written = generation;
    let path = jobs_path();
    let saved = persistable(jobs, failed);
    if saved.jobs.is_empty() && saved.failed.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Err(error) = write_private(&path, &saved) {
        tracing::warn!(%error, "could not save upload jobs");
    }
}

fn write_private(path: &Path, saved: &SavedUploadsRef<'_>) -> anyhow::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(&serde_json::to_vec(saved)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(started_at: i64) -> UploadJob {
        UploadJob {
            user_id: UserId(7),
            clan_id: 1,
            channel_id: 2,
            parent_channel_id: 2,
            topic_id: 0,
            message_id: 3,
            mode: 2,
            is_public: true,
            content: "clip".into(),
            mentions: Vec::new(),
            hashtags: Vec::new(),
            hashtag_channels: Vec::new(),
            emojis: Vec::new(),
            create_time_seconds: 0,
            started_at,
            finished: vec!["a".into()],
            pending: Vec::new(),
            sync_failures: 0,
        }
    }

    fn job_for(user: i64, message_id: i64, started_at: i64) -> UploadJob {
        UploadJob {
            user_id: UserId(user),
            message_id,
            ..job(started_at)
        }
    }

    fn single_upload(path: &str) -> ResumableUpload {
        serde_json::from_value(serde_json::json!({
            "plan": {"Single": {"put_url": "https://s3.example/put", "path": path, "content_type": "image/png"}}
        }))
        .expect("plan")
    }

    fn message(id: i64) -> ApiMessage {
        ApiMessage {
            message_id: id,
            content: String::new(),
            content_raw: String::new(),
            content_tokens: Default::default(),
            code: 0,
            sender_id: 7,
            sender_name: String::new(),
            avatar: String::new(),
            create_time: 0,
            update_time: 0,
            hide_editted: false,
            attachments: vec![],
            references: vec![],
            reactions: vec![],
            entity_mentions: vec![],
            topic_id: 0,
        }
    }

    #[test]
    fn a_job_is_identified_by_its_bucket_and_message() {
        let mut other_channel = job(0);
        other_channel.channel_id = 9;
        assert_ne!(job(0).id(), other_channel.id());
        assert_eq!(job(0).id(), (2, 3));
    }

    #[test]
    fn a_job_stops_retrying_its_patch_after_three_failures() {
        let mut job = job(0);
        assert!(job.note_sync_failure());
        assert!(job.note_sync_failure());
        assert!(!job.note_sync_failure());
    }

    #[test]
    fn a_job_resumes_only_inside_the_presign_window() {
        let job = job(1_000);
        assert!(job.resumable_at(1_000 + PRESIGN_PENDING_MAX_AGE_SEC - 1));
        assert!(!job.resumable_at(1_000 + PRESIGN_PENDING_MAX_AGE_SEC));
    }

    #[test]
    fn an_upload_moves_its_key_to_finished_once() {
        let mut job = job(0);
        job.pending.push(PendingUpload {
            key: "b".into(),
            upload: single_upload("/tmp/b.png"),
        });
        job.record(&AttachmentUploadOutcome::Uploaded("b".into()));
        job.record(&AttachmentUploadOutcome::Uploaded("b".into()));
        assert!(job.pending.is_empty());
        assert_eq!(job.finished, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_failed_upload_keeps_the_local_file_to_preview() {
        let mut job = job(0);
        job.pending.push(PendingUpload {
            key: "k".into(),
            upload: single_upload("/tmp/photo.png"),
        });
        assert_eq!(
            job.failed_upload("k", 50),
            Some(FailedUpload {
                user_id: UserId(7),
                key: "k".into(),
                path: "/tmp/photo.png".into(),
                failed_at: 50,
            })
        );
        assert_eq!(job.failed_upload("missing", 50), None);
        let sources: Vec<_> = job.local_sources().collect();
        assert_eq!(sources, vec![("k", Path::new("/tmp/photo.png"))]);
    }

    #[test]
    fn restoring_resumes_fresh_jobs_and_expires_old_ones_for_a_week() {
        let now = 10 * RETENTION_SEC;
        let saved = vec![
            job_for(7, 1, now - 60),
            job_for(7, 2, now - PRESIGN_PENDING_MAX_AGE_SEC - 1),
            job_for(8, 3, now - 60),
            job_for(7, 4, now - RETENTION_SEC),
            job_for(7, 5, now - 30),
            job_for(7, 6, now - 2 * 60 * 60),
        ];
        let plan = plan_restore(saved, UserId(7), now, |job| job.message_id == 5);
        let ids = |jobs: &[UploadJob]| jobs.iter().map(|j| j.message_id).collect::<Vec<_>>();
        assert_eq!(ids(&plan.resume), vec![1]);
        assert_eq!(ids(&plan.keep), vec![3]);
        assert_eq!(ids(&plan.expired), vec![2, 6]);
        assert_eq!(plan.replaced, vec![(2, 1), (2, 2), (2, 3), (2, 6)]);
    }

    #[test]
    fn pruning_keeps_a_week_and_strips_lapsed_urls_from_idle_jobs() {
        let now = 10 * RETENTION_SEC;
        let with_upload = |message_id, started_at| {
            let mut job = job_for(7, message_id, started_at);
            job.pending.push(PendingUpload {
                key: format!("k{message_id}"),
                upload: single_upload("/tmp/x.png"),
            });
            Arc::new(job)
        };
        let mut jobs = vec![
            with_upload(1, now - 60),
            with_upload(2, now - PRESIGNED_URL_LIFETIME_SEC),
            with_upload(3, now - RETENTION_SEC),
            with_upload(4, now - RETENTION_SEC - 5),
            with_upload(5, now - PRESIGNED_URL_LIFETIME_SEC),
        ];
        prune(&mut jobs, now, |job| {
            job.message_id == 4 || job.message_id == 5
        });
        let ids: Vec<i64> = jobs.iter().map(|j| j.message_id).collect();
        assert_eq!(ids, vec![1, 2, 4, 5]);
        assert!(jobs[0].has_upload_urls());
        assert!(!jobs[1].has_upload_urls());
        assert!(jobs[3].has_upload_urls());
    }

    #[test]
    fn failed_uploads_are_kept_for_a_week_and_capped() {
        let failed = |key: &str, failed_at: i64| FailedUpload {
            user_id: UserId(7),
            key: key.into(),
            path: "/tmp/x".into(),
            failed_at,
        };
        let now = 10 * RETENTION_SEC;
        let mut list = vec![
            failed("old", now - RETENTION_SEC),
            failed("fresh", now - 60),
        ];
        assert_eq!(prune_failed(&mut list, now), vec!["old".to_string()]);
        assert_eq!(list, vec![failed("fresh", now - 60)]);

        let mut many: Vec<_> = (0..MAX_FAILED_UPLOADS + 3)
            .map(|i| failed(&format!("k{i}"), now))
            .collect();
        let removed = prune_failed(&mut many, now);
        assert_eq!(many.len(), MAX_FAILED_UPLOADS);
        assert_eq!(many[0].key, "k3");
        assert_eq!(removed, vec!["k0", "k1", "k2"]);
    }

    #[cfg(unix)]
    #[test]
    fn entries_whose_path_is_not_utf8_are_left_out_of_the_file() {
        use std::os::unix::ffi::OsStrExt as _;
        let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff.png"));
        let jobs = vec![Arc::new(job(0))];
        let failed = vec![
            FailedUpload {
                user_id: UserId(7),
                key: "bad".into(),
                path: bad,
                failed_at: 0,
            },
            FailedUpload {
                user_id: UserId(7),
                key: "good".into(),
                path: "/tmp/good.png".into(),
                failed_at: 0,
            },
        ];
        let saved = persistable(&jobs, &failed);
        assert_eq!(saved.jobs.len(), 1);
        assert_eq!(saved.failed.len(), 1);
        assert_eq!(saved.failed[0].key, "good");
        assert!(serde_json::to_vec(&saved).is_ok());
    }

    #[test]
    fn a_resumed_job_takes_the_messages_current_text_and_tokens() {
        let mut job = job(0);
        let mut current = message(3);
        current.content_tokens.t = "edited @bob #general :wave:".into();
        current.content_tokens.hg = vec![ContentToken {
            channel_id: Some("55".into()),
            s: Some(12),
            e: Some(20),
            ..Default::default()
        }];
        current.content_tokens.ej = vec![ContentToken {
            emojiid: Some("77".into()),
            s: Some(21),
            e: Some(27),
            ..Default::default()
        }];
        current.entity_mentions = vec![mezon_client::transport::ApiEntityMention {
            user_id: 9,
            role_id: 0,
            username: "bob".into(),
            s: 7,
            e: 11,
        }];
        job.adopt_current_message(&current);
        assert_eq!(job.content, "edited @bob #general :wave:");
        assert_eq!(job.mentions.len(), 1);
        assert_eq!(job.mentions[0].user_id, "9");
        assert_eq!(job.mentions[0].role_id, "");
        assert_eq!(job.mentions[0].s, 7);
        assert_eq!(job.hashtags[0].channel_id, "55");
        assert_eq!(job.emojis[0].emoji_id, "77");
    }

    #[test]
    fn a_message_counts_as_deleted_only_when_the_page_straddles_it() {
        assert_eq!(
            message_presence(5, &[message(4), message(5)]),
            MessagePresence::Present
        );
        assert_eq!(
            message_presence(5, &[message(4), message(6)]),
            MessagePresence::Deleted
        );
        assert_eq!(message_presence(5, &[message(4)]), MessagePresence::Unknown);
        assert_eq!(message_presence(5, &[]), MessagePresence::Unknown);
    }

    #[test]
    fn saved_uploads_round_trip() {
        let jobs = vec![Arc::new(job(6))];
        let failed = vec![FailedUpload {
            user_id: UserId(7),
            key: "k".into(),
            path: "/tmp/x".into(),
            failed_at: 1,
        }];
        let bytes = serde_json::to_vec(&persistable(&jobs, &failed)).expect("serialize");
        let parsed: SavedUploads = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(parsed.jobs[0].started_at, 6);
        assert_eq!(parsed.jobs[0].id(), (2, 3));
        assert_eq!(parsed.failed.len(), 1);
    }
}
