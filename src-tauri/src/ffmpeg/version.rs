use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::utils::error::FfmpegError;
use crate::utils::http::HttpClient;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FfmpegVersionInfo {
    pub tag: String,
    pub download_url: String,
    pub sha256_url: Option<String>,
    /// Unix timestamp (seconds) when this version was last checked.
    pub checked_at: i64,
}

// ---------------------------------------------------------------------------
// GitHub API response types (subset we care about)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

/// The exact asset name we are looking for in the release.
const TARGET_ASSET_NAME: &str = "ffmpeg-master-latest-win64-gpl.zip";

/// Filename used to persist the local version info.
const VERSION_FILE: &str = "version.json";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Queries the GitHub API for the latest FFmpeg-Builds release and returns
/// the download URL for the Windows 64-bit GPL build.
pub async fn check_latest_version(http: &HttpClient) -> Result<FfmpegVersionInfo, FfmpegError> {
    info!("checking latest FFmpeg version from GitHub");

    let response = http
        .client()
        .get(GITHUB_RELEASES_URL)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "InstagramDownloaderPro/0.1")
        .send()
        .await
        .map_err(|e| FfmpegError::DownloadFailed {
            message: format!("failed to query GitHub releases API: {e}"),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        error!(%status, "GitHub API returned non-success status");
        return Err(FfmpegError::DownloadFailed {
            message: format!("GitHub API returned {status}: {body}"),
        });
    }

    let release: GitHubRelease =
        response
            .json()
            .await
            .map_err(|e| FfmpegError::DownloadFailed {
                message: format!("failed to parse GitHub release JSON: {e}"),
            })?;

    debug!(tag = %release.tag_name, asset_count = release.assets.len(), "parsed GitHub release");

    // Find the primary zip asset.
    let zip_asset = release
        .assets
        .iter()
        .find(|a| a.name == TARGET_ASSET_NAME)
        .ok_or_else(|| FfmpegError::DownloadFailed {
            message: format!(
                "release {} does not contain asset '{TARGET_ASSET_NAME}'",
                release.tag_name
            ),
        })?;

    // Look for a matching .sha256 checksum asset (best-effort).
    let sha256_name = format!("{TARGET_ASSET_NAME}.sha256");
    let sha256_asset = release.assets.iter().find(|a| a.name == sha256_name);

    if sha256_asset.is_none() {
        warn!("no .sha256 checksum asset found in release — integrity check will be skipped");
    }

    let info = FfmpegVersionInfo {
        tag: release.tag_name.clone(),
        download_url: zip_asset.browser_download_url.clone(),
        sha256_url: sha256_asset.map(|a| a.browser_download_url.clone()),
        checked_at: chrono::Utc::now().timestamp(),
    };

    info!(tag = %info.tag, "found latest FFmpeg release");
    Ok(info)
}

/// Loads the previously saved version info from `{app_data}/ffmpeg/version.json`.
///
/// Returns `None` if the file does not exist or cannot be parsed.
pub fn load_local_version(app_data: &Path) -> Option<FfmpegVersionInfo> {
    let path = app_data.join("ffmpeg").join(VERSION_FILE);
    debug!(?path, "loading local FFmpeg version info");

    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            debug!(?path, %e, "could not read local version file");
            return None;
        }
    };

    match serde_json::from_str::<FfmpegVersionInfo>(&contents) {
        Ok(info) => {
            debug!(tag = %info.tag, "loaded local FFmpeg version info");
            Some(info)
        }
        Err(e) => {
            warn!(?path, %e, "failed to parse local FFmpeg version file");
            None
        }
    }
}

/// Persists the given version info to `{app_data}/ffmpeg/version.json`.
pub fn save_local_version(app_data: &Path, info: &FfmpegVersionInfo) -> Result<(), FfmpegError> {
    let dir = app_data.join("ffmpeg");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| FfmpegError::DownloadFailed {
            message: format!("failed to create ffmpeg directory {}: {e}", dir.display()),
        })?;
    }

    let path = dir.join(VERSION_FILE);
    let json = serde_json::to_string_pretty(info).map_err(|e| FfmpegError::DownloadFailed {
        message: format!("failed to serialize version info: {e}"),
    })?;

    std::fs::write(&path, json).map_err(|e| FfmpegError::DownloadFailed {
        message: format!("failed to write version file {}: {e}", path.display()),
    })?;

    debug!(?path, "saved local FFmpeg version info");
    Ok(())
}

/// Returns `true` when a download/update is needed: either no local version
/// exists or the local tag differs from the remote tag.
pub fn needs_update(local: &Option<FfmpegVersionInfo>, remote: &FfmpegVersionInfo) -> bool {
    match local {
        None => {
            debug!("no local version — update needed");
            true
        }
        Some(l) if l.tag != remote.tag => {
            info!(
                local_tag = %l.tag,
                remote_tag = %remote.tag,
                "local tag differs from remote — update needed"
            );
            true
        }
        Some(l) => {
            debug!(tag = %l.tag, "local version matches remote — no update needed");
            false
        }
    }
}
