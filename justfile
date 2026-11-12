bin := env_var('HOME') / ".local/bin/lahza"
apps := env_var('HOME') / ".local/share/applications"
icons := env_var('HOME') / ".local/share/icons/hicolor"
ngx := env_var('HOME') / ".local/lib/lahza"

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

# Install/enable the GNOME capture helper without rebuilding Lahza
install-helper:
    python3 packaging/install-gnome-helper.py

# Build the optional NVIDIA Ray Reconstruction shim (the fast path-traced mode) from a DLSS SDK
# checkout (github.com/NVIDIA/DLSS) and Vulkan headers, and install it with the SDK's snippet
ngx sdk="ignore/DLSS" headers="ignore/Vulkan-Headers/include":
    mkdir -p {{ngx}}
    g++ -std=c++17 -O2 -fPIC -shared -I{{headers}} -I{{sdk}}/include packaging/ngx/lahza_ngx.cpp \
      -o {{ngx}}/liblahza_ngx.so {{sdk}}/lib/Linux_x86_64/libnvsdk_ngx.a -l:libvulkan.so.1 -ldl -lpthread
    cp {{sdk}}/lib/Linux_x86_64/rel/libnvidia-ngx-dlssd.so.* {{ngx}}/
    @echo "Installed the Ray Reconstruction shim in {{ngx}}"

# Build, replace the installed binary, and install the GNOME capture helper
install: build
    target/release/lahza --version
    pkill -x lahza || true
    install -Dm755 target/release/lahza {{bin}}
    @echo "Installed $(stat -c %y {{bin}} | cut -d. -f1) -> {{bin}}"
    python3 packaging/install-gnome-helper.py

# Full desktop install: binary, desktop entry, and icon
install-desktop: install
    install -Dm644 packaging/com.lahza.Lahza.desktop \
      {{apps}}/com.lahza.Lahza.desktop
    install -Dm644 assets/Lahza.png \
      {{icons}}/512x512/apps/com.lahza.Lahza.png
    update-desktop-database {{apps}}
    gtk-update-icon-cache -t {{icons}}

# Build, replace the binary, and launch the app
run: install
    python3 packaging/run.py '{{bin}}'

# Remove the installed binary, desktop entry, and icon
uninstall:
    pkill -x lahza || true
    rm -f {{bin}} {{apps}}/com.lahza.Lahza.desktop \
      {{icons}}/512x512/apps/com.lahza.Lahza.png
    update-desktop-database {{apps}}

# Run the test suite
test:
    cargo test --release
