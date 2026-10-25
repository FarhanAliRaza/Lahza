#!/usr/bin/env python3
"""Install the local capture helper without restarting the user's desktop."""

import ast
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def command(*args, check=True):
    result = subprocess.run(args, text=True, capture_output=True,
                            env={**os.environ, "LC_ALL": "C"})
    if check and result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip()
                           or f"{' '.join(args)} failed")
    return result


def setting_array(key, run):
    value = run("gsettings", "get", "org.gnome.shell", key).stdout.strip()
    if value.startswith("@as "):
        value = value[4:]
    try:
        values = ast.literal_eval(value)
    except (ValueError, SyntaxError) as error:
        raise RuntimeError(f"Could not read GNOME's {key} setting") from error
    if not isinstance(values, list) or not all(isinstance(item, str) for item in values):
        raise RuntimeError(f"Could not read GNOME's {key} setting")
    return values


def enable_at_next_login(uuid, run):
    # Read both lists before writing either, so a malformed response cannot
    # accidentally replace another extension's preferences.
    enabled = setting_array("enabled-extensions", run)
    disabled = setting_array("disabled-extensions", run)
    if uuid not in enabled:
        run("gsettings", "set", "org.gnome.shell", "enabled-extensions",
            repr([*enabled, uuid]))
    if uuid in disabled:
        run("gsettings", "set", "org.gnome.shell", "disabled-extensions",
            repr([item for item in disabled if item != uuid]))


def install_helper(source, data_home, run=command):
    metadata = json.loads((source / "metadata.json").read_text())
    uuid = metadata["uuid"]
    files = [source / "metadata.json", *sorted(source.glob("*.js"))]
    destination = data_home / "gnome-shell" / "extensions" / uuid
    changed = any(not (destination / path.name).is_file()
                  or (destination / path.name).read_bytes() != path.read_bytes()
                  for path in files)
    if changed:
        with tempfile.TemporaryDirectory(prefix="lahza-helper-") as temporary:
            # Pack every JS module; gnome-extensions otherwise includes only
            # extension.js and silently omits the capture-exclusion module.
            extras = [f"--extra-source={path.name}" for path in files
                      if path.suffix == ".js" and path.name != "extension.js"]
            run("gnome-extensions", "pack", "--force", "--out-dir", temporary,
                *extras, str(source))
            run("gnome-extensions", "install", "--force",
                str(Path(temporary) / f"{uuid}.shell-extension.zip"))

    enable_at_next_login(uuid, run)
    globally_disabled = run("gsettings", "get", "org.gnome.shell",
                            "disable-user-extensions").stdout.strip() == "true"
    if globally_disabled:
        return ("Lahza Capture Helper is installed. GNOME extensions are globally disabled; "
                "enable them in the Extensions app when you want to use the helper. "
                "Log out and back in after a first installation or update.")
    # New extensions are not discoverable in an existing Shell session.
    # Request activation if they are known, but verify the running state.
    run("gnome-extensions", "enable", uuid, check=False)
    info = run("gnome-extensions", "info", uuid, check=False)
    active = info.returncode == 0 and re.search(
        r"^\s*State:\s*(?:ACTIVE|ENABLED)\s*$", info.stdout, re.MULTILINE
    )
    if changed or not active:
        return ("Lahza Capture Helper is installed and configured to enable at login. "
                "Log out of GNOME and back in before screen/area recording with the teleprompter. "
                "Restarting Lahza alone does not reload GNOME extensions.")
    return ("Lahza Capture Helper is enabled. If you updated the helper during this "
            "login session, log out and back in to load its changed code.")


def main():
    desktops = os.environ.get("XDG_CURRENT_DESKTOP", "").casefold().split(":")
    if "gnome" not in desktops:
        print("Skipped GNOME capture helper: this is not a GNOME desktop session.")
        return 0
    if any(shutil.which(tool) is None for tool in ("gnome-extensions", "gsettings")):
        print("GNOME capture helper was not installed: gnome-extensions and gsettings "
              "are required. Run just install-helper from a GNOME session after installing them.")
        return 0
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    source = Path(__file__).resolve().parent / "gnome-shell-extension"
    try:
        print(install_helper(source, data_home))
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Could not install Lahza Capture Helper: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
