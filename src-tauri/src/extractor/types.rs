use serde::{Deserialize, Serialize};

/// The kind of Instagram media.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MediaType {
    Photo,
    Video,
    Reel,
    Carousel,
    Story,
    Highlight,
}

/// A complete Instagram post, possibly containing multiple items (carousel).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaPost {
    pub id: String,
    pub shortcode: String,
    pub media_type: MediaType,
    pub owner_username: String,
    pub caption: Option<String>,
    pub timestamp: i64,
    pub items: Vec<MediaItem>,
}

/// A single media item within a post (one photo or video).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub media_type: MediaType,
    pub video_url: Option<String>,
    pub dash_manifest: Option<String>,
    pub photo_url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub duration_secs: Option<f64>,
}

/// User preference for video quality selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QualityPref {
    /// DASH mux for highest bitrate (requires ffmpeg).
    Max,
    /// Best direct CDN URL without muxing.
    High,
    /// 720p or lower.
    Medium,
}

/// An authenticated Instagram session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub csrf_token: String,
    pub ds_user_id: String,
    pub ig_did: String,
    pub username: Option<String>,
}

// ---------------------------------------------------------------------------
// MediaPost helpers
// ---------------------------------------------------------------------------

impl MediaPost {
    /// Returns `true` if any item in this post contains video content (direct URL
    /// or DASH manifest).
    pub fn is_video(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.video_url.is_some() || item.dash_manifest.is_some())
    }

    /// Returns `true` if any item in this post has a DASH manifest that can be
    /// used for high-quality muxing.
    pub fn has_dash(&self) -> bool {
        self.items.iter().any(|item| item.dash_manifest.is_some())
    }
}

// ---------------------------------------------------------------------------
// MediaItem helpers
// ---------------------------------------------------------------------------

impl MediaItem {
    /// Selects the best download URL for this item based on the given quality
    /// preference.
    ///
    /// - `Max`: prefers the direct `video_url` (DASH muxing is handled
    ///   separately by the download pipeline when a manifest is present).
    /// - `High`: same as `Max` -- returns the best available direct URL.
    /// - `Medium`: returns `video_url` if available, otherwise `photo_url`.
    ///
    /// For photos (no `video_url`), always returns `photo_url`.
    pub fn best_url(&self, quality: &QualityPref) -> Option<&str> {
        match quality {
            QualityPref::Max | QualityPref::High => self
                .video_url
                .as_deref()
                .or(self.photo_url.as_deref()),
            QualityPref::Medium => self
                .video_url
                .as_deref()
                .or(self.photo_url.as_deref()),
        }
    }

    /// Returns the file extension appropriate for this item's media type.
    pub fn file_extension(&self) -> &str {
        match self.media_type {
            MediaType::Video | MediaType::Reel => "mp4",
            MediaType::Photo => "jpg",
            // Stories and highlights could be either; fall back to checking
            // whether a video URL is present.
            MediaType::Story | MediaType::Highlight => {
                if self.video_url.is_some() || self.dash_manifest.is_some() {
                    "mp4"
                } else {
                    "jpg"
                }
            }
            // Carousel items are always wrapped in individual MediaItems with
            // their own media_type, so this branch shouldn't normally be hit.
            MediaType::Carousel => "jpg",
        }
    }
}

// ---------------------------------------------------------------------------
// Session helpers
// ---------------------------------------------------------------------------

impl Session {
    /// A session is valid when both the session ID and CSRF token are present.
    pub fn is_valid(&self) -> bool {
        !self.session_id.is_empty() && !self.csrf_token.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_video_item() -> MediaItem {
        MediaItem {
            id: "item1".into(),
            media_type: MediaType::Video,
            video_url: Some("https://cdn.instagram.com/video.mp4".into()),
            dash_manifest: Some("<MPD/>".into()),
            photo_url: Some("https://cdn.instagram.com/thumb.jpg".into()),
            width: 1080,
            height: 1920,
            duration_secs: Some(30.0),
        }
    }

    fn sample_photo_item() -> MediaItem {
        MediaItem {
            id: "item2".into(),
            media_type: MediaType::Photo,
            video_url: None,
            dash_manifest: None,
            photo_url: Some("https://cdn.instagram.com/photo.jpg".into()),
            width: 1080,
            height: 1080,
            duration_secs: None,
        }
    }

    #[test]
    fn test_is_video_with_video_item() {
        let post = MediaPost {
            id: "1".into(),
            shortcode: "ABC".into(),
            media_type: MediaType::Video,
            owner_username: "user".into(),
            caption: None,
            timestamp: 0,
            items: vec![sample_video_item()],
        };
        assert!(post.is_video());
    }

    #[test]
    fn test_is_video_with_photo_only() {
        let post = MediaPost {
            id: "2".into(),
            shortcode: "DEF".into(),
            media_type: MediaType::Photo,
            owner_username: "user".into(),
            caption: None,
            timestamp: 0,
            items: vec![sample_photo_item()],
        };
        assert!(!post.is_video());
    }

    #[test]
    fn test_has_dash() {
        let post = MediaPost {
            id: "3".into(),
            shortcode: "GHI".into(),
            media_type: MediaType::Video,
            owner_username: "user".into(),
            caption: None,
            timestamp: 0,
            items: vec![sample_video_item()],
        };
        assert!(post.has_dash());
    }

    #[test]
    fn test_best_url_video_max() {
        let item = sample_video_item();
        assert_eq!(
            item.best_url(&QualityPref::Max),
            Some("https://cdn.instagram.com/video.mp4")
        );
    }

    #[test]
    fn test_best_url_photo() {
        let item = sample_photo_item();
        assert_eq!(
            item.best_url(&QualityPref::High),
            Some("https://cdn.instagram.com/photo.jpg")
        );
    }

    #[test]
    fn test_file_extension() {
        assert_eq!(sample_video_item().file_extension(), "mp4");
        assert_eq!(sample_photo_item().file_extension(), "jpg");

        let reel = MediaItem {
            media_type: MediaType::Reel,
            ..sample_video_item()
        };
        assert_eq!(reel.file_extension(), "mp4");

        let story_video = MediaItem {
            media_type: MediaType::Story,
            ..sample_video_item()
        };
        assert_eq!(story_video.file_extension(), "mp4");

        let story_photo = MediaItem {
            media_type: MediaType::Story,
            ..sample_photo_item()
        };
        assert_eq!(story_photo.file_extension(), "jpg");
    }

    #[test]
    fn test_session_validity() {
        let valid = Session {
            session_id: "abc123".into(),
            csrf_token: "token".into(),
            ds_user_id: "12345".into(),
            ig_did: "did".into(),
            username: Some("user".into()),
        };
        assert!(valid.is_valid());

        let empty_sid = Session {
            session_id: String::new(),
            ..valid.clone()
        };
        assert!(!empty_sid.is_valid());

        let empty_csrf = Session {
            csrf_token: String::new(),
            ..valid
        };
        assert!(!empty_csrf.is_valid());
    }
}
