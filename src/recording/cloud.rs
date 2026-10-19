//! Cloud sharing to a self-hosted upload worker (the Screendrop protocol):
//! `PUT /api/upload` streams the video with metadata headers and returns the
//! share link; `POST /api/assets/{id}` adds the poster, a storyboard sprite
//! and the transcript to the share page. HTTP goes through `curl`, like the
//! speech model download. The token is passed in a private header file so it
//! never appears in the process list.

use super::transcript::Transcript;
use super::video::{probe_media, VideoError};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where uploads go, saved with owner-only permissions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudSettings {
    pub worker_url: String,
    pub upload_token: String,
    /// Allow comments and likes on new share pages.
    pub social_enabled: bool,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            worker_url: String::new(),
            upload_token: String::new(),
            social_enabled: true,
        }
    }
}

impl CloudSettings {
    const FILE_NAME: &'static str = "cloud.json";

    fn path() -> PathBuf {
        super::presets::config_root().join(Self::FILE_NAME)
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        write_private(&temporary, &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(temporary, path)
    }

    pub fn is_configured(&self) -> bool {
        !self.worker_url.trim().is_empty() && !self.upload_token.trim().is_empty()
    }

    /// `https://` is assumed when no scheme is given; trailing slashes go.
    pub fn base_url(&self) -> String {
        let trimmed = self.worker_url.trim().trim_end_matches('/');
        if trimmed.to_ascii_lowercase().starts_with("http") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        }
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadResult {
    pub id: String,
    pub url: String,
}

/// Owner-only scratch directory removed when the share ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("lahza-share-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn header_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

/// Percent-encode a header value the way the Swift client does.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~ ".contains(&byte) {
            encoded.push(if byte == b' ' { '+' } else { byte as char });
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded.replace('+', "%20")
}

/// Upload `video` and its share-page sidecars; returns the share link.
/// `on_progress` receives the upload fraction.
pub fn share_video(
    settings: &CloudSettings,
    video: &Path,
    title: Option<&str>,
    transcript: Option<&Transcript>,
    cancelled: impl Fn() -> bool,
    on_progress: impl Fn(f64),
) -> Result<UploadResult, VideoError> {
    if !settings.is_configured() {
        return Err(VideoError::InvalidMedia("cloud sharing is not set up".into()));
    }
    let info = probe_media(video)?;
    let scratch = Scratch::new()?;
    let headers = scratch.0.join("headers");
    let filename = video
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording.mp4".into());
    let mut lines = vec![
        format!("Authorization: Bearer {}", header_value(settings.upload_token.trim())),
        "Content-Type: video/mp4".to_string(),
        format!("X-Filename: {}", header_value(&filename)),
        "X-Media-Type: video".to_string(),
        format!("X-Width: {}", info.width),
        format!("X-Height: {}", info.height),
        format!("X-Duration: {}", info.duration),
        format!("X-Social-Enabled: {}", settings.social_enabled),
    ];
    if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
        lines.push(format!("X-Title: {}", percent_encode(title)));
    }
    write_private(&headers, lines.join("\n").as_bytes())?;
    let response = scratch.0.join("response.json");
    let mut child = Command::new("curl")
        .args(["--show-error", "--progress-bar", "--fail-with-body", "-X", "PUT"])
        .arg("-H")
        .arg(format!("@{}", headers.display()))
        .arg("--data-binary")
        .arg(format!("@{}", video.display()))
        .arg("--output")
        .arg(&response)
        .args(["--write-out", "%{http_code}"])
        .arg(format!("{}/api/upload", settings.base_url()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| VideoError::InvalidMedia(format!("could not start curl: {error}")))?;
    let stderr = child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        // curl's progress bar ends each update with "\r" and a percentage.
        let mut tail = String::new();
        let mut reader = BufReader::new(stderr);
        let mut chunk = Vec::new();
        while reader.read_until(b'\r', &mut chunk).map(|count| count > 0).unwrap_or(false) {
            let text = String::from_utf8_lossy(&chunk).to_string();
            chunk.clear();
            let _ = sender.send(text.clone());
            tail.push_str(&text);
            if tail.len() > 2000 {
                tail.drain(..tail.len() - 2000);
            }
        }
        tail
    });
    let status = loop {
        for text in receiver.try_iter() {
            if let Some(percent) = text
                .split_whitespace()
                .filter_map(|token| token.strip_suffix('%'))
                .filter_map(|value| value.parse::<f64>().ok())
                .last()
            {
                on_progress((percent / 100.0).clamp(0.0, 1.0));
            }
        }
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(VideoError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let mut code = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut code);
    }
    let messages = reader.join().unwrap_or_default();
    let body = std::fs::read_to_string(&response).unwrap_or_default();
    if !status.success() || code.trim() != "201" {
        let detail = if body.trim().is_empty() {
            messages
                .lines()
                .rev()
                .find(|line| line.contains("curl:"))
                .unwrap_or("")
                .trim()
                .to_string()
        } else {
            body.trim().chars().take(300).collect()
        };
        return Err(VideoError::InvalidMedia(format!(
            "the upload failed (HTTP {}): {detail}",
            code.trim()
        )));
    }
    let result: UploadResult = serde_json::from_str(&body)
        .map_err(|error| VideoError::InvalidMedia(format!("unexpected upload response: {error}")))?;
    on_progress(1.0);
    // Sidecars enrich the share page; the link already works without them.
    let _ = upload_sidecars(settings, &result.id, video, info.duration, title, transcript, &scratch.0);
    Ok(result)
}

#[derive(Serialize)]
struct TranscriptPayload<'a> {
    cues: Vec<PayloadCue<'a>>,
    words: Vec<PayloadWord<'a>>,
}

#[derive(Serialize)]
struct PayloadCue<'a> {
    start: f64,
    end: f64,
    text: &'a str,
}

#[derive(Serialize)]
struct PayloadWord<'a> {
    text: &'a str,
    start: f64,
    end: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoryboardMeta {
    tile_width: u32,
    tile_height: u32,
    columns: u32,
    interval: f64,
    count: u32,
}

const STORYBOARD_TILE_WIDTH: u32 = 160;
const MAXIMUM_STORYBOARD_TILES: f64 = 100.0;

fn upload_sidecars(
    settings: &CloudSettings,
    id: &str,
    video: &Path,
    duration: f64,
    title: Option<&str>,
    transcript: Option<&Transcript>,
    scratch: &Path,
) -> Result<(), VideoError> {
    let mut form: Vec<String> = Vec::new();
    let title = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| chrono::Local::now().format("Screen Recording - %b %-d at %-I:%M %p").to_string());
    let title_file = scratch.join("title.txt");
    std::fs::write(&title_file, &title)?;
    form.push(format!("title=<{}", title_file.display()));
    let poster = scratch.join("poster.jpg");
    let seek = (duration / 2.0).min(0.5).max(0.0);
    if ffmpeg(&[
        "-ss", &format!("{seek:.3}"), "-i", &video.to_string_lossy(), "-frames:v", "1",
        "-vf", "scale='min(1600,iw)':-2", "-q:v", "3", &poster.to_string_lossy(),
    ]) {
        form.push(format!("poster=@{};type=image/jpeg;filename=poster.jpg", poster.display()));
    }
    if duration >= 2.0 {
        let interval = (duration / MAXIMUM_STORYBOARD_TILES).ceil().max(1.0);
        let count = ((duration / interval).ceil() as u32).max(1);
        let columns = (count as f64).sqrt().ceil() as u32;
        let rows = count.div_ceil(columns);
        let storyboard = scratch.join("storyboard.jpg");
        let info = probe_media(video)?;
        let mut tile_height = ((STORYBOARD_TILE_WIDTH as f64 * info.height as f64 / info.width.max(1) as f64).round() as u32).max(2);
        tile_height += tile_height % 2;
        if ffmpeg(&[
            "-i", &video.to_string_lossy(),
            "-vf", &format!(
                "fps=1/{interval},scale={STORYBOARD_TILE_WIDTH}:{tile_height},tile={columns}x{rows}"
            ),
            "-frames:v", "1", "-q:v", "5", &storyboard.to_string_lossy(),
        ]) {
            let meta = serde_json::to_string(&StoryboardMeta {
                tile_width: STORYBOARD_TILE_WIDTH,
                tile_height,
                columns,
                interval,
                count,
            })
            .map_err(|error| VideoError::InvalidMedia(error.to_string()))?;
            let meta_file = scratch.join("storyboard.json");
            std::fs::write(&meta_file, meta)?;
            form.push(format!("storyboard=@{};type=image/jpeg;filename=storyboard.jpg", storyboard.display()));
            form.push(format!("storyboard_meta=<{}", meta_file.display()));
        }
    }
    if let Some(transcript) = transcript.filter(|transcript| !transcript.cues.is_empty()) {
        let payload = TranscriptPayload {
            cues: transcript
                .cues
                .iter()
                .map(|cue| PayloadCue { start: cue.start, end: cue.end, text: &cue.text })
                .collect(),
            words: transcript
                .words
                .iter()
                .map(|word| PayloadWord { text: &word.text, start: word.start, end: word.end })
                .collect(),
        };
        let file = scratch.join("transcript.json");
        std::fs::write(&file, serde_json::to_vec(&payload).map_err(|error| VideoError::InvalidMedia(error.to_string()))?)?;
        form.push(format!("transcript=<{}", file.display()));
    }
    let headers = scratch.join("asset-headers");
    write_private(&headers, format!("Authorization: Bearer {}", header_value(settings.upload_token.trim())).as_bytes())?;
    let mut command = Command::new("curl");
    command
        .args(["--silent", "--show-error", "--fail-with-body", "--max-time", "120", "-X", "POST"])
        .arg("-H")
        .arg(format!("@{}", headers.display()));
    for field in &form {
        command.arg("-F").arg(field);
    }
    let output = command
        .arg(format!("{}/api/assets/{id}", settings.base_url()))
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(VideoError::InvalidMedia(String::from_utf8_lossy(&output.stdout).trim().into()));
    }
    Ok(())
}

fn ffmpeg(arguments: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y"])
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_url_and_title_encoding() {
        let settings = CloudSettings {
            worker_url: " share.example.com/ ".into(),
            upload_token: "token".into(),
            social_enabled: true,
        };
        assert_eq!(settings.base_url(), "https://share.example.com");
        assert!(settings.is_configured());
        assert_eq!(percent_encode("Demo: v2 & more"), "Demo%3A%20v2%20%26%20more");
    }
}
