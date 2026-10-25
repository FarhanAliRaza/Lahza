#!/usr/bin/env python3
"""Launch Lahza in the background without hiding startup failures."""

import os
from pathlib import Path
import subprocess
import sys


def launch(binary, log_path, startup_timeout=1.5):
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("wb") as log:
        try:
            process = subprocess.Popen(
                [str(binary)], stdin=subprocess.DEVNULL, stdout=log,
                stderr=subprocess.STDOUT, start_new_session=True,
            )
        except OSError as error:
            message = f"Could not launch Lahza: {error}"
            log.write((message + "\n").encode())
            raise RuntimeError(message) from error
    try:
        status = process.wait(timeout=startup_timeout)
    except subprocess.TimeoutExpired:
        return process
    raise RuntimeError(f"Lahza exited during startup (exit status {status}).")


def log_tail(path):
    try:
        with path.open("rb") as log:
            log.seek(0, os.SEEK_END)
            log.seek(max(0, log.tell() - 8192))
            return "\n".join(log.read().decode(errors="replace").splitlines()[-30:])
    except OSError:
        return ""


def main():
    if len(sys.argv) != 2:
        print("Usage: run.py /path/to/lahza", file=sys.stderr)
        return 2
    state_home = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    log_path = state_home / "lahza" / "launch.log"
    try:
        process = launch(Path(sys.argv[1]), log_path)
    except (OSError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        details = log_tail(log_path)
        if details:
            print(details, file=sys.stderr)
        print(f"Startup log: {log_path}", file=sys.stderr)
        return 1
    print(f"Lahza started (PID {process.pid}). Log: {log_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
