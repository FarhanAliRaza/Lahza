# Snap package

The Snap configuration is `snap/snapcraft.yaml`. It builds an amd64, strictly
confined package on Ubuntu 24.04 (`core24`), taking its version from `Cargo.toml`.
The GNOME extension supplies desktop integration and the graphics runtime.
FFmpeg, GStreamer tools/plugins, PipeWire modules, and Lahza assets are bundled.

## Build

With Snapcraft and its LXD build provider configured, build from a clean source
copy to avoid copying Cargo's large local target directory:

```bash
build_dir="$(mktemp -d /tmp/lahza-snap.XXXXXX)"
rsync -a --exclude=/.git --exclude=/.agents --exclude=/.codex \
  --exclude=/target --exclude=/parts --exclude=/stage --exclude=/prime \
  --exclude=/dist --exclude='/*.snap' ./ "$build_dir/"
(cd "$build_dir" && snapcraft pack --use-lxd)
mkdir -p dist
cp "$build_dir"/lahza_*.snap dist/
```

The recipe pins Rust 1.96.0. Snapcraft's managed build environment installs the
compiler and the GNOME SDK automatically.

Docker builds need extra preparation: Canonical's `8_core24` image currently
lacks the desktop command-chain files, Rust, and the GNOME SDK. Those resources,
plus the base and content-provider snaps, must be supplied explicitly. Use the
managed LXD route above for a fresh build environment.

## Local test

```bash
sudo snap install --dangerous ./dist/lahza_*.snap
snap connections lahza
snap run lahza --version
snap run lahza
```

`--dangerous` permits a local package without a Store signature; confinement
remains strict. Use `snap run lahza` explicitly when a Debian or source install
also exists.

Run the synthetic recording/export smoke test inside the installed Snap:

```bash
snap run --shell lahza -c 'exec "$SNAP/snap/command-chain/gpu-2404-wrapper" "$SNAP/snap/command-chain/desktop-launch" bash -s' \
  < packaging/snap-smoke-test.sh
```

This checks the bundled tools, plugins, PipeWire modules, VP8/Opus recording,
H.264/AAC export, and frame decoding without recording any real devices.

Microphone/system-audio capture and webcam access need their interfaces connected:

```bash
sudo snap connect lahza:audio-record
sudo snap connect lahza:camera
# Optional, for files on removable drives:
sudo snap connect lahza:removable-media
```

Check these operations on a real desktop before releasing:

1. Launch from both the terminal and the desktop application menu.
2. Capture a screenshot through the desktop source picker.
3. Import an image, apply Tutorial steps, and export PNG and MP4.
4. Record the screen, pause/resume, and stop; verify the resulting video.
5. Record microphone/system audio and a webcam; verify playback and export.
6. Save and reopen a project under the user's home directory.

Run the checks on Wayland and X11. Optional GNOME Shell input tracking needs
separate validation; portal screen capture does not prove that integration works.
Host unit tests alone do not verify Snap confinement or desktop permissions.

## GitHub Actions releases

The `Build Linux Release` workflow builds Debian packages and the Snap in
separate jobs on pushes to `main`, pull requests, and manual workflow runs.
The Snap job installs the built package under strict confinement and runs the
synthetic recording/export smoke test before it can be published. Artifacts
include the Snap, `SNAP-SHA256SUMS`, and the smoke-test log.

Pushing a version tag publishes after **both package jobs pass**:

- `v0.4.3` must match version `0.4.3` in `Cargo.toml` and publishes to `stable`.
- `v0.4.3-rc.1` must match version `0.4.3-rc.1` and publishes to `beta`.
- Branch pushes, pull requests, and manual runs only build and test.

The tested Snap and its checksum are also attached to the GitHub release
created by the Debian job. Publishing uses the same artifact that passed the
Snap test; it does not rebuild with Store credentials present.

### One-time Store authorization

Run this locally, with Snapcraft and GitHub CLI installed and authenticated:

```bash
bash packaging/configure-snap-publishing.sh
```

Complete Snapcraft's interactive login when prompted. The script exports a
credential restricted to `lahza` and the `stable,beta` channels, uploads it as
the `SNAPCRAFT_STORE_CREDENTIALS` Actions secret in `FarhanAliRaza/lahza`, and
removes the temporary credential file. It expires after 365 days; rerun the
script to renew it. Never commit the credential or paste it into a workflow.

The publishing job fails with an explicit setup message if the secret is
missing. Fix or renew the secret and rerun the failed job if authentication
fails. Store review can delay or reject publication even after CI tests pass;
the upload action reports that result in the job log.

### Cut a release

After merging the workflow and completing authorization, update `Cargo.toml`
and `Cargo.lock`, commit the release changes, and push a matching tag:

```bash
# Example for the next stable release, after updating both version files:
git push origin main
git tag v0.4.3
git push origin v0.4.3
```

No manual Snap build or Store upload is needed for subsequent releases.
CI checks use synthetic media; real source-picker and device capture checks
still need a desktop when changing those integrations.

## Manual publishing

```bash
snapcraft upload --release=beta ./dist/lahza_*.snap
snapcraft revisions lahza
snapcraft release lahza <tested-revision> stable
```

Snap updates are managed by snapd. The Debian-specific `lahza-update` helper is
not included in the Snap.
