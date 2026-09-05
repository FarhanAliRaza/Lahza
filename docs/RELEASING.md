# Publishing a release

The Linux package workflow builds on Ubuntu 24.04 for amd64. Main-branch pushes produce downloadable Actions artifacts; pushing a `v*` tag publishes a GitHub Release with the Debian package, binary bundle with a user-local installer, and SHA-256 checksums attached.

Before the first release:

1. Run `cargo check --locked` and `cargo test --release --locked` with the documented native dependencies and FFmpeg installed.
2. Install the Actions `.deb` on a clean Ubuntu 24.04 desktop. Verify launch, shortcut registration, screenshot capture and PNG export.
3. Record with system audio and microphone, pause/resume, stop, reopen the project, trim a clip, add motion and a timed annotation, and export MP4, WebM, and GIF. Check audio synchronization and pointer alignment.
4. Animate a screenshot, export an image sequence, and test project draft recovery. Check the optional GNOME helper separately.
5. Confirm the version in `Cargo.toml` and `Cargo.lock`, document any known issues, and inspect the package's installed files and dependencies.

Once checks pass, publish the matching version tag:

```bash
git tag -a v0.1.0 -m "Lahza v0.1.0"
git push origin v0.1.0
```

This starts `.github/workflows/linux-deb.yml` and creates a public release automatically. The Debian package version comes from `Cargo.toml`, so it must match the tag. Review the generated release notes and attached package after the workflow succeeds. Do not describe untested platforms as supported.
