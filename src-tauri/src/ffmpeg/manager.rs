use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::utils::error::FfmpegError;
use crate::utils::http::HttpClient;

use super::version::{self, FfmpegVersionInfo};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FfmpegStatus {
    NotInstalled,
    Installed { version: String, path: PathBuf },
    Downloading { progress_pct: f32 },
    Updating { from: String, to: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct FfmpegProgress {
    pub phase: FfmpegPhase,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub progress_pct: f32,
}

#[derive(Debug, Clone, Serialize)]
pub enum FfmpegPhase {
    Downloading,
    Verifying,
    Extracting,
}

pub struct FfmpegOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum time (in seconds) we allow an ffmpeg child process to run.
const EXECUTION_TIMEOUT_SECS: u64 = 300;

/// The name of the ffmpeg binary inside the managed directory.
const FFMPEG_BIN: &str = "ffmpeg.exe";

/// The subdirectory under `app_data` where ffmpeg artifacts live.
const FFMPEG_DIR: &str = "ffmpeg";

// ---------------------------------------------------------------------------
// FfmpegManager
// ---------------------------------------------------------------------------

pub struct FfmpegManager {
    app_data: PathBuf,
    http: HttpClient,
    status: Arc<RwLock<FfmpegStatus>>,
}

impl FfmpegManager {
    /// Creates a new manager. Inspects the filesystem synchronously to
    /// determine whether ffmpeg.exe is already present and sets the initial
    /// status accordingly.
    pub fn new(app_data: PathBuf, http: HttpClient) -> Self {
        let ffmpeg_exe = app_data.join(FFMPEG_DIR).join(FFMPEG_BIN);
        let initial_status = if ffmpeg_exe.exists() {
            let local = version::load_local_version(&app_data);
            let version_tag = local.map(|v| v.tag).unwrap_or_else(|| "unknown".into());
            info!(path = %ffmpeg_exe.display(), version = %version_tag, "ffmpeg found on disk");
            FfmpegStatus::Installed {
                version: version_tag,
                path: ffmpeg_exe,
            }
        } else {
            debug!(path = %ffmpeg_exe.display(), "ffmpeg not found on disk");
            FfmpegStatus::NotInstalled
        };

        Self {
            app_data,
            http,
            status: Arc::new(RwLock::new(initial_status)),
        }
    }

    /// Absolute path where ffmpeg.exe should live.
    pub fn ffmpeg_path(&self) -> PathBuf {
        self.app_data.join(FFMPEG_DIR).join(FFMPEG_BIN)
    }

    /// Returns a snapshot of the current status.
    pub async fn status(&self) -> FfmpegStatus {
        self.status.read().await.clone()
    }

    /// Ensures ffmpeg.exe is available on disk. If it already exists the path
    /// is returned immediately; otherwise a full download-and-install cycle is
    /// triggered (without progress reporting).
    pub async fn ensure_available(&self) -> Result<PathBuf, FfmpegError> {
        let exe = self.ffmpeg_path();
        if exe.exists() {
            debug!(path = %exe.display(), "ffmpeg already available");
            return Ok(exe);
        }

        info!("ffmpeg not present — starting download");
        self.download_and_install(None).await?;
        Ok(exe)
    }

    /// Downloads, verifies, and extracts ffmpeg.exe from the latest GitHub
    /// release. Progress updates are sent to `progress_tx` when provided.
    pub async fn download_and_install(
        &self,
        progress_tx: Option<tokio::sync::mpsc::Sender<FfmpegProgress>>,
    ) -> Result<(), FfmpegError> {
        // --- 1. Fetch latest version info -------------------------------------
        let remote = version::check_latest_version(&self.http).await?;
        info!(tag = %remote.tag, url = %remote.download_url, "resolved latest FFmpeg release");

        // Determine if this is a fresh install or an update.
        let local = version::load_local_version(&self.app_data);
        if let Some(ref l) = local {
            *self.status.write().await = FfmpegStatus::Updating {
                from: l.tag.clone(),
                to: remote.tag.clone(),
            };
        } else {
            *self.status.write().await = FfmpegStatus::Downloading { progress_pct: 0.0 };
        }

        // --- 2. Download zip --------------------------------------------------
        let zip_bytes = self
            .download_with_progress(&remote.download_url, &progress_tx)
            .await?;

        info!(
            bytes = zip_bytes.len(),
            "ffmpeg zip download complete"
        );

        // --- 3. Verify SHA-256 (if checksum URL available) --------------------
        if let Some(ref sha_url) = remote.sha256_url {
            Self::send_progress(&progress_tx, FfmpegPhase::Verifying, zip_bytes.len() as u64, None, 100.0)
                .await;

            info!("downloading SHA-256 checksum for verification");
            let expected_hash = self.download_checksum(sha_url).await?;
            let actual_hash = Self::compute_sha256(&zip_bytes);

            if actual_hash != expected_hash {
                error!(%expected_hash, %actual_hash, "SHA-256 checksum mismatch");
                *self.status.write().await = FfmpegStatus::Error {
                    message: format!("checksum mismatch: expected {expected_hash}, got {actual_hash}"),
                };
                return Err(FfmpegError::ChecksumMismatch {
                    expected: expected_hash,
                    actual: actual_hash,
                });
            }

            info!("SHA-256 checksum verified successfully");
        } else {
            warn!("skipping SHA-256 verification — no checksum URL available");
        }

        // --- 4. Extract -------------------------------------------------------
        Self::send_progress(&progress_tx, FfmpegPhase::Extracting, 0, None, 0.0).await;

        let ffmpeg_dir = self.app_data.join(FFMPEG_DIR);
        Self::extract_ffmpeg_from_zip(&zip_bytes, &ffmpeg_dir)?;

        info!(path = %ffmpeg_dir.display(), "ffmpeg extracted successfully");

        // --- 5. Save version info ---------------------------------------------
        version::save_local_version(&self.app_data, &remote)?;

        // --- 6. Update status -------------------------------------------------
        let exe_path = self.ffmpeg_path();
        *self.status.write().await = FfmpegStatus::Installed {
            version: remote.tag.clone(),
            path: exe_path,
        };

        info!(tag = %remote.tag, "ffmpeg install complete");
        Ok(())
    }

    /// Executes ffmpeg with the given arguments and returns the captured
    /// output. The child process is automatically killed if it exceeds the
    /// timeout or if the returned future is dropped.
    pub async fn execute(&self, args: Vec<String>) -> Result<FfmpegOutput, FfmpegError> {
        let exe = self.ffmpeg_path();

        if !exe.exists() {
            error!(path = %exe.display(), "ffmpeg binary not found");
            return Err(FfmpegError::NotInstalled);
        }

        debug!(
            cmd = %exe.display(),
            args = ?args,
            "executing ffmpeg"
        );

        let child = Command::new(&exe)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| FfmpegError::ExecutionFailed {
                exit_code: None,
                stderr: format!("failed to spawn ffmpeg process: {e}"),
            })?;

        let timeout = Duration::from_secs(EXECUTION_TIMEOUT_SECS);
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                error!(%e, "error waiting for ffmpeg process");
                return Err(FfmpegError::ExecutionFailed {
                    exit_code: None,
                    stderr: format!("error waiting for ffmpeg: {e}"),
                });
            }
            Err(_elapsed) => {
                // The child is dropped here, which triggers kill_on_drop.
                error!(timeout_secs = EXECUTION_TIMEOUT_SECS, "ffmpeg execution timed out");
                return Err(FfmpegError::Timeout);
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if !output.status.success() {
            warn!(
                exit_code,
                stderr_len = stderr.len(),
                "ffmpeg exited with non-zero status"
            );
            return Err(FfmpegError::ExecutionFailed {
                exit_code: Some(exit_code),
                stderr,
            });
        }

        debug!(exit_code, "ffmpeg execution succeeded");
        Ok(FfmpegOutput {
            exit_code,
            stdout,
            stderr,
        })
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Downloads a file from `url` with streaming, tracking progress.
    async fn download_with_progress(
        &self,
        url: &str,
        progress_tx: &Option<tokio::sync::mpsc::Sender<FfmpegProgress>>,
    ) -> Result<Vec<u8>, FfmpegError> {
        let response = self
            .http
            .client()
            .get(url)
            .header("User-Agent", "InstagramDownloaderPro/0.1")
            .send()
            .await
            .map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to start download from {url}: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(FfmpegError::DownloadFailed {
                message: format!("download returned HTTP {status} for {url}"),
            });
        }

        let total_bytes = response.content_length();
        let mut downloaded: u64 = 0;
        let mut buffer = Vec::with_capacity(total_bytes.unwrap_or(128 * 1024 * 1024) as usize);

        // Use reqwest's chunk() API to stream the download without requiring
        // futures-util / StreamExt.
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FfmpegError::DownloadFailed {
                message: format!("error reading download stream: {e}"),
            })?
        {
            downloaded += chunk.len() as u64;
            buffer.extend_from_slice(&chunk);

            let pct = total_bytes
                .map(|t| if t > 0 { (downloaded as f32 / t as f32) * 100.0 } else { 0.0 })
                .unwrap_or(0.0);

            // Update shared status.
            *self.status.write().await = FfmpegStatus::Downloading { progress_pct: pct };

            // Send progress to channel.
            Self::send_progress(progress_tx, FfmpegPhase::Downloading, downloaded, total_bytes, pct)
                .await;
        }

        Ok(buffer)
    }

    /// Downloads the SHA-256 checksum file and parses out the hex digest.
    ///
    /// The file format is typically: `<hash>  <filename>\n` or just `<hash>\n`.
    async fn download_checksum(&self, url: &str) -> Result<String, FfmpegError> {
        let text = self
            .http
            .client()
            .get(url)
            .header("User-Agent", "InstagramDownloaderPro/0.1")
            .send()
            .await
            .map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to download checksum from {url}: {e}"),
            })?
            .text()
            .await
            .map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to read checksum response: {e}"),
            })?;

        // Parse: take the first whitespace-delimited token as the hex hash.
        let hash = text
            .split_whitespace()
            .next()
            .ok_or_else(|| FfmpegError::DownloadFailed {
                message: format!("checksum file is empty or malformed: {text:?}"),
            })?
            .to_lowercase();

        debug!(hash = %hash, "parsed SHA-256 checksum");
        Ok(hash)
    }

    /// Computes the SHA-256 digest of `data`, returning the lowercase hex string.
    fn compute_sha256(data: &[u8]) -> String {
        let digest = Sha256::digest(data);
        // Format as lowercase hex.
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Extracts `ffmpeg.exe` from the downloaded zip into `target_dir`.
    ///
    /// The BtbN builds contain a nested directory structure like
    /// `ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe`. We search through the
    /// archive entries to find `ffmpeg.exe` and flatten it into `target_dir`.
    fn extract_ffmpeg_from_zip(zip_bytes: &[u8], target_dir: &Path) -> Result<(), FfmpegError> {
        std::fs::create_dir_all(target_dir).map_err(|e| FfmpegError::DownloadFailed {
            message: format!(
                "failed to create directory {}: {e}",
                target_dir.display()
            ),
        })?;

        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| FfmpegError::DownloadFailed {
            message: format!("failed to open zip archive: {e}"),
        })?;

        info!(entries = archive.len(), "opened zip archive");

        let mut found_ffmpeg = false;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to read zip entry {i}: {e}"),
            })?;

            let entry_name = entry.name().to_string();

            // We are only interested in files whose basename is ffmpeg.exe
            // (or ffprobe.exe, which is also useful to have).
            let basename = Path::new(&entry_name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            let dominated = matches!(
                basename.to_lowercase().as_str(),
                "ffmpeg.exe" | "ffprobe.exe"
            );

            if !dominated || entry.is_dir() {
                continue;
            }

            let dest = target_dir.join(basename);
            debug!(
                entry = %entry_name,
                dest = %dest.display(),
                size = entry.size(),
                "extracting"
            );

            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf).map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to read zip entry '{entry_name}': {e}"),
            })?;

            std::fs::write(&dest, &buf).map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to write {}: {e}", dest.display()),
            })?;

            if basename.eq_ignore_ascii_case(FFMPEG_BIN) {
                found_ffmpeg = true;
            }

            info!(file = %dest.display(), bytes = buf.len(), "extracted");
        }

        if !found_ffmpeg {
            error!("ffmpeg.exe not found inside the zip archive");
            return Err(FfmpegError::DownloadFailed {
                message: "ffmpeg.exe was not found inside the downloaded zip archive".into(),
            });
        }

        Ok(())
    }

    /// Sends a progress update through the channel, if one was provided.
    /// Silently ignores send failures (e.g. if the receiver was dropped).
    async fn send_progress(
        tx: &Option<tokio::sync::mpsc::Sender<FfmpegProgress>>,
        phase: FfmpegPhase,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
        progress_pct: f32,
    ) {
        if let Some(sender) = tx {
            let _ = sender
                .send(FfmpegProgress {
                    phase,
                    bytes_downloaded,
                    total_bytes,
                    progress_pct,
                })
                .await;
        }
    }
}
