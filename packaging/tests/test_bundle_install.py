import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class BundleInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.bundle = root / "bundle"
        self.prefix = root / "installed"
        self.bundle.mkdir()
        shutil.copyfile(Path(__file__).resolve().parents[1] / "install.sh", self.bundle / "install.sh")
        for name in (
            "bin/lahza", "share/lahza/assets/icons/test.svg",
            "share/applications/com.lahza.Lahza.desktop",
            "share/icons/hicolor/512x512/apps/com.lahza.Lahza.png",
            "lib/lahza/whisper/bin/whisper-stream",
            "lib/lahza/whisper/lib/libggml.so.0.9.5",
        ):
            path = self.bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture")
            if "/bin/" in str(path):
                path.chmod(0o755)
        (self.bundle / "lib/lahza/whisper/lib/libggml.so.0").symlink_to("libggml.so.0.9.5")

    def install(self):
        return subprocess.run(
            ["bash", str(self.bundle / "install.sh")],
            env={**os.environ, "LAHZA_PREFIX": str(self.prefix)},
            capture_output=True, text=True,
        )

    def test_runtime_is_installed_with_executable_and_relocatable_library_links(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        runtime = self.prefix / "lib/lahza/whisper"
        self.assertTrue(os.access(runtime / "bin/whisper-stream", os.X_OK))
        library = runtime / "lib/libggml.so.0"
        self.assertTrue(library.is_symlink())
        self.assertEqual(library.readlink(), Path("libggml.so.0.9.5"))
        self.assertEqual(library.read_text(), "fixture")

    def test_missing_runtime_rejects_bundle_before_installation(self):
        (self.bundle / "lib/lahza/whisper/bin/whisper-stream").unlink()
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.prefix.exists())


if __name__ == "__main__":
    unittest.main()
