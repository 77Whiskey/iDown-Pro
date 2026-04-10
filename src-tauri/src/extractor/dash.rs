use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

use crate::utils::error::ExtractorError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Parsed DASH streams separated into video and audio tracks.
#[derive(Debug, Clone, Serialize)]
pub struct DashStreams {
    pub video: Vec<DashRepresentation>,
    pub audio: Vec<DashRepresentation>,
}

/// A single DASH representation (one quality level of video or audio).
#[derive(Debug, Clone, Serialize)]
pub struct DashRepresentation {
    pub id: String,
    pub base_url: String,
    pub bandwidth: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codecs: String,
    pub mime_type: String,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse an Instagram DASH MPD manifest XML string into separated video and
/// audio streams.
///
/// Instagram embeds DASH manifests as XML strings inside GraphQL responses.
/// Each manifest contains `<AdaptationSet>` elements with a `contentType`
/// attribute ("video" or "audio"), and nested `<Representation>` elements
/// with quality attributes and a `<BaseURL>` child containing the CDN URL.
pub fn parse_dash_manifest(mpd_xml: &str) -> Result<DashStreams, ExtractorError> {
    let mut reader = Reader::from_str(mpd_xml);

    let mut streams = DashStreams {
        video: Vec::new(),
        audio: Vec::new(),
    };

    // State tracked while walking the XML tree.
    let mut current_content_type: Option<String> = None;
    let mut current_mime_type: Option<String> = None;
    let mut current_rep: Option<PartialRep> = None;
    let mut inside_base_url = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"AdaptationSet" => {
                        current_content_type = None;
                        current_mime_type = None;
                        for attr in e.attributes().flatten() {
                            match attr.key.local_name().as_ref() {
                                b"contentType" => {
                                    current_content_type = Some(
                                        String::from_utf8_lossy(&attr.value).to_string(),
                                    );
                                }
                                b"mimeType" => {
                                    current_mime_type = Some(
                                        String::from_utf8_lossy(&attr.value).to_string(),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    b"Representation" => {
                        let mut rep = PartialRep::default();
                        // Inherit mime_type from parent AdaptationSet if set.
                        if let Some(ref mt) = current_mime_type {
                            rep.mime_type = Some(mt.clone());
                        }
                        for attr in e.attributes().flatten() {
                            match attr.key.local_name().as_ref() {
                                b"id" => {
                                    rep.id = Some(
                                        String::from_utf8_lossy(&attr.value).to_string(),
                                    );
                                }
                                b"bandwidth" => {
                                    rep.bandwidth = String::from_utf8_lossy(&attr.value)
                                        .parse::<u64>()
                                        .ok();
                                }
                                b"width" => {
                                    rep.width = String::from_utf8_lossy(&attr.value)
                                        .parse::<u32>()
                                        .ok();
                                }
                                b"height" => {
                                    rep.height = String::from_utf8_lossy(&attr.value)
                                        .parse::<u32>()
                                        .ok();
                                }
                                b"codecs" => {
                                    rep.codecs = Some(
                                        String::from_utf8_lossy(&attr.value).to_string(),
                                    );
                                }
                                b"mimeType" => {
                                    rep.mime_type = Some(
                                        String::from_utf8_lossy(&attr.value).to_string(),
                                    );
                                }
                                _ => {}
                            }
                        }
                        current_rep = Some(rep);

                        // If this was an empty element (<Representation ... />),
                        // we won't see an End event, but we also won't get a
                        // BaseURL, so we skip finalising here.
                    }
                    b"BaseURL" => {
                        inside_base_url = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if inside_base_url => {
                if let Some(ref mut rep) = current_rep {
                    let text = e.unescape().map_err(|err| ExtractorError::ParseError {
                        message: format!("failed to unescape BaseURL text: {err}"),
                    })?;
                    rep.base_url = Some(text.to_string());
                }
            }
            Ok(Event::End(ref e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"BaseURL" => {
                        inside_base_url = false;
                    }
                    b"Representation" => {
                        if let Some(rep) = current_rep.take() {
                            if let Some(finished) = rep.try_finish() {
                                match current_content_type.as_deref() {
                                    Some("video") => streams.video.push(finished),
                                    Some("audio") => streams.audio.push(finished),
                                    // Fall back to inferring from mime_type.
                                    _ => {
                                        if finished.mime_type.starts_with("video") {
                                            streams.video.push(finished);
                                        } else if finished.mime_type.starts_with("audio") {
                                            streams.audio.push(finished);
                                        }
                                        // Otherwise silently skip unknown tracks.
                                    }
                                }
                            }
                        }
                    }
                    b"AdaptationSet" => {
                        current_content_type = None;
                        current_mime_type = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractorError::ParseError {
                    message: format!("malformed DASH manifest XML: {err}"),
                });
            }
            _ => {}
        }
    }

    Ok(streams)
}

// ---------------------------------------------------------------------------
// Selection helpers
// ---------------------------------------------------------------------------

/// Returns the video representation with the highest bandwidth.
pub fn select_best_video(streams: &DashStreams) -> Option<&DashRepresentation> {
    streams.video.iter().max_by_key(|r| r.bandwidth)
}

/// Returns the audio representation with the highest bandwidth.
pub fn select_best_audio(streams: &DashStreams) -> Option<&DashRepresentation> {
    streams.audio.iter().max_by_key(|r| r.bandwidth)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Intermediate builder struct used while parsing attributes and children of
/// a `<Representation>` element. Fields are `Option` because we accumulate
/// them incrementally.
#[derive(Default)]
struct PartialRep {
    id: Option<String>,
    base_url: Option<String>,
    bandwidth: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    codecs: Option<String>,
    mime_type: Option<String>,
}

impl PartialRep {
    /// Attempt to produce a fully-formed `DashRepresentation`. Returns `None`
    /// if any required field is missing.
    fn try_finish(self) -> Option<DashRepresentation> {
        Some(DashRepresentation {
            id: self.id.unwrap_or_default(),
            base_url: self.base_url?,
            bandwidth: self.bandwidth.unwrap_or(0),
            width: self.width,
            height: self.height,
            codecs: self.codecs.unwrap_or_default(),
            mime_type: self.mime_type.unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011">
  <Period>
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <Representation id="1" bandwidth="500000" width="640" height="640" codecs="avc1.4d401e">
        <BaseURL>https://cdn.instagram.com/video_low.mp4</BaseURL>
      </Representation>
      <Representation id="2" bandwidth="2000000" width="1080" height="1080" codecs="avc1.640028">
        <BaseURL>https://cdn.instagram.com/video_high.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <Representation id="3" bandwidth="128000" codecs="mp4a.40.2">
        <BaseURL>https://cdn.instagram.com/audio.m4a</BaseURL>
      </Representation>
      <Representation id="4" bandwidth="64000" codecs="mp4a.40.2">
        <BaseURL>https://cdn.instagram.com/audio_low.m4a</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn test_parse_sample_manifest() {
        let streams = parse_dash_manifest(SAMPLE_MPD).expect("should parse valid MPD");
        assert_eq!(streams.video.len(), 2);
        assert_eq!(streams.audio.len(), 2);

        // Verify first video representation.
        let v1 = &streams.video[0];
        assert_eq!(v1.id, "1");
        assert_eq!(v1.bandwidth, 500_000);
        assert_eq!(v1.width, Some(640));
        assert_eq!(v1.height, Some(640));
        assert_eq!(v1.codecs, "avc1.4d401e");
        assert_eq!(v1.base_url, "https://cdn.instagram.com/video_low.mp4");

        // Verify second video representation.
        let v2 = &streams.video[1];
        assert_eq!(v2.id, "2");
        assert_eq!(v2.bandwidth, 2_000_000);
        assert_eq!(v2.width, Some(1080));
        assert_eq!(v2.height, Some(1080));

        // Verify audio representations.
        let a1 = &streams.audio[0];
        assert_eq!(a1.id, "3");
        assert_eq!(a1.bandwidth, 128_000);
        assert_eq!(a1.base_url, "https://cdn.instagram.com/audio.m4a");
    }

    #[test]
    fn test_select_best_video() {
        let streams = parse_dash_manifest(SAMPLE_MPD).unwrap();
        let best = select_best_video(&streams).expect("should find best video");
        assert_eq!(best.id, "2");
        assert_eq!(best.bandwidth, 2_000_000);
        assert_eq!(best.width, Some(1080));
    }

    #[test]
    fn test_select_best_audio() {
        let streams = parse_dash_manifest(SAMPLE_MPD).unwrap();
        let best = select_best_audio(&streams).expect("should find best audio");
        assert_eq!(best.id, "3");
        assert_eq!(best.bandwidth, 128_000);
    }

    #[test]
    fn test_malformed_xml_returns_parse_error() {
        let bad_xml = "<MPD><Period><broken";
        let result = parse_dash_manifest(bad_xml);
        assert!(result.is_err());
        match result.unwrap_err() {
            ExtractorError::ParseError { message } => {
                assert!(
                    message.contains("malformed"),
                    "error message should mention malformed XML, got: {message}"
                );
            }
            other => panic!("expected ParseError, got: {other:?}"),
        }
    }

    #[test]
    fn test_empty_manifest() {
        let empty_mpd = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011">
  <Period>
  </Period>
</MPD>"#;
        let streams = parse_dash_manifest(empty_mpd).expect("empty MPD should still parse");
        assert!(streams.video.is_empty());
        assert!(streams.audio.is_empty());
        assert!(select_best_video(&streams).is_none());
        assert!(select_best_audio(&streams).is_none());
    }

    #[test]
    fn test_manifest_without_content_type_uses_mime_fallback() {
        // Some manifests may omit contentType on AdaptationSet but include
        // mimeType on the Representation itself.
        let mpd = r#"<MPD>
  <Period>
    <AdaptationSet>
      <Representation id="v1" bandwidth="1000000" width="720" height="720"
                      codecs="avc1.4d401e" mimeType="video/mp4">
        <BaseURL>https://cdn.example.com/v.mp4</BaseURL>
      </Representation>
      <Representation id="a1" bandwidth="96000"
                      codecs="mp4a.40.2" mimeType="audio/mp4">
        <BaseURL>https://cdn.example.com/a.m4a</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let streams = parse_dash_manifest(mpd).unwrap();
        assert_eq!(streams.video.len(), 1);
        assert_eq!(streams.audio.len(), 1);
    }

    #[test]
    fn test_representation_missing_base_url_is_skipped() {
        let mpd = r#"<MPD>
  <Period>
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <Representation id="no_url" bandwidth="500000" width="640" height="640"
                      codecs="avc1.4d401e">
      </Representation>
      <Representation id="has_url" bandwidth="1000000" width="1080" height="1080"
                      codecs="avc1.640028">
        <BaseURL>https://cdn.example.com/good.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let streams = parse_dash_manifest(mpd).unwrap();
        // The representation without a BaseURL should be silently skipped.
        assert_eq!(streams.video.len(), 1);
        assert_eq!(streams.video[0].id, "has_url");
    }
}
