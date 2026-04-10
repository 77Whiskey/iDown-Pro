use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::extractor::types::{MediaType, QualityPref};

// ---------------------------------------------------------------------------
// DownloadStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Muxing,
    Paused,
    Completed,
    Error,
}

impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::Queued => "queued",
            DownloadStatus::Downloading => "downloading",
            DownloadStatus::Muxing => "muxing",
            DownloadStatus::Paused => "paused",
            DownloadStatus::Completed => "completed",
            DownloadStatus::Error => "error",
        }
    }
}

impl std::fmt::Display for DownloadStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// DownloadPhase
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadPhase {
    Downloading,
    Muxing,
    PostProcessing,
}

// ---------------------------------------------------------------------------
// DownloadTask
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: String,
    pub shortcode: String,
    pub url: String,
    pub owner_username: String,
    pub media_type: MediaType,
    pub status: DownloadStatus,
    pub quality_pref: QualityPref,
    pub file_path: Option<PathBuf>,
    pub error_message: Option<String>,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub retry_count: u32,
}

/// Maximum number of automatic retries before a task is considered permanently failed.
const MAX_RETRIES: u32 = 3;

impl DownloadTask {
    /// Create a new download task in the [`DownloadStatus::Queued`] state.
    pub fn new(
        shortcode: String,
        url: String,
        owner_username: String,
        media_type: MediaType,
        quality_pref: QualityPref,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            shortcode,
            url,
            owner_username,
            media_type,
            status: DownloadStatus::Queued,
            quality_pref,
            file_path: None,
            error_message: None,
            bytes_downloaded: 0,
            total_bytes: None,
            created_at: chrono::Utc::now().timestamp(),
            completed_at: None,
            retry_count: 0,
        }
    }

    /// Returns `true` when the task is actively doing work (downloading or muxing).
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            DownloadStatus::Downloading | DownloadStatus::Muxing
        )
    }

    /// Returns `true` when the task has reached a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            DownloadStatus::Completed | DownloadStatus::Error
        )
    }

    /// Returns `true` when the task is in an error state and has not yet
    /// exhausted the maximum number of retries.
    pub fn can_retry(&self) -> bool {
        self.status == DownloadStatus::Error && self.retry_count < MAX_RETRIES
    }

    /// Transition to [`DownloadStatus::Downloading`].
    pub fn mark_downloading(&mut self) {
        self.status = DownloadStatus::Downloading;
        self.error_message = None;
    }

    /// Transition to [`DownloadStatus::Muxing`].
    pub fn mark_muxing(&mut self) {
        self.status = DownloadStatus::Muxing;
    }

    /// Transition to [`DownloadStatus::Completed`] and record the output file path.
    pub fn mark_completed(&mut self, file_path: PathBuf) {
        self.status = DownloadStatus::Completed;
        self.file_path = Some(file_path);
        self.completed_at = Some(chrono::Utc::now().timestamp());
        self.error_message = None;
    }

    /// Transition to [`DownloadStatus::Error`], record the error message, and
    /// increment the retry counter.
    pub fn mark_error(&mut self, message: String) {
        self.status = DownloadStatus::Error;
        self.error_message = Some(message);
        self.retry_count += 1;
    }

    /// Transition to [`DownloadStatus::Paused`].
    pub fn mark_paused(&mut self) {
        self.status = DownloadStatus::Paused;
    }

    /// Re-queue a paused or errored task for another attempt.
    pub fn mark_queued(&mut self) {
        self.status = DownloadStatus::Queued;
        self.error_message = None;
    }
}

// ---------------------------------------------------------------------------
// DownloadProgress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub task_id: String,
    pub phase: DownloadPhase,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: u64,
    pub eta_secs: Option<u32>,
}

impl DownloadProgress {
    /// Create a new progress snapshot with zero counters.
    pub fn new(task_id: String, phase: DownloadPhase) -> Self {
        Self {
            task_id,
            phase,
            bytes_downloaded: 0,
            total_bytes: None,
            speed_bps: 0,
            eta_secs: None,
        }
    }

    /// Set byte counters (builder pattern).
    pub fn with_bytes(mut self, downloaded: u64, total: Option<u64>) -> Self {
        self.bytes_downloaded = downloaded;
        self.total_bytes = total;
        self
    }

    /// Set download speed in bytes per second (builder pattern).
    pub fn with_speed(mut self, speed_bps: u64) -> Self {
        self.speed_bps = speed_bps;
        // Recompute ETA if we have both speed and remaining bytes.
        if speed_bps > 0 {
            if let Some(total) = self.total_bytes {
                let remaining = total.saturating_sub(self.bytes_downloaded);
                self.eta_secs = Some((remaining / speed_bps) as u32);
            }
        }
        self
    }

    /// Compute the download percentage (`0.0..=100.0`), or `None` when the
    /// total size is unknown.
    pub fn percentage(&self) -> Option<f32> {
        self.total_bytes.map(|total| {
            if total == 0 {
                100.0
            } else {
                (self.bytes_downloaded as f64 / total as f64 * 100.0) as f32
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task() -> DownloadTask {
        DownloadTask::new(
            "ABC123".to_string(),
            "https://example.com/video.mp4".to_string(),
            "testuser".to_string(),
            MediaType::Video,
            QualityPref::Max,
        )
    }

    // -- DownloadTask state transitions ----------------------------------------

    #[test]
    fn new_task_is_queued() {
        let task = make_task();
        assert_eq!(task.status, DownloadStatus::Queued);
        assert!(!task.is_active());
        assert!(!task.is_terminal());
    }

    #[test]
    fn downloading_is_active() {
        let mut task = make_task();
        task.mark_downloading();
        assert_eq!(task.status, DownloadStatus::Downloading);
        assert!(task.is_active());
        assert!(!task.is_terminal());
    }

    #[test]
    fn muxing_is_active() {
        let mut task = make_task();
        task.mark_muxing();
        assert_eq!(task.status, DownloadStatus::Muxing);
        assert!(task.is_active());
        assert!(!task.is_terminal());
    }

    #[test]
    fn completed_is_terminal() {
        let mut task = make_task();
        task.mark_completed(PathBuf::from("/out/video.mp4"));
        assert_eq!(task.status, DownloadStatus::Completed);
        assert!(task.is_terminal());
        assert!(!task.is_active());
        assert!(task.file_path.is_some());
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn error_is_terminal_and_retryable() {
        let mut task = make_task();
        task.mark_error("network timeout".into());
        assert_eq!(task.status, DownloadStatus::Error);
        assert!(task.is_terminal());
        assert_eq!(task.retry_count, 1);
        assert!(task.can_retry());
    }

    #[test]
    fn exhausted_retries_cannot_retry() {
        let mut task = make_task();
        task.mark_error("err 1".into());
        task.mark_error("err 2".into());
        task.mark_error("err 3".into());
        assert_eq!(task.retry_count, 3);
        assert!(!task.can_retry());
    }

    #[test]
    fn paused_then_requeued() {
        let mut task = make_task();
        task.mark_downloading();
        task.mark_paused();
        assert_eq!(task.status, DownloadStatus::Paused);
        assert!(!task.is_active());
        assert!(!task.is_terminal());

        task.mark_queued();
        assert_eq!(task.status, DownloadStatus::Queued);
        assert!(task.error_message.is_none());
    }

    #[test]
    fn mark_downloading_clears_error_message() {
        let mut task = make_task();
        task.mark_error("something broke".into());
        assert!(task.error_message.is_some());
        task.mark_downloading();
        assert!(task.error_message.is_none());
    }

    #[test]
    fn completed_clears_error_message() {
        let mut task = make_task();
        task.mark_error("oops".into());
        task.mark_completed(PathBuf::from("/out.mp4"));
        assert!(task.error_message.is_none());
    }

    #[test]
    fn task_id_is_valid_uuid() {
        let task = make_task();
        assert!(uuid::Uuid::parse_str(&task.id).is_ok());
    }

    // -- DownloadProgress percentage ------------------------------------------

    #[test]
    fn percentage_with_known_total() {
        let progress = DownloadProgress::new("t1".into(), DownloadPhase::Downloading)
            .with_bytes(50, Some(200));
        let pct = progress.percentage().unwrap();
        assert!((pct - 25.0).abs() < 0.01);
    }

    #[test]
    fn percentage_unknown_total_is_none() {
        let progress =
            DownloadProgress::new("t1".into(), DownloadPhase::Downloading).with_bytes(50, None);
        assert!(progress.percentage().is_none());
    }

    #[test]
    fn percentage_zero_total_is_100() {
        let progress = DownloadProgress::new("t1".into(), DownloadPhase::Downloading)
            .with_bytes(0, Some(0));
        let pct = progress.percentage().unwrap();
        assert!((pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn percentage_complete() {
        let progress = DownloadProgress::new("t1".into(), DownloadPhase::Downloading)
            .with_bytes(1000, Some(1000));
        let pct = progress.percentage().unwrap();
        assert!((pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn with_speed_computes_eta() {
        let progress = DownloadProgress::new("t1".into(), DownloadPhase::Downloading)
            .with_bytes(500, Some(1500))
            .with_speed(100);
        assert_eq!(progress.speed_bps, 100);
        // remaining = 1000, speed = 100 => 10 seconds
        assert_eq!(progress.eta_secs, Some(10));
    }

    #[test]
    fn with_speed_zero_no_eta() {
        let progress = DownloadProgress::new("t1".into(), DownloadPhase::Downloading)
            .with_bytes(500, Some(1500))
            .with_speed(0);
        assert!(progress.eta_secs.is_none());
    }

    // -- DownloadStatus display -----------------------------------------------

    #[test]
    fn status_display() {
        assert_eq!(DownloadStatus::Queued.to_string(), "queued");
        assert_eq!(DownloadStatus::Downloading.to_string(), "downloading");
        assert_eq!(DownloadStatus::Muxing.to_string(), "muxing");
        assert_eq!(DownloadStatus::Paused.to_string(), "paused");
        assert_eq!(DownloadStatus::Completed.to_string(), "completed");
        assert_eq!(DownloadStatus::Error.to_string(), "error");
    }
}
