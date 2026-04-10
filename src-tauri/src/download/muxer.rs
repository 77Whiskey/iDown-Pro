use std::path::{Path, PathBuf};
use std::sync::Arc;

use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{debug, error, info, warn};

use crate::ffmpeg::manager::FfmpegManager;
use crate::utils::error::DownloadError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Wraps FFmpeg to mux separate video and audio streams into a single MP4.
pub struct Muxer {
    ffmpeg: Arc<FfmpegManager>,
}

/// Progress information parsed from a single FFmpeg stderr line.
#[derive(Debug, Clone)]
pub struct MuxProgress {
    /// Elapsed time in the output stream (seconds).
    pub time_secs: f64,
    /// Encoding speed reported by FFmpeg (e.g. `"2.5x"`), if present.
    pub speed: Option<String>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl Muxer {
    /// Create a new muxer backed by the given [`FfmpegManager`].
    pub fn new(ffmpeg: Arc<FfmpegManager>) -> Self {
        Self { ffmpeg }
    }

    /// Mux separate video and audio streams into a single MP4.
    ///
    /// Uses `-c copy` (no re-encoding) and `-movflags +faststart` so the
    /// output is optimised for progressive web playback.
    pub async fn mux(
        &self,
        video_path: &Path,
        audio_path: &Path,
        output_path: &Path,
    ) -> Result<PathBuf, DownloadError> {
        // 1. Verify inputs exist.
        Self::verify_input_exists(video_path)?;
        Self::verify_input_exists(audio_path)?;

        // 2. Ensure the output directory exists.
        Self::ensure_parent_dir(output_path)?;

        // 3. Make sure FFmpeg is available.
        self.ffmpeg
            .ensure_available()
            .await
            .map_err(|_| DownloadError::FfmpegNotAvailable)?;

        // 4. Build the argument list.
        let args = Self::build_args(video_path, audio_path, output_path);

        info!(
            video = %video_path.display(),
            audio = %audio_path.display(),
            output = %output_path.display(),
            "starting mux"
        );

        // 5. Execute FFmpeg.
        let result = self
            .ffmpeg
            .execute(args)
            .await
            .map_err(|e| DownloadError::MuxError {
                message: e.to_string(),
            })?;

        // 6. Evaluate the result.
        if result.exit_code == 0 {
            Self::verify_output(output_path)?;
            Self::cleanup_temp_files(video_path, audio_path);
            info!(output = %output_path.display(), "mux completed successfully");
            Ok(output_path.to_path_buf())
        } else {
            error!(
                exit_code = result.exit_code,
                stderr = %result.stderr,
                "ffmpeg mux failed"
            );
            // Preserve temp files for debugging.
            Err(DownloadError::MuxError {
                message: format!(
                    "ffmpeg exited with code {} — {}",
                    result.exit_code,
                    truncate_stderr(&result.stderr)
                ),
            })
        }
    }

    /// Mux with a progress callback that is invoked for every progress line
    /// FFmpeg emits on stderr.
    ///
    /// This spawns the FFmpeg process directly so that stderr can be streamed
    /// line-by-line instead of captured all at once.
    pub async fn mux_with_progress<F>(
        &self,
        video_path: &Path,
        audio_path: &Path,
        output_path: &Path,
        on_progress: F,
    ) -> Result<PathBuf, DownloadError>
    where
        F: Fn(MuxProgress) + Send + 'static,
    {
        // 1. Verify inputs exist.
        Self::verify_input_exists(video_path)?;
        Self::verify_input_exists(audio_path)?;

        // 2. Ensure the output directory exists.
        Self::ensure_parent_dir(output_path)?;

        // 3. Get the FFmpeg binary path.
        let ffmpeg_bin = self
            .ffmpeg
            .ensure_available()
            .await
            .map_err(|_| DownloadError::FfmpegNotAvailable)?;

        // 4. Build the argument list (same flags as `mux`).
        let args = Self::build_args(video_path, audio_path, output_path);

        info!(
            video = %video_path.display(),
            audio = %audio_path.display(),
            output = %output_path.display(),
            "starting mux with progress"
        );

        // 5. Spawn the child process with piped stderr.
        let mut child = tokio::process::Command::new(&ffmpeg_bin)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| DownloadError::MuxError {
                message: format!("failed to spawn ffmpeg: {e}"),
            })?;

        // 6. Stream stderr, parsing progress lines.
        let stderr = child.stderr.take().expect("stderr was piped");
        let mut reader = BufReader::new(stderr).lines();
        let mut last_stderr = String::new();

        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| DownloadError::IoError {
                message: format!("error reading ffmpeg stderr: {e}"),
            })?
        {
            debug!(line = %line, "ffmpeg stderr");
            last_stderr = line.clone();

            if let Some(progress) = Self::parse_progress_line(&line) {
                on_progress(progress);
            }
        }

        // 7. Wait for exit.
        let status = child.wait().await.map_err(|e| DownloadError::MuxError {
            message: format!("failed to wait on ffmpeg: {e}"),
        })?;

        if status.success() {
            Self::verify_output(output_path)?;
            Self::cleanup_temp_files(video_path, audio_path);
            info!(output = %output_path.display(), "mux with progress completed successfully");
            Ok(output_path.to_path_buf())
        } else {
            let code = status.code().unwrap_or(-1);
            error!(exit_code = code, stderr = %last_stderr, "ffmpeg mux failed");
            Err(DownloadError::MuxError {
                message: format!(
                    "ffmpeg exited with code {} — {}",
                    code,
                    truncate_stderr(&last_stderr)
                ),
            })
        }
    }

    /// Parse a single FFmpeg stderr line for progress information.
    ///
    /// Looks for patterns like:
    /// ```text
    /// frame=  120 fps= 60 … time=00:01:23.45 bitrate=1234.5kbits/s speed=2.50x
    /// ```
    fn parse_progress_line(line: &str) -> Option<MuxProgress> {
        // Match `time=HH:MM:SS.ss`
        let time_re = Regex::new(r"time=(\d{2}):(\d{2}):(\d{2}(?:\.\d+)?)").ok()?;
        let caps = time_re.captures(line)?;

        let hours: f64 = caps.get(1)?.as_str().parse().ok()?;
        let minutes: f64 = caps.get(2)?.as_str().parse().ok()?;
        let seconds: f64 = caps.get(3)?.as_str().parse().ok()?;

        let time_secs = hours * 3600.0 + minutes * 60.0 + seconds;

        // Optionally extract `speed=...x`
        let speed = Regex::new(r"speed=\s*([0-9.]+x)")
            .ok()
            .and_then(|re| re.captures(line))
            .map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .flatten();

        Some(MuxProgress { time_secs, speed })
    }

    /// Delete the temporary video and audio files after a successful mux.
    fn cleanup_temp_files(video_path: &Path, audio_path: &Path) {
        for path in [video_path, audio_path] {
            match std::fs::remove_file(path) {
                Ok(()) => debug!(path = %path.display(), "removed temp file"),
                Err(e) => warn!(path = %path.display(), error = %e, "failed to remove temp file"),
            }
        }
    }

    // -- helpers --------------------------------------------------------------

    /// Build the FFmpeg argument list for a copy-mux operation.
    fn build_args(video_path: &Path, audio_path: &Path, output_path: &Path) -> Vec<String> {
        vec![
            "-y".to_string(),
            "-i".to_string(),
            video_path.to_string_lossy().into_owned(),
            "-i".to_string(),
            audio_path.to_string_lossy().into_owned(),
            "-c".to_string(),
            "copy".to_string(),
            "-movflags".to_string(),
            "+faststart".to_string(),
            output_path.to_string_lossy().into_owned(),
        ]
    }

    /// Return an error if the given input file does not exist.
    fn verify_input_exists(path: &Path) -> Result<(), DownloadError> {
        if !path.exists() {
            return Err(DownloadError::IoError {
                message: format!("input file does not exist: {}", path.display()),
            });
        }
        Ok(())
    }

    /// Create the parent directory of `path` if it does not already exist.
    fn ensure_parent_dir(path: &Path) -> Result<(), DownloadError> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| DownloadError::IoError {
                    message: format!(
                        "failed to create output directory {}: {}",
                        parent.display(),
                        e
                    ),
                })?;
            }
        }
        Ok(())
    }

    /// Verify the output file exists and is non-empty after a successful mux.
    fn verify_output(path: &Path) -> Result<(), DownloadError> {
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > 0 => Ok(()),
            Ok(_) => Err(DownloadError::MuxError {
                message: format!("output file is empty: {}", path.display()),
            }),
            Err(e) => Err(DownloadError::MuxError {
                message: format!(
                    "output file not found after mux ({}): {}",
                    path.display(),
                    e
                ),
            }),
        }
    }
}

/// Truncate excessively long stderr output so error messages stay readable.
fn truncate_stderr(stderr: &str) -> &str {
    const MAX_LEN: usize = 512;
    if stderr.len() <= MAX_LEN {
        stderr
    } else {
        // Find a valid UTF-8 boundary near MAX_LEN.
        let mut end = MAX_LEN;
        while !stderr.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &stderr[..end]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_progress_full_line() {
        let line = "frame=  120 fps= 60 q=-1.0 Lsize=   12345kB time=00:01:23.45 \
                    bitrate=1234.5kbits/s speed=2.50x";
        let progress = Muxer::parse_progress_line(line).expect("should parse");
        // 1*3600 + 1*60 + 23.45 = 83.45  — wait: 00:01:23.45 = 0*3600 + 1*60 + 23.45
        let expected = 1.0 * 60.0 + 23.45;
        assert!(
            (progress.time_secs - expected).abs() < 0.001,
            "expected {expected}, got {}",
            progress.time_secs
        );
        assert_eq!(progress.speed.as_deref(), Some("2.50x"));
    }

    #[test]
    fn parse_progress_no_speed() {
        let line = "frame=   10 fps=0.0 q=-1.0 size=    256kB time=00:00:05.00 bitrate= 419.4kbits/s";
        let progress = Muxer::parse_progress_line(line).expect("should parse");
        assert!((progress.time_secs - 5.0).abs() < 0.001);
        assert!(progress.speed.is_none());
    }

    #[test]
    fn parse_progress_hours() {
        let line = "time=02:30:00.00 speed=1.00x";
        let progress = Muxer::parse_progress_line(line).expect("should parse");
        let expected = 2.0 * 3600.0 + 30.0 * 60.0;
        assert!(
            (progress.time_secs - expected).abs() < 0.001,
            "expected {expected}, got {}",
            progress.time_secs
        );
        assert_eq!(progress.speed.as_deref(), Some("1.00x"));
    }

    #[test]
    fn parse_progress_fractional_seconds() {
        let line = "time=00:00:00.50 speed=0.50x";
        let progress = Muxer::parse_progress_line(line).expect("should parse");
        assert!((progress.time_secs - 0.5).abs() < 0.001);
    }

    #[test]
    fn parse_non_progress_line_returns_none() {
        assert!(Muxer::parse_progress_line("Input #0, matroska,webm, from 'video.mkv':").is_none());
        assert!(Muxer::parse_progress_line("  Duration: 00:01:30.00, start: 0.000000").is_none());
        assert!(Muxer::parse_progress_line("Stream mapping:").is_none());
        assert!(Muxer::parse_progress_line("").is_none());
    }

    #[test]
    fn parse_progress_speed_with_whitespace() {
        // Some FFmpeg versions emit "speed= 1.5x" with a space after the equals.
        let line = "time=00:00:10.00 speed= 1.50x";
        let progress = Muxer::parse_progress_line(line).expect("should parse");
        assert!((progress.time_secs - 10.0).abs() < 0.001);
        assert_eq!(progress.speed.as_deref(), Some("1.50x"));
    }

    #[test]
    fn build_args_produces_correct_sequence() {
        let args = Muxer::build_args(
            Path::new("/tmp/video.mp4"),
            Path::new("/tmp/audio.m4a"),
            Path::new("/out/final.mp4"),
        );
        assert_eq!(
            args,
            vec![
                "-y",
                "-i",
                "/tmp/video.mp4",
                "-i",
                "/tmp/audio.m4a",
                "-c",
                "copy",
                "-movflags",
                "+faststart",
                "/out/final.mp4",
            ]
        );
    }

    #[test]
    fn truncate_stderr_short() {
        let s = "short message";
        assert_eq!(truncate_stderr(s), s);
    }

    #[test]
    fn truncate_stderr_long() {
        let s = "a".repeat(1000);
        let truncated = truncate_stderr(&s);
        assert!(truncated.len() <= 512);
    }
}
