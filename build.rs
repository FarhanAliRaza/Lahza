#[cfg(target_os = "linux")]
fn main() {
    use std::{env, fs, path::Path};

    println!("cargo:rerun-if-changed=build.rs");

    // GPUI 0.2.2 enables xkbcommon's X11 bindings alongside Wayland and the
    // crate therefore asks the linker for `libxkbcommon-x11.so`. Some desktop
    // installations ship the runtime soname but not the unversioned developer
    // symlink. Keep local development working without modifying the system.
    let candidates = [
        "/usr/lib/x86_64-linux-gnu/libxkbcommon-x11.so.0",
        "/usr/lib64/libxkbcommon-x11.so.0",
        "/usr/lib/libxkbcommon-x11.so.0",
    ];

    if let Some(runtime_library) = candidates.iter().map(Path::new).find(|path| path.exists()) {
        let link_dir = env::var_os("OUT_DIR").expect("Cargo did not set OUT_DIR");
        let link_dir = Path::new(&link_dir).join("native-libs");
        fs::create_dir_all(&link_dir).expect("failed to create private native library directory");
        let alias = link_dir.join("libxkbcommon-x11.so");
        if !alias.exists() {
            std::os::unix::fs::symlink(runtime_library, &alias)
                .expect("failed to alias the installed xkbcommon-x11 runtime library");
        }
        println!("cargo:rustc-link-search=native={}", link_dir.display());
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {}
