use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Windows reserved device names (case-insensitive)
// ---------------------------------------------------------------------------

/// Device names reserved by Windows that cannot be used as filenames.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Maximum filename length after sanitisation.
const MAX_FILENAME_LEN: usize = 200;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Sanitise an arbitrary string so it is safe to use as a filename on all
/// major platforms (Windows, macOS, Linux).
///
/// - Removes characters in `<>:"/\|?*` and ASCII control characters (0x00-0x1F).
/// - Replaces removed characters with `_`.
/// - Collapses consecutive underscores into one.
/// - Trims trailing dots and spaces (invalid on Windows).
/// - Prefixes Windows reserved device names with `_`.
/// - Truncates to [`MAX_FILENAME_LEN`] characters.
/// - Returns `"_"` for empty or all-invalid input.
pub fn sanitize_filename(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());

    for ch in raw.chars() {
        if is_forbidden(ch) {
            // Avoid consecutive underscores during replacement.
            if !out.ends_with('_') {
                out.push('_');
            }
        } else {
            out.push(ch);
        }
    }

    // Trim trailing dots and spaces.
    let trimmed = out.trim_end_matches(|c: char| c == '.' || c == ' ');
    let mut result = trimmed.to_string();

    // Collapse any remaining consecutive underscores (could arise from
    // sequences like `a__b` in the original input).
    while result.contains("__") {
        result = result.replace("__", "_");
    }

    // Trim leading/trailing underscores that look ugly.
    result = result.trim_matches('_').to_string();

    // Handle empty result.
    if result.is_empty() {
        return "_".to_string();
    }

    // Guard against Windows reserved names (compare case-insensitively,
    // also check for names followed by an extension like `CON.txt`).
    let upper = result.to_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    if RESERVED_NAMES.contains(&stem) {
        result = format!("_{result}");
    }

    // Truncate to max length (on a char boundary).
    if result.chars().count() > MAX_FILENAME_LEN {
        result = result.chars().take(MAX_FILENAME_LEN).collect();
    }

    result
}

/// Build the output file path:
/// `{output_dir}/{sanitised_username}/{shortcode}_{index}.{ext}`
pub fn resolve_output_path(
    output_dir: &Path,
    username: &str,
    shortcode: &str,
    index: usize,
    ext: &str,
) -> PathBuf {
    let safe_user = sanitize_filename(username);
    let safe_code = sanitize_filename(shortcode);
    let ext_clean = ext.trim_start_matches('.');

    output_dir
        .join(&safe_user)
        .join(format!("{safe_code}_{index}.{ext_clean}"))
}

/// Create a directory and all of its parents if they do not already exist.
pub fn ensure_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Return a path inside the application temp directory scoped to a specific
/// download task: `{app_data}/temp/{task_id}_{suffix}`.
pub fn temp_file_path(app_data: &Path, task_id: &str, suffix: &str) -> PathBuf {
    app_data.join("temp").join(format!("{task_id}_{suffix}"))
}

/// Check whether a file exists and its size is at least `expected_min` bytes.
pub fn file_exists_with_size(path: &Path, expected_min: u64) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.len() >= expected_min,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` for characters that are not allowed in filenames.
fn is_forbidden(ch: char) -> bool {
    matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        || ch.is_ascii_control()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_text_unchanged() {
        assert_eq!(sanitize_filename("hello_world"), "hello_world");
    }

    #[test]
    fn removes_forbidden_chars() {
        assert_eq!(sanitize_filename("a<b>c:d"), "a_b_c_d");
    }

    #[test]
    fn collapses_underscores() {
        assert_eq!(sanitize_filename("a///b"), "a_b");
    }

    #[test]
    fn unicode_preserved() {
        assert_eq!(sanitize_filename("cafe\u{0301}_photo"), "cafe\u{0301}_photo");
        assert_eq!(sanitize_filename("\u{1F600}_post"), "\u{1F600}_post");
    }

    #[test]
    fn long_string_truncated() {
        let long = "a".repeat(300);
        let result = sanitize_filename(&long);
        assert_eq!(result.chars().count(), MAX_FILENAME_LEN);
    }

    #[test]
    fn reserved_names_prefixed() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("con"), "_con");
        assert_eq!(sanitize_filename("COM1"), "_COM1");
        assert_eq!(sanitize_filename("lpt3"), "_lpt3");
        // reserved name with extension
        assert_eq!(sanitize_filename("NUL.txt"), "_NUL.txt");
    }

    #[test]
    fn control_chars_stripped() {
        assert_eq!(sanitize_filename("hello\x00world\x1F"), "hello_world");
    }

    #[test]
    fn empty_string_becomes_underscore() {
        assert_eq!(sanitize_filename(""), "_");
    }

    #[test]
    fn all_invalid_becomes_underscore() {
        assert_eq!(sanitize_filename(":::"), "_");
    }

    #[test]
    fn trailing_dots_and_spaces_trimmed() {
        assert_eq!(sanitize_filename("file..."), "file");
        assert_eq!(sanitize_filename("file   "), "file");
        assert_eq!(sanitize_filename("file . ."), "file");
    }

    #[test]
    fn resolve_output_path_builds_correctly() {
        let path = resolve_output_path(
            Path::new("/downloads"),
            "user_name",
            "ABC123",
            2,
            ".mp4",
        );
        assert_eq!(path, PathBuf::from("/downloads/user_name/ABC123_2.mp4"));
    }

    #[test]
    fn temp_file_path_builds_correctly() {
        let path = temp_file_path(Path::new("/app"), "task-1", "video.tmp");
        assert_eq!(path, PathBuf::from("/app/temp/task-1_video.tmp"));
    }

    #[test]
    fn file_exists_with_size_returns_false_for_missing() {
        assert!(!file_exists_with_size(Path::new("/nonexistent_path_xyz"), 0));
    }
}
