use std::io::Write;
use std::path::Path;

use regex::Regex;
use tracing_appender::rolling;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the application-wide tracing subscriber.
///
/// - Rolling daily log files written to `{app_data_dir}/logs/`.
/// - Sensitive tokens (`sessionid`, `csrftoken`) are redacted from all output.
/// - In debug builds an additional console layer is enabled at `DEBUG` level.
/// - In release builds the default level is `INFO` with file output only.
pub fn init_logging(app_data_dir: &Path) {
    let log_dir = app_data_dir.join("logs");

    // Best-effort directory creation so the appender doesn't fail silently.
    let _ = std::fs::create_dir_all(&log_dir);

    // Rolling daily file appender.
    let file_appender = rolling::daily(&log_dir, "app.log");
    let redacting_file = RedactingMakeWriter::new(file_appender);

    let env_filter = if cfg!(debug_assertions) {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };

    let file_layer = fmt::layer()
        .with_writer(redacting_file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer);

    if cfg!(debug_assertions) {
        let console_layer = fmt::layer()
            .with_writer(RedactingMakeWriter::new(std::io::stderr))
            .with_target(true)
            .with_thread_ids(false)
            .with_file(true)
            .with_line_number(true)
            .pretty();

        registry.with(console_layer).init();
    } else {
        registry.init();
    }

    // Best-effort cleanup of logs older than 7 days.
    cleanup_old_logs(&log_dir, 7);

    tracing::info!(log_dir = %log_dir.display(), "logging initialised");
}

// ---------------------------------------------------------------------------
// Log retention cleanup
// ---------------------------------------------------------------------------

/// Remove log files older than `keep_days` from `log_dir`.
fn cleanup_old_logs(log_dir: &Path, keep_days: u64) {
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days * 24 * 60 * 60);

    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            let modified = meta.modified().unwrap_or(std::time::SystemTime::now());
            if modified < cutoff {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Redacting writer
// ---------------------------------------------------------------------------

/// A [`MakeWriter`] wrapper that strips sensitive Instagram session tokens
/// from every line written to the underlying writer.
#[derive(Clone)]
struct RedactingMakeWriter<W> {
    inner: W,
}

impl<W> RedactingMakeWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<'a, W> MakeWriter<'a> for RedactingMakeWriter<W>
where
    W: MakeWriter<'a>,
{
    type Writer = RedactingWriter<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new(self.inner.make_writer())
    }
}

/// Writer returned by [`RedactingMakeWriter`].  It buffers each `write` call,
/// applies regex-based redaction, then flushes the sanitised bytes to the
/// underlying writer.
struct RedactingWriter<W> {
    inner: W,
}

impl<W> RedactingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let original_len = buf.len();

        // Fast path: skip regex overhead when no sensitive prefixes are present.
        if !contains_sensitive(buf) {
            self.inner.write_all(buf)?;
            return Ok(original_len);
        }

        // Slow path: convert to string, apply redactions, write.
        let text = String::from_utf8_lossy(buf);
        let redacted = redact(&text);
        self.inner.write_all(redacted.as_bytes())?;
        Ok(original_len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Quick byte-level scan for sensitive token prefixes.
fn contains_sensitive(buf: &[u8]) -> bool {
    buf.windows(10)
        .any(|w| w.starts_with(b"sessionid=") || w.starts_with(b"csrftoken="))
}

/// Apply regex redactions to a string.
fn redact(input: &str) -> String {
    thread_local! {
        static SESSION_RE: Regex =
            Regex::new(r"sessionid=[^;&\s]+").expect("invalid regex");
        static CSRF_RE: Regex =
            Regex::new(r"csrftoken=[^;&\s]+").expect("invalid regex");
    }

    let output = SESSION_RE.with(|re| re.replace_all(input, "sessionid=[REDACTED]").into_owned());
    CSRF_RE.with(|re| re.replace_all(&output, "csrftoken=[REDACTED]").into_owned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_sessionid() {
        let input = "cookie: sessionid=abc123def456; ds_user_id=12345";
        let result = redact(input);
        assert_eq!(result, "cookie: sessionid=[REDACTED]; ds_user_id=12345");
    }

    #[test]
    fn redact_csrftoken() {
        let input = "csrftoken=longtoken123&other=value";
        let result = redact(input);
        assert_eq!(result, "csrftoken=[REDACTED]&other=value");
    }

    #[test]
    fn redact_both_tokens() {
        let input = "sessionid=abc; csrftoken=xyz";
        let result = redact(input);
        assert_eq!(result, "sessionid=[REDACTED]; csrftoken=[REDACTED]");
    }

    #[test]
    fn no_sensitive_data_unchanged() {
        let input = "just a normal log line with no secrets";
        let result = redact(input);
        assert_eq!(result, input);
    }

    #[test]
    fn contains_sensitive_detects_tokens() {
        assert!(contains_sensitive(b"sessionid=abc"));
        assert!(contains_sensitive(b"csrftoken=abc"));
        assert!(!contains_sensitive(b"nothing here"));
    }
}
