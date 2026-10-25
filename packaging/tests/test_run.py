import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "run_lahza", Path(__file__).resolve().parents[1] / "run.py"
)
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class LaunchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / "fake lahza"
        self.log = self.root / "state" / "launch.log"

    def script(self, source):
        self.binary.write_text("#!/usr/bin/env python3\n" + source)
        self.binary.chmod(0o755)

    def test_success_detaches_and_keeps_the_startup_log(self):
        self.script("import time\nprint('window ready', flush=True)\ntime.sleep(20)\n")
        process = launcher.launch(self.binary, self.log, startup_timeout=0.2)
        try:
            self.assertIsNone(process.poll())
            self.assertEqual(launcher.log_tail(self.log), "window ready")
        finally:
            process.terminate()
            process.wait(timeout=3)

    def test_immediate_failure_preserves_stderr_and_reports_status(self):
        self.script("import sys\nprint('cannot open display', file=sys.stderr)\nsys.exit(7)\n")
        with self.assertRaisesRegex(RuntimeError, "exit status 7"):
            launcher.launch(self.binary, self.log)
        self.assertEqual(launcher.log_tail(self.log), "cannot open display")

    def test_corrupt_executable_is_reported_without_shell_fallback(self):
        self.binary.write_bytes(b"\0" * 64)
        self.binary.chmod(0o755)
        with self.assertRaisesRegex(RuntimeError, "Could not launch Lahza"):
            launcher.launch(self.binary, self.log)
        self.assertIn("Exec format error", launcher.log_tail(self.log))

    def test_cli_returns_failure_and_prints_the_saved_error(self):
        self.script("import sys\nprint('startup panic', file=sys.stderr)\nsys.exit(101)\n")
        result = subprocess.run(
            [sys.executable, str(Path(launcher.__file__)), str(self.binary)],
            env={**os.environ, "XDG_STATE_HOME": str(self.root / "cli-state")},
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("startup panic", result.stderr)
        self.assertIn("exit status 101", result.stderr)
        self.assertIn("Startup log:", result.stderr)


if __name__ == "__main__":
    unittest.main()
