//! Persistent source-time webcam decoding. The screen/audio transport owns
//! the clock; this decoder follows timestamps rather than running a second clock.
use super::video::{VideoError, VideoFrameStream};
use image::RgbaImage;
use std::{path::PathBuf, sync::Arc};

/// Keep the master clock stopped while the camera warms up. Short waits
/// allow pause, seek, or closing the preview to cancel startup promptly.
pub fn wait_until_ready(
    ready: &std::sync::mpsc::Receiver<()>,
    cancelled: impl Fn() -> bool,
) -> bool {
    while !cancelled() {
        match ready.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(()) => return !cancelled(),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
    false
}

pub struct CameraPlaybackDecoder {
    path: PathBuf,
    stream: Option<VideoFrameStream>,
    current: Option<(f64, Arc<RgbaImage>)>,
    exhausted_at: Option<f64>,
    #[cfg(test)]
    opens: usize,
}

impl CameraPlaybackDecoder {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            stream: None,
            current: None,
            exhausted_at: None,
            #[cfg(test)]
            opens: 0,
        }
    }

    pub fn frame_at(
        &mut self,
        target: Option<f64>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<(f64, Arc<RgbaImage>)>, VideoError> {
        let Some(target) = target else {
            self.stream = None;
            self.current = None;
            self.exhausted_at = None;
            return Ok(None);
        };
        if cancelled() {
            return Ok(None);
        }
        // A webcam can end before the screen recording. Do not repeatedly
        // launch decoders for its missing tail; seek again only when moving back.
        if self.exhausted_at.is_some_and(|end| target >= end) {
            return Ok(self.current.clone());
        }
        let restart = self
            .current
            .as_ref()
            .is_none_or(|(time, _)| target < *time - 1e-6 || target - *time > 0.5);
        if restart {
            self.stream = None;
            self.current = None;
            self.exhausted_at = None;
            self.stream = Some(VideoFrameStream::open_with_frame_rate(
                &self.path,
                target,
                640,
                640,
                Some(30.0),
            )?);
            #[cfg(test)]
            {
                self.opens += 1;
            }
        }
        let stream = self.stream.as_mut().unwrap();
        while self.current.is_none() || stream.next_time() <= target + 1e-6 {
            if cancelled() {
                return Ok(None);
            }
            let Some(frame) = stream.next_frame()? else {
                self.exhausted_at = Some(target);
                break;
            };
            let image = RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                .ok_or_else(|| VideoError::Decode("invalid webcam frame".into()))?;
            self.current = Some((frame.time, Arc::new(image)));
        }
        Ok(self.current.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn playback_waits_for_camera_readiness_and_startup_can_be_cancelled() {
        use std::{
            sync::{
                atomic::{AtomicBool, Ordering},
                mpsc,
            },
            time::Duration,
        };
        let (ready, receiver) = mpsc::sync_channel(1);
        let (started, status) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(wait_until_ready(&receiver, || false)).unwrap();
        });
        assert!(
            status.recv_timeout(Duration::from_millis(60)).is_err(),
            "master clock started before camera was ready"
        );
        ready.send(()).unwrap();
        assert!(status.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();

        let (_ready, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (stopped, status) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            stopped
                .send(wait_until_ready(&receiver, || {
                    cancelled.load(Ordering::SeqCst)
                }))
                .unwrap();
        });
        cancel.store(true, Ordering::SeqCst);
        assert!(!status.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();

        let (ready, receiver) = mpsc::sync_channel(1);
        drop(ready);
        assert!(!wait_until_ready(&receiver, || false));
    }

    #[test]
    fn follows_source_time_reuses_decoder_and_handles_cuts_and_gaps() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("lahza-camera-sync-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("camera.mkv");
        let output = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=96x72:r=30:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=96x72:r=30:d=1",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                "-map",
                "[v]",
                "-c:v",
                "ffv1",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut decoder = CameraPlaybackDecoder::new(path);
        // Uneven increments simulate slow motion, speed changes and UI frame drops.
        for time in [0.1, 0.12, 0.15, 0.3, 0.6, 0.9, 1.1, 1.4] {
            let (decoded_time, image) = decoder.frame_at(Some(time), || false).unwrap().unwrap();
            assert!(decoded_time <= time + 1e-6 && time - decoded_time < 1.0 / 30.0 + 1e-6);
            let pixel = image.get_pixel(48, 36).0;
            assert!(if time < 1.0 {
                pixel[0] > 200 && pixel[2] < 30
            } else {
                pixel[2] > 200 && pixel[0] < 30
            });
        }
        assert_eq!(
            decoder.opens, 1,
            "continuous playback must reuse the decoder"
        );
        let (_, image) = decoder.frame_at(Some(0.2), || false).unwrap().unwrap();
        assert!(image.get_pixel(48, 36).0[0] > 200);
        assert_eq!(decoder.opens, 2, "backward cut seeks the camera");
        assert!(decoder.frame_at(None, || false).unwrap().is_none());
        assert!(decoder.current.is_none() && decoder.stream.is_none());
        let (_, image) = decoder.frame_at(Some(1.2), || false).unwrap().unwrap();
        assert!(image.get_pixel(48, 36).0[2] > 200);
        assert_eq!(decoder.opens, 3);
        assert!(decoder.frame_at(Some(0.0), || true).unwrap().is_none());
        assert_eq!(decoder.opens, 3, "cancel must not open another process");
        decoder.frame_at(Some(2.5), || false).unwrap();
        let opens = decoder.opens;
        decoder.frame_at(Some(2.7), || false).unwrap();
        decoder.frame_at(Some(3.0), || false).unwrap();
        assert_eq!(
            decoder.opens, opens,
            "an ended webcam must not restart every frame"
        );
        let (_, image) = decoder.frame_at(Some(0.3), || false).unwrap().unwrap();
        assert!(image.get_pixel(48, 36).0[0] > 200);
        drop(decoder);
        fs::remove_dir_all(root).unwrap();
    }
}
