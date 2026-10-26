//! Bundled CPU inference runtime. The standalone executable contains all
//! runtime bytes and materializes a verified, atomic cache entry locally.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_x86_64 {
    use sha2::{Digest, Sha256};
    use std::{
        io::{self, Write},
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicU64, Ordering},
            Mutex, OnceLock,
        },
    };
    const LIBRARY: &[u8] =
        include_bytes!("../../assets/matting/runtime/linux-x86_64/libonnxruntime.so.1.31.0");
    const SHA256: &str = "469849977cb60744ea160927e1a3a0643fd4c55c0aff743251c6d293402e334e";
    #[used]
    static LICENSE: &str = include_str!("../../assets/matting/runtime/LICENSE");
    #[used]
    static NOTICES: &str = include_str!("../../assets/matting/runtime/ThirdPartyNotices.txt");
    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    // ort 2.0 rc15 permits only one environment per process. Keep a strong
    // reference for the process lifetime; sessions receive cheap Arc-backed
    // clones, while each model keeps its own execution options and cancellation.
    static ENVIRONMENT: OnceLock<ort::environment::Environment> = OnceLock::new();
    static ENVIRONMENT_INIT: Mutex<()> = Mutex::new(());

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn materialize(directory: &Path, bytes: &[u8], expected: &str) -> io::Result<PathBuf> {
        let target = directory.join("libonnxruntime.so.1.31.0");
        if std::fs::read(&target).is_ok_and(|cached| digest(&cached) == expected) {
            return Ok(target);
        }
        if digest(bytes) != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Bundled camera runtime checksum mismatch",
            ));
        }
        std::fs::create_dir_all(directory)?;
        let temporary = directory.join(format!(
            ".runtime-{}-{}.partial",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &target)?;
            Ok(target)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }

    pub(crate) fn load(model: &[u8]) -> Result<ort::session::Session, String> {
        load_with_options(model, false)
    }

    /// Restoration uses large attention workspaces with variable frame counts.
    /// Releasing them after each run prevents its cached session from retaining
    /// a multi-gigabyte CPU arena while the editor is idle.
    pub(crate) fn load_audio(model: &[u8]) -> Result<ort::session::Session, String> {
        load_with_options(model, true)
    }

    fn environment() -> Result<ort::environment::Environment, String> {
        if let Some(environment) = ENVIRONMENT.get() {
            return Ok(environment.clone());
        }
        // Initialization includes fallible filesystem and dynamic-library work.
        // Cache only success so fixing a cache-directory/library error permits
        // a later retry. Serialize the build and recheck after taking the lock
        // so concurrently starting camera/audio models cannot build two envs.
        let _initializing = ENVIRONMENT_INIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(environment) = ENVIRONMENT.get() {
            return Ok(environment.clone());
        }
        let initialized = create_environment()?;
        Ok(ENVIRONMENT.get_or_init(|| initialized).clone())
    }

    fn create_environment() -> Result<ort::environment::Environment, String> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .ok_or_else(|| "No user cache directory for camera runtime".to_string())?;
        let directory = cache
            .join("lahza/runtimes/onnxruntime-1.31.0-linux-x86_64")
            .join(SHA256);
        let library =
            materialize(&directory, LIBRARY, SHA256).map_err(|error| error.to_string())?;
        // Keep the required third-party texts beside the cached library too.
        for (name, text) in [("LICENSE", LICENSE), ("ThirdPartyNotices.txt", NOTICES)] {
            if !directory.join(name).is_file() {
                std::fs::write(directory.join(name), text).map_err(|error| error.to_string())?;
            }
        }
        ort::init_from(library)
            .map_err(|error| error.to_string())?
            .with_telemetry(false)
            .build()
            .map_err(|error| error.to_string())
    }

    fn load_with_options(model: &[u8], bounded_audio: bool) -> Result<ort::session::Session, String> {
        let environment = environment()?;
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(if bounded_audio { 2 } else { 4 });
        let builder = ort::session::Session::builder(&environment)
            .map_err(|error| error.to_string())?
            .with_intra_threads(threads)
            .map_err(|error| error.to_string())?;
        let mut builder = if bounded_audio {
            builder
                .with_memory_pattern(false)
                .map_err(|error| error.to_string())?
                .with_execution_providers([ort::ep::CPU::default()
                    .with_arena_allocator(false)
                    .build()])
                .map_err(|error| error.to_string())?
        } else {
            builder
        };
        builder
            .commit_from_memory(model)
            .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn concurrent_model_loaders_share_a_single_environment() {
            use ort::AsPointer;
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    std::thread::spawn(|| {
                        let environment = environment().unwrap();
                        // Creating independent session options must not try to
                        // rebuild or transfer ownership of the global env.
                        let _options = ort::session::Session::builder(&environment).unwrap();
                        environment.ptr() as usize
                    })
                })
                .collect();
            let identities: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect();
            assert_ne!(identities[0], 0);
            assert!(identities.iter().all(|ptr| *ptr == identities[0]));
            // All caller handles have been dropped, but the process-level
            // reference still owns the same environment for later model loads.
            assert_eq!(environment().unwrap().ptr() as usize, identities[0]);
        }
        #[test]
        fn cached_runtime_is_verified_and_corruption_is_repaired_atomically() {
            let directory = std::env::temp_dir().join(format!(
                "lahza-runtime-cache-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let bytes = b"synthetic runtime fixture";
            let hash = digest(bytes);
            let path = materialize(&directory, bytes, &hash).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            std::fs::write(&path, b"damaged cache").unwrap();
            assert_eq!(materialize(&directory, bytes, &hash).unwrap(), path);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
            assert!(materialize(&directory, b"invalid embedded fixture", "incorrect").is_err());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(super) use linux_x86_64::{load, load_audio};
