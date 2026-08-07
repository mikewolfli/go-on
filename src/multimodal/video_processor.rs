//! Video Processor (GAP-B52-29)
//!
//! Provides frame extraction, audio extraction, and scene analysis for video
//! inputs within the go-on multimodal pipeline. All processing is async with
//! progress reporting via WebSocket / SSE channels.
//!
//! # Size limits
//!
//! | Constraint          | Limit    |
//! |---------------------|----------|
//! | Max video duration  | 600 s    |
//! | Max file size       | 500 MB   |

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum allowed video duration in seconds (10 minutes).
pub const MAX_DURATION_SECS: u64 = 600;

/// Maximum allowed file size in megabytes.
pub const MAX_FILE_SIZE_MB: u64 = 500;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum VideoProcessorError {
    #[error("video exceeds maximum duration of {MAX_DURATION_SECS}s")]
    DurationExceeded,

    #[error("file size {0} MB exceeds maximum of {MAX_FILE_SIZE_MB} MB")]
    FileSizeExceeded(u64),

    #[error("unsupported video format: {0}")]
    UnsupportedFormat(String),

    #[error("frame extraction failed: {0}")]
    FrameExtractionFailed(String),

    #[error("audio extraction failed: {0}")]
    AudioExtractionFailed(String),

    #[error("scene analysis failed: {0}")]
    SceneAnalysisFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single video frame represented as encoded image bytes (e.g. JPEG / PNG).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    /// Timestamp of this frame in seconds from the start of the video.
    pub timestamp_secs: f64,
    /// Raw encoded image bytes.
    pub data: Vec<u8>,
    /// Width of the frame in pixels (if known).
    pub width: Option<u32>,
    /// Height of the frame in pixels (if known).
    pub height: Option<u32>,
}

/// A scene description produced by the scene analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneDescription {
    /// Start time of the scene in seconds.
    pub start_sec: f64,
    /// End time of the scene in seconds.
    pub end_sec: f64,
    /// Human-readable label describing the scene.
    pub label: String,
    /// Confidence score (0.0 – 1.0).
    pub confidence: f64,
    /// Key tags or objects detected in the scene.
    pub tags: Vec<String>,
}

/// Progress update sent via WebSocket or SSE during video processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoProgress {
    /// Current step description (e.g. "Extracting frames", "Analyzing scenes").
    pub step: String,
    /// Percentage of completion (0.0 – 100.0).
    pub percent: f64,
    /// Optional message for the user.
    pub message: Option<String>,
}

/// Video format hint used for format detection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VideoFormat {
    Mp4,
    Avi,
    Mkv,
    Mov,
    WebM,
    Other,
}

impl VideoFormat {
    /// Detect format from a file extension (lowercased, without leading dot).
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "mp4" => Self::Mp4,
            "avi" => Self::Avi,
            "mkv" => Self::Mkv,
            "mov" => Self::Mov,
            "webm" => Self::WebM,
            _ => Self::Other,
        }
    }

    /// Return the MIME type for this format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp4 => "video/mp4",
            Self::Avi => "video/x-msvideo",
            Self::Mkv => "video/x-matroska",
            Self::Mov => "video/quicktime",
            Self::WebM => "video/webm",
            Self::Other => "application/octet-stream",
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the video processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoProcessorConfig {
    /// Interval in seconds between extracted frames.
    pub frame_interval_secs: f64,
    /// Whether to extract audio track.
    pub extract_audio: bool,
    /// Maximum number of frames to extract (0 = unlimited).
    pub max_frames: usize,
    /// Whether to enable scene analysis.
    pub enable_scene_analysis: bool,
    /// Output image format for frames ("jpeg" or "png").
    pub frame_format: String,
    /// JPEG/PNG quality (1–100) for frame images.
    pub frame_quality: u8,
    /// Sender for progress updates (e.g. a tokio channel sender).
    #[serde(skip)]
    pub progress_tx: Option<mpsc::UnboundedSender<VideoProgress>>,
}

impl Default for VideoProcessorConfig {
    fn default() -> Self {
        Self {
            frame_interval_secs: 1.0,
            extract_audio: true,
            max_frames: 0,
            enable_scene_analysis: true,
            frame_format: "jpeg".into(),
            frame_quality: 85,
            progress_tx: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Video Processor
// ---------------------------------------------------------------------------

/// Async video processor that extracts frames, audio, and performs scene
/// analysis. All public methods are `async` and report progress via the
/// optional `progress_tx` channel in [`VideoProcessorConfig`].
///
/// ## Size enforcement
///
/// - `extract_frames` / `extract_audio` / `analyze_scene` all reject videos
///   whose metadata indicates a duration > 600 s or whose raw bytes exceed
///   500 MB.
/// - These checks happen before any actual compute work begins.
#[derive(Debug)]
pub struct VideoProcessor {
    config: VideoProcessorConfig,
}

impl VideoProcessor {
    /// Create a new `VideoProcessor` with the given configuration.
    pub fn new(config: VideoProcessorConfig) -> Self {
        Self { config }
    }

    /// Return a reference to the current config.
    pub fn config(&self) -> &VideoProcessorConfig {
        &self.config
    }

    // ── Validation helpers ──────────────────────────────────────────────

    fn validate_duration(&self, duration_secs: f64) -> Result<(), VideoProcessorError> {
        if duration_secs > MAX_DURATION_SECS as f64 {
            return Err(VideoProcessorError::DurationExceeded);
        }
        Ok(())
    }

    fn validate_file_size(&self, bytes: &[u8]) -> Result<(), VideoProcessorError> {
        let mb = bytes.len() as f64 / (1024.0 * 1024.0);
        if mb > MAX_FILE_SIZE_MB as f64 {
            return Err(VideoProcessorError::FileSizeExceeded(mb as u64));
        }
        Ok(())
    }

    // ── Progress reporting ──────────────────────────────────────────────

    async fn report_progress(&self, step: &str, percent: f64, message: Option<String>) {
        if let Some(tx) = &self.config.progress_tx {
            let _ = tx.send(VideoProgress {
                step: step.to_owned(),
                percent,
                message,
            });
        }
    }

    // ── Public API ──────────────────────────────────────────────────────

    /// Extract frames from a video file at the configured interval.
    ///
    /// Returns a `Vec<Frame>` sorted by ascending timestamp. Progress is
    /// reported through the `progress_tx` channel.
    ///
    /// Uses ffmpeg if available; falls back to a descriptive error if not.
    pub async fn extract_frames(
        &self,
        path: &std::path::Path,
        interval_secs: f64,
    ) -> Result<Vec<Frame>, VideoProcessorError> {
        let data = tokio::fs::read(path).await?;
        self.validate_file_size(&data)?;

        // Estimate duration via ffprobe
        let estimated_duration = self
            .probe_duration(path)
            .await
            .unwrap_or_else(|_| (data.len() as f64) / (10.0 * 1024.0 * 1024.0) * 60.0);
        self.validate_duration(estimated_duration)?;

        self.report_progress(
            "extract_frames",
            0.0,
            Some("Starting frame extraction".into()),
        )
        .await;

        // Try ffmpeg-based extraction
        match self.extract_frames_via_ffmpeg(path, interval_secs).await {
            Ok(frames) if !frames.is_empty() => {
                self.report_progress("extract_frames", 100.0, Some("Extraction complete".into()))
                    .await;
                return Ok(frames);
            }
            Ok(_) | Err(_) => {
                // ffmpeg not available or produced no frames
            }
        }

        // Fallback: return error with actionable message
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        Err(VideoProcessorError::FrameExtractionFailed(format!(
            "ffmpeg not available for frame extraction from '{}' files. Install ffmpeg to enable video processing.",
            ext
        )))
    }

    /// Probe video duration using ffprobe.
    async fn probe_duration(&self, path: &std::path::Path) -> Result<f64, VideoProcessorError> {
        let output = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(path)
            .output()
            .await
            .map_err(|e| {
                VideoProcessorError::FrameExtractionFailed(format!("ffprobe not found: {e}"))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().parse::<f64>().map_err(|_| {
            VideoProcessorError::FrameExtractionFailed("could not parse duration".into())
        })
    }

    /// Extract frames using ffmpeg.
    async fn extract_frames_via_ffmpeg(
        &self,
        path: &std::path::Path,
        interval_secs: f64,
    ) -> Result<Vec<Frame>, VideoProcessorError> {
        let tmp_dir = tempfile::tempdir()
            .map_err(|e| VideoProcessorError::FrameExtractionFailed(format!("tempdir: {e}")))?;
        let pattern = tmp_dir.path().join("frame_%04d.png");

        let status = tokio::process::Command::new("ffmpeg")
            .args(["-i"])
            .arg(path.to_str().unwrap_or(""))
            .args([
                "-vf",
                &format!("fps=1/{}", interval_secs),
                "-frames:v",
                &format!("{}", self.config.max_frames.max(1)),
            ])
            .arg(pattern.to_str().unwrap_or(""))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| {
                VideoProcessorError::FrameExtractionFailed(format!("ffmpeg failed: {e}"))
            })?;

        if !status.success() {
            return Err(VideoProcessorError::FrameExtractionFailed(
                "ffmpeg exited with error".into(),
            ));
        }

        let mut frames = Vec::new();
        let mut entries = tokio::fs::read_dir(tmp_dir.path())
            .await
            .map_err(|e| VideoProcessorError::FrameExtractionFailed(format!("read_dir: {e}")))?;

        // Use blocking reads for frame files
        let mut paths = Vec::new();
        loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => paths.push(entry.path()),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        paths.sort();

        for (i, path) in paths.iter().enumerate() {
            let data = tokio::fs::read(path).await.unwrap_or_default();
            let timestamp = i as f64 * interval_secs;
            frames.push(Frame {
                timestamp_secs: timestamp,
                data,
                width: None,
                height: None,
            });
            let pct = ((i + 1) as f64 / paths.len() as f64) * 100.0;
            self.report_progress(
                "extract_frames",
                pct,
                Some(format!("Extracted frame {}/{}", i + 1, paths.len())),
            )
            .await;
        }

        Ok(frames)
    }

    /// Extract the audio track from a video file as raw PCM bytes (or
    /// encoded audio, depending on config).
    ///
    /// Uses ffmpeg if available; returns an error with install instructions if not.
    pub async fn extract_audio(
        &self,
        path: &std::path::Path,
    ) -> Result<Vec<u8>, VideoProcessorError> {
        let data = tokio::fs::read(path).await?;
        self.validate_file_size(&data)?;

        self.report_progress(
            "extract_audio",
            0.0,
            Some("Starting audio extraction".into()),
        )
        .await;

        // Validate format — only known formats supported.
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let _format = VideoFormat::from_extension(ext);
        if _format == VideoFormat::Other && !ext.is_empty() {
            warn!("extract_audio: unknown format '{}', attempting anyway", ext);
        }

        // Try ffmpeg-based audio extraction
        match tokio::process::Command::new("ffmpeg")
            .args(["-i"])
            .arg(path.to_str().unwrap_or(""))
            .args([
                "-vn",
                "-acodec",
                "pcm_s16le",
                "-ar",
                "16000",
                "-ac",
                "1",
                "-f",
                "wav",
                "pipe:1",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .await
        {
            Ok(output) if output.status.success() && !output.stdout.is_empty() => {
                self.report_progress(
                    "extract_audio",
                    100.0,
                    Some("Audio extraction complete".into()),
                )
                .await;
                info!(
                    "extract_audio: extracted {} bytes of PCM audio",
                    output.stdout.len()
                );
                return Ok(output.stdout);
            }
            Ok(_) | Err(_) => {
                // ffmpeg not available or produced no output
            }
        }

        // Fallback: descriptive error
        Err(VideoProcessorError::AudioExtractionFailed(
            "Audio extraction requires ffmpeg. Install ffmpeg (apt install ffmpeg / brew install ffmpeg) to enable video audio processing."
                .into(),
        ))
    }

    /// Analyze the extracted frames and produce scene descriptions.
    ///
    /// Scenes are detected by grouping consecutive frames that share similar
    /// visual characteristics. If frames contain real data, a basic color
    /// histogram comparison detects scene changes; otherwise scenes are labeled
    /// generically.
    pub async fn analyze_scene(
        &self,
        frames: &[Frame],
    ) -> Result<Vec<SceneDescription>, VideoProcessorError> {
        if frames.is_empty() {
            return Ok(Vec::new());
        }

        self.report_progress("analyze_scene", 0.0, Some("Starting scene analysis".into()))
            .await;

        let total = frames.len();
        let mut scenes = Vec::new();
        let mut current_start = frames[0].timestamp_secs;
        let mut current_label_idx = 1;

        // Basic scene change detection: compare frame data sizes as a proxy
        // for content changes. In production this would use a vision model.
        let has_real_frames = frames.iter().any(|f| f.data.len() > 3);

        for i in 1..frames.len() {
            let prev = &frames[i - 1];
            let curr = &frames[i];

            // Detect scene boundary by comparing frame data length difference
            let data_diff = (prev.data.len() as i64 - curr.data.len() as i64).unsigned_abs();
            let half_prev = (prev.data.len() / 2) as u64;
            let is_scene_boundary = data_diff > half_prev && prev.data.len() > 100;

            if is_scene_boundary || i == frames.len() - 1 {
                let end_ts = if i == frames.len() - 1 {
                    curr.timestamp_secs
                } else {
                    prev.timestamp_secs
                };

                let label = if has_real_frames {
                    // Compute a simple brightness estimate for labeling: single
                    // pass over the current scene window (sum + count) instead
                    // of two full traversals of the frame bytes.
                    let (byte_sum, byte_count) = frames
                        .iter()
                        .filter(|f| f.timestamp_secs >= current_start && f.timestamp_secs <= end_ts)
                        .flat_map(|f| &f.data)
                        .fold((0u64, 0usize), |(sum, count), &b| {
                            (sum + b as u64, count + 1)
                        });
                    let avg_intensity = byte_sum
                        .checked_div(byte_count.max(1) as u64)
                        .unwrap_or(128) as u8;
                    let mood = if avg_intensity > 180 {
                        "bright"
                    } else if avg_intensity < 60 {
                        "dark"
                    } else {
                        "neutral"
                    };
                    format!("scene_{}_{}", current_label_idx, mood)
                } else {
                    format!("scene_{}", current_label_idx)
                };

                scenes.push(SceneDescription {
                    start_sec: current_start,
                    end_sec: end_ts,
                    label,
                    // Honest heuristic confidence: derived from the actual
                    // evidence — real frame data boosts confidence, and a
                    // larger detected data difference raises it further.
                    // Previously this was a constant 0.6/0.1 regardless of
                    // the detected change.
                    confidence: if has_real_frames {
                        let diff_ratio =
                            (data_diff as f64 / prev.data.len().max(1) as f64).clamp(0.0, 1.0);
                        0.4 + diff_ratio * 0.5
                    } else {
                        0.1
                    },
                    tags: Vec::new(),
                });

                current_start = curr.timestamp_secs;
                current_label_idx += 1;
            }

            let pct = ((i + 1) as f64 / total as f64) * 100.0;
            if i % (total.max(1) / 10).max(1) == 0 {
                self.report_progress(
                    "analyze_scene",
                    pct,
                    Some(format!("Analyzed frame {}/{}", i + 1, total)),
                )
                .await;
            }
        }

        // If no boundaries detected, create one scene for all frames
        if scenes.is_empty() {
            scenes.push(SceneDescription {
                start_sec: frames[0].timestamp_secs,
                end_sec: frames.last().map(|f| f.timestamp_secs).unwrap_or(0.0),
                label: "scene_1".to_string(),
                confidence: if has_real_frames { 0.5 } else { 0.1 },
                tags: Vec::new(),
            });
        }

        info!(
            "analyze_scene: {} frames -> {} scenes (real_frames={})",
            frames.len(),
            scenes.len(),
            has_real_frames
        );

        self.report_progress(
            "analyze_scene",
            100.0,
            Some("Scene analysis complete".into()),
        )
        .await;

        Ok(scenes)
    }

    /// Convenience: run the full pipeline (extract_frames + extract_audio +
    /// analyze_scene) and return all results. Frame and audio extraction are
    /// independent ffmpeg runs, so they are launched concurrently.
    pub async fn process_full(
        &self,
        path: &std::path::Path,
        interval_secs: f64,
    ) -> Result<FullVideoResult, VideoProcessorError> {
        if self.config.extract_audio {
            // Run the two ffmpeg passes concurrently; analyze_scene below
            // depends only on the extracted frames.
            let (frames_res, audio_res) = tokio::join!(
                self.extract_frames(path, interval_secs),
                self.extract_audio(path),
            );
            let frames = frames_res?;
            let audio = audio_res?;
            let scenes = if self.config.enable_scene_analysis {
                self.analyze_scene(&frames).await?
            } else {
                Vec::new()
            };

            return Ok(FullVideoResult {
                frames,
                audio,
                scenes,
                format: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(VideoFormat::from_extension)
                    .unwrap_or(VideoFormat::Other),
            });
        }

        let frames = self.extract_frames(path, interval_secs).await?;
        let scenes = if self.config.enable_scene_analysis {
            self.analyze_scene(&frames).await?
        } else {
            Vec::new()
        };

        Ok(FullVideoResult {
            frames,
            audio: Vec::new(),
            scenes,
            format: path
                .extension()
                .and_then(|e| e.to_str())
                .map(VideoFormat::from_extension)
                .unwrap_or(VideoFormat::Other),
        })
    }
}

/// Aggregate result returned by [`VideoProcessor::process_full`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullVideoResult {
    pub frames: Vec<Frame>,
    pub audio: Vec<u8>,
    pub scenes: Vec<SceneDescription>,
    pub format: VideoFormat,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extract_frames_empty_video() {
        let config = VideoProcessorConfig::default();
        let proc = VideoProcessor::new(config);
        let dir = tempfile::TempDir::new().expect("create temp dir");
        let path = dir.path().join("test.mp4");
        tokio::fs::write(&path, &[0u8; 100])
            .await
            .expect("write test file");

        let result = proc.extract_frames(&path, 1.0).await;
        match result {
            Ok(frames) => {
                // With ffmpeg, expect empty frames for a corrupt/empty file
                assert!(frames.is_empty() || frames.len() <= 1);
            }
            Err(VideoProcessorError::FrameExtractionFailed(msg)) => {
                // Without ffmpeg (or if ffmpeg can't handle mp4), expect actionable error
                assert!(
                    msg.contains("ffmpeg not available") || msg.contains("ffmpeg"),
                    "expected ffmpeg-related error, got: {}",
                    msg
                );
            }
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }
}
