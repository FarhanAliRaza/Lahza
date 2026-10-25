//! Shared speech processing preferences and evidence from Whisper itself.
//!
//! Discovering a GPU, loading its plugin, or requesting `use gpu = 1` does
//! not mean the model is running there. Only backend initialization confirms it.

use super::transcription::JobProgress;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessingMode {
    #[default]
    Auto,
    Gpu,
    Cpu,
}

impl ProcessingMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Gpu, Self::Cpu];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Gpu => "GPU",
            Self::Cpu => "CPU",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Preferences {
    processing: ProcessingMode,
}

pub fn load_processing_mode() -> ProcessingMode {
    load_from(&super::presets::config_root().join("speech.json"))
}

fn load_from(path: &Path) -> ProcessingMode {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
        .unwrap_or_default()
        .processing
}

pub fn save_processing_mode(mode: ProcessingMode) -> io::Result<()> {
    save_to(&super::presets::config_root().join("speech.json"), mode)
}

fn save_to(path: &Path, processing: ProcessingMode) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&Preferences { processing }).map_err(io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}

pub fn data_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("lahza")
}

pub fn command(executable: &Path) -> Command {
    command_with_mode(executable, load_processing_mode())
}

pub fn command_with_mode(executable: &Path, mode: ProcessingMode) -> Command {
    let mut command = Command::new(executable);
    if mode == ProcessingMode::Cpu {
        // Loading a Vulkan plugin enumerates drivers even with `-ng`. A
        // forced CPU launch must work when the GPU driver itself is broken.
        command.arg("-ng").env_remove("GGML_BACKEND_PATH");
    }
    let runtime = executable.parent().and_then(Path::parent).filter(|root| {
        root.file_name().is_some_and(|name| name == "whisper") && root.join("lib").is_dir()
    });
    if let Some(runtime) = runtime {
        // The runtime and its ggml plugins are built together. Never inject a
        // plugin from the user's system into this private library directory.
        let library = runtime.join("lib");
        let mut paths = vec![library.clone()];
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(paths) = std::env::join_paths(paths) {
            command.env("LD_LIBRARY_PATH", paths);
        }
        if mode != ProcessingMode::Cpu && std::env::var_os("GGML_BACKEND_PATH").is_none() {
            let backend = library.join("libggml-vulkan.so");
            if backend.is_file() {
                command.env("GGML_BACKEND_PATH", backend);
            }
        }
    } else if mode != ProcessingMode::Cpu && std::env::var_os("GGML_BACKEND_PATH").is_none() {
        let backend = data_root().join("backends/libggml-vulkan.so");
        if backend.is_file() {
            command.env("GGML_BACKEND_PATH", backend);
        }
    }
    command
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RuntimeStatus {
    #[default]
    Checking,
    Cpu {
        reason: String,
    },
    Gpu {
        device: String,
        backend: String,
    },
}

impl RuntimeStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Checking => "Processing device has not been checked yet".into(),
            Self::Cpu { reason } => format!("Using CPU — {reason}"),
            Self::Gpu { device, backend } => format!("Using {device} — {backend}"),
        }
    }

    pub fn is_gpu(&self) -> bool {
        matches!(self, Self::Gpu { .. })
    }

    pub fn validate(&self, mode: ProcessingMode) -> Result<(), String> {
        match self {
            Self::Gpu { .. } => Ok(()),
            Self::Cpu { reason } if mode == ProcessingMode::Gpu => Err(format!(
                "GPU processing is unavailable: {reason}. Select Auto or CPU to continue."
            )),
            Self::Cpu { .. } => Ok(()),
            Self::Checking => Err("Whisper did not confirm its processing device.".into()),
        }
    }
}

fn status_store() -> &'static Mutex<RuntimeStatus> {
    static STATUS: OnceLock<Mutex<RuntimeStatus>> = OnceLock::new();
    STATUS.get_or_init(|| Mutex::new(RuntimeStatus::Checking))
}

pub fn current_status() -> RuntimeStatus {
    status_store()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

pub fn set_status(status: RuntimeStatus) {
    *status_store()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = status;
}

/// Incremental parser used by the probe, live following and transcription.
pub struct RuntimeDiagnostics {
    mode: ProcessingMode,
    status: RuntimeStatus,
    devices: BTreeMap<String, String>,
    failure: Option<String>,
}

impl RuntimeDiagnostics {
    pub fn new(mode: ProcessingMode) -> Self {
        Self {
            mode,
            status: RuntimeStatus::Checking,
            devices: BTreeMap::new(),
            failure: None,
        }
    }

    pub fn observe(&mut self, line: &str) {
        let line = line.trim();
        if let Some(rest) = line.split("ggml_vulkan:").nth(1) {
            if let Some((index, name)) = rest.trim().split_once(" = ") {
                if index.chars().all(|character| character.is_ascii_digit()) && !index.is_empty() {
                    let name = name.split(" | ").next().unwrap_or(name).trim();
                    self.devices
                        .insert(format!("Vulkan{index}"), name.to_owned());
                }
            }
        }
        if let Some(rest) = line.split("Device ").nth(1) {
            if line.contains("ggml_cuda_init:") {
                if let Some((index, name)) = rest.split_once(": ") {
                    if index.chars().all(|character| character.is_ascii_digit())
                        && !index.is_empty()
                    {
                        let name = name
                            .split(", compute capability")
                            .next()
                            .unwrap_or(name)
                            .trim();
                        self.devices.insert(format!("CUDA{index}"), name.to_owned());
                    }
                }
            }
        }
        let lower = line.to_ascii_lowercase();
        if (lower.contains("failed") || lower.contains("error") || lower.contains("not found"))
            && ["vulkan", "cuda", "gpu", "backend"]
                .iter()
                .any(|word| lower.contains(word))
        {
            self.failure = Some(line.chars().take(240).collect());
        }
        if let Some(rest) = line.split("whisper_backend_init_gpu: using ").nth(1) {
            if let Some(backend) = rest.strip_suffix(" backend") {
                let device = self
                    .devices
                    .get(backend)
                    .cloned()
                    .unwrap_or_else(|| backend.to_owned());
                let lower_device = device.to_ascii_lowercase();
                if ["llvmpipe", "lavapipe", "swiftshader", "software"]
                    .iter()
                    .any(|name| lower_device.contains(name))
                {
                    self.status = RuntimeStatus::Cpu {
                        reason: format!("{device} is a software renderer"),
                    };
                } else if backend.eq_ignore_ascii_case("cpu") {
                    self.set_cpu();
                } else {
                    self.status = RuntimeStatus::Gpu {
                        device,
                        backend: backend
                            .trim_end_matches(|character: char| character.is_ascii_digit())
                            .to_owned(),
                    };
                }
            }
        } else if line.contains("whisper_backend_init_gpu: no GPU found")
            || (self.mode == ProcessingMode::Cpu && line.contains("whisper_model_load:") && line.contains("CPU total size"))
        {
            self.set_cpu();
        }
    }

    fn set_cpu(&mut self) {
        self.status = RuntimeStatus::Cpu {
            reason: if self.mode == ProcessingMode::Cpu {
                "selected in Processing".into()
            } else {
                self.failure
                    .clone()
                    .unwrap_or_else(|| "no compatible GPU backend or device is available".into())
            },
        };
    }

    pub fn status(&self) -> RuntimeStatus {
        self.status.clone()
    }

    pub fn validate(&self) -> Result<(), String> {
        self.status.validate(self.mode)
    }
}

/// Verify initialization and a real inference without opening the microphone.
/// Call from a background executor: cold GPU shader compilation may take time.
pub fn probe(
    executable: &Path,
    model: &Path,
    mode: ProcessingMode,
    progress: &JobProgress,
) -> Result<RuntimeStatus, String> {
    match probe_once(executable, model, mode, progress) {
        Err(gpu_error) if mode == ProcessingMode::Auto && !progress.is_cancelled() => {
            match probe_once(executable, model, ProcessingMode::Cpu, progress) {
                Ok(RuntimeStatus::Cpu { .. }) => Ok(RuntimeStatus::Cpu {
                    reason: format!(
                        "GPU check failed: {}",
                        gpu_error.chars().take(240).collect::<String>()
                    ),
                }),
                Ok(other) => Ok(other),
                Err(cpu_error) => Err(format!(
                    "{gpu_error}\nCPU fallback also failed: {cpu_error}"
                )),
            }
        }
        result => result,
    }
}

fn probe_once(
    executable: &Path,
    model: &Path,
    mode: ProcessingMode,
    progress: &JobProgress,
) -> Result<RuntimeStatus, String> {
    if progress.is_cancelled() {
        return Err("Speech setup was cancelled".into());
    }
    if !model.is_file() {
        return Err("Download the voice model to check the processing device".into());
    }
    let work = ProbeDirectory::new()
        .map_err(|error| format!("Could not prepare speech check: {error}"))?;
    let audio = work.0.join("silence.wav");
    std::fs::write(&audio, silent_wav())
        .map_err(|error| format!("Could not prepare speech check: {error}"))?;
    let mut child = command_with_mode(executable, mode)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&audio)
        // `-np` disables Whisper's backend diagnostics in upstream builds,
        // including the confirmation needed to enforce the selected mode.
        .args(["-l", "en", "-nf", "-nt", "-t", "4"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start Whisper: {error}"))?;
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader = std::thread::spawn(move || {
        let mut diagnostics = RuntimeDiagnostics::new(mode);
        let mut tail = Vec::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            diagnostics.observe(&line);
            if !line.trim().is_empty() {
                tail.push(line);
                if tail.len() > 12 {
                    tail.remove(0);
                }
            }
        }
        (diagnostics, tail.join("\n"))
    });
    let start = Instant::now();
    let result = loop {
        if progress.is_cancelled() || start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break Err(if progress.is_cancelled() {
                "Speech setup was cancelled".to_string()
            } else {
                "Whisper's processing check timed out after 60 seconds".to_string()
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("Could not check Whisper: {error}"));
            }
        }
    };
    let (diagnostics, messages) = reader
        .join()
        .map_err(|_| "Whisper's output reader stopped".to_string())?;
    let status = result?;
    if !status.success() {
        return Err(format!(
            "Whisper's processing check failed ({status}): {}",
            messages.trim()
        ));
    }
    diagnostics.validate()?;
    Ok(diagnostics.status())
}

fn silent_wav() -> Vec<u8> {
    const SAMPLES: u32 = 16_000;
    let size = SAMPLES * 2;
    let mut wav = Vec::with_capacity(44 + size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&32_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&size.to_le_bytes());
    wav.resize(44 + size as usize, 0);
    wav
}

struct ProbeDirectory(PathBuf);

impl ProbeDirectory {
    fn new() -> io::Result<Self> {
        let path =
            std::env::temp_dir().join(format!("lahza-speech-check-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_preferences_are_auto_and_explicit_choices_round_trip() {
        let work = ProbeDirectory::new().unwrap();
        let path = work.0.join("config/speech.json");
        assert_eq!(load_from(&path), ProcessingMode::Auto);
        for mode in ProcessingMode::ALL {
            save_to(&path, mode).unwrap();
            assert_eq!(load_from(&path), mode);
        }
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(load_from(&path), ProcessingMode::Auto);
    }

    #[test]
    fn gpu_discovery_and_requested_gpu_are_not_confirmation() {
        let mut diagnostics = RuntimeDiagnostics::new(ProcessingMode::Auto);
        for line in [
            "ggml_vulkan: 0 = NVIDIA GeForce RTX 3060 (NVIDIA) | uma: 0 | fp16: 1",
            "load_backend: loaded Vulkan backend from /tmp/libggml-vulkan.so",
            "whisper_init_with_params_no_state: use gpu    = 1",
        ] {
            diagnostics.observe(line);
        }
        assert_eq!(diagnostics.status(), RuntimeStatus::Checking);
        diagnostics.observe("whisper_backend_init_gpu: using Vulkan0 backend");
        assert_eq!(
            diagnostics.status(),
            RuntimeStatus::Gpu {
                device: "NVIDIA GeForce RTX 3060 (NVIDIA)".into(),
                backend: "Vulkan".into(),
            }
        );
        assert!(diagnostics.validate().is_ok());
    }

    #[test]
    fn cpu_fallback_is_allowed_only_when_not_explicit_gpu() {
        for mode in ProcessingMode::ALL {
            let mut diagnostics = RuntimeDiagnostics::new(mode);
            diagnostics.observe("whisper_init_with_params_no_state: use gpu    = 1");
            diagnostics.observe("whisper_model_load:          CPU total size =   147.37 MB");
            diagnostics.observe("whisper_backend_init_gpu: no GPU found");
            assert!(matches!(diagnostics.status(), RuntimeStatus::Cpu { .. }));
            assert_eq!(diagnostics.validate().is_err(), mode == ProcessingMode::Gpu);
        }
    }

    #[test]
    fn software_vulkan_is_reported_as_cpu() {
        let mut diagnostics = RuntimeDiagnostics::new(ProcessingMode::Gpu);
        diagnostics.observe("ggml_vulkan: 0 = llvmpipe (LLVM 20.1.8, 256 bits) | uma: 1");
        diagnostics.observe("whisper_backend_init_gpu: using Vulkan0 backend");
        assert!(matches!(diagnostics.status(), RuntimeStatus::Cpu { .. }));
        assert!(diagnostics.validate().is_err());
    }

    #[test]
    fn cpu_selection_disables_whisper_gpu() {
        let cpu = command_with_mode(Path::new("/usr/bin/whisper-cli"), ProcessingMode::Cpu);
        assert!(cpu.get_args().any(|argument| argument == "-ng"));
        let auto = command_with_mode(Path::new("/usr/bin/whisper-cli"), ProcessingMode::Auto);
        assert!(!auto.get_args().any(|argument| argument == "-ng"));
    }

    #[test]
    fn cuda_device_name_matches_selected_backend() {
        let mut diagnostics = RuntimeDiagnostics::new(ProcessingMode::Auto);
        diagnostics.observe(
            "ggml_cuda_init:  Device 0: NVIDIA RTX 3060, compute capability 8.6, VMM: yes",
        );
        diagnostics.observe(
            "ggml_cuda_init:  Device 1: NVIDIA RTX 4090, compute capability 8.9, VMM: yes",
        );
        diagnostics.observe("whisper_backend_init_gpu: using CUDA1 backend");
        assert_eq!(
            diagnostics.status(),
            RuntimeStatus::Gpu {
                device: "NVIDIA RTX 4090".into(),
                backend: "CUDA".into()
            }
        );
    }

    #[test]
    fn probes_keep_upstream_backend_diagnostics_enabled() {
        use std::os::unix::fs::PermissionsExt;
        let work = ProbeDirectory::new().unwrap();
        let executable = work.0.join("whisper-cli");
        let model = work.0.join("model.bin");
        std::fs::write(&model, []).unwrap();
        // Upstream Whisper's -np turns off its logging callback, including
        // backend initialization. A successful exit alone proves nothing.
        std::fs::write(&executable, r#"#!/bin/sh
cpu=0
for arg in "$@"; do
    if [ "$arg" = "-np" ]; then exit 0; fi
    if [ "$arg" = "-ng" ]; then cpu=1; fi
done
if [ "$cpu" = 1 ]; then
    printf 'whisper_model_load: CPU total size = 147.37 MB\n' >&2
else
    printf 'ggml_vulkan: 0 = Test GPU | uma: 0\nwhisper_backend_init_gpu: using Vulkan0 backend\n' >&2
fi
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        for mode in ProcessingMode::ALL {
            let status = probe(&executable, &model, mode, &JobProgress::default()).unwrap();
            assert_eq!(status.is_gpu(), mode != ProcessingMode::Cpu);
        }
    }
}
