bin := env_var('HOME') / ".local/bin/lahza"
apps := env_var('HOME') / ".local/share/applications"
icons := env_var('HOME') / ".local/share/icons/hicolor"

# List recipes
default:
    @just --list

# Build the release binary
build:
    cargo build --release

# Bump the patch version (or pass a version), commit, and publish the release tag
[positional-arguments]
release *args:
    python3 packaging/release.py "$@"

# Validate a release without changing files, creating a tag, or pushing
[positional-arguments]
release-check *args:
    python3 packaging/release.py --dry-run "$@"

# Build, stop the running app, and replace the installed binary
install: build
    pkill -x lahza || true
    install -Dm755 target/release/lahza {{bin}}
    @echo "Installed $(stat -c %y {{bin}} | cut -d. -f1) -> {{bin}}"

# Full desktop install: binary, desktop entry, and icon
install-desktop: install
    install -Dm644 packaging/com.lahza.Lahza.desktop \
      {{apps}}/com.lahza.Lahza.desktop
    install -Dm644 Lahza.png \
      {{icons}}/512x512/apps/com.lahza.Lahza.png
    update-desktop-database {{apps}}
    gtk-update-icon-cache -t {{icons}}

# Build, replace the binary, and launch the app
run: install
    setsid {{bin}} >/dev/null 2>&1 &

# Remove the installed binary, desktop entry, and icon
uninstall:
    pkill -x lahza || true
    rm -f {{bin}} {{apps}}/com.lahza.Lahza.desktop \
      {{icons}}/512x512/apps/com.lahza.Lahza.png
    update-desktop-database {{apps}}

# Run the test suite
test:
    cargo test --release
