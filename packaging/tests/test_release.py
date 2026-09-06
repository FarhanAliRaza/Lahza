import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "release_script", Path(__file__).resolve().parents[1] / "release.py"
)
release_script = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_script)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        self.remote = base / "remote.git"
        self.root = base / "work"
        subprocess.run(["git", "init", "--bare", "-q", str(self.remote)], check=True)
        subprocess.run(["git", "init", "-q", "-b", "master", str(self.root)], check=True)
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "Release test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        (self.root / "Cargo.toml").write_text('[package]\nname = "lahza"\nversion = "0.4.3"\n')
        (self.root / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "dependency"\nversion = "0.4.3"\n\n'
            '[[package]]\nname = "lahza"\nversion = "0.4.3"\n'
        )
        self.git("add", ".")
        self.git("commit", "-qm", "Initial")
        self.git("remote", "add", "origin", str(self.remote))
        self.git("push", "-q", "-u", "origin", "master")
        self.initial = self.git("rev-parse", "HEAD")

    def git(self, *args):
        return release_script.git(self.root, *args)

    def test_patch_release_updates_only_app_and_pushes_matching_tag(self):
        release_script.release(self.root)
        self.assertIn('version = "0.4.4"', (self.root / "Cargo.toml").read_text())
        lock = (self.root / "Cargo.lock").read_text()
        self.assertIn('name = "dependency"\nversion = "0.4.3"', lock)
        self.assertIn('name = "lahza"\nversion = "0.4.4"', lock)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("rev-parse", "HEAD"), self.git("rev-parse", "v0.4.4^{}"))
        self.assertIn(self.git("rev-parse", "HEAD"), self.git("ls-remote", "origin", "refs/heads/master"))
        self.assertTrue(self.git("ls-remote", "origin", "refs/tags/v0.4.4"))

    def test_dry_run_does_not_change_files_commit_or_tags(self):
        release_script.release(self.root, "0.5.0-rc.1", dry_run=True)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.initial)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("tag", "--list"), "")

    def test_dirty_tree_rejected(self):
        (self.root / "uncommitted.txt").write_text("work")
        with self.assertRaisesRegex(ValueError, "working changes"):
            release_script.release(self.root)

    def test_wrong_branch_rejected(self):
        self.git("switch", "-qc", "feature")
        with self.assertRaisesRegex(ValueError, "Switch to master"):
            release_script.release(self.root)

    def test_old_version_and_existing_tag_rejected(self):
        with self.assertRaisesRegex(ValueError, "greater than"):
            release_script.release(self.root, "0.4.3")
        self.git("tag", "v0.4.4")
        with self.assertRaisesRegex(ValueError, "already exists"):
            release_script.release(self.root)

    def test_remote_ahead_rejected(self):
        self.git("commit", "--allow-empty", "-qm", "Remote update")
        self.git("push", "-q", "origin", "master")
        self.git("reset", "--hard", self.initial)
        with self.assertRaisesRegex(ValueError, "behind or diverged"):
            release_script.release(self.root)

    def test_rejected_tag_leaves_remote_branch_unchanged(self):
        hook = self.remote / "hooks" / "update"
        hook.write_text('#!/bin/sh\ncase "$1" in refs/tags/*) exit 1;; esac\n')
        hook.chmod(0o755)
        with self.assertRaises(subprocess.CalledProcessError):
            release_script.release(self.root)
        self.assertIn(self.initial, self.git("ls-remote", "origin", "refs/heads/master"))
        self.assertEqual(self.git("ls-remote", "origin", "refs/tags/v0.4.4"), "")
        self.assertEqual(self.git("rev-parse", "HEAD"), self.git("rev-parse", "v0.4.4^{}"))

    def test_semver_ordering_and_validation(self):
        versions = ["0.4.3", "0.4.4-alpha", "0.4.4-rc.2", "0.4.4-rc.10", "0.4.4"]
        self.assertEqual(sorted(versions, key=release_script.version_key), versions)
        for invalid in ("v0.4.4", "0.04.4", "0.4.4-rc.01", "0.4.4+build"):
            with self.assertRaises(ValueError):
                release_script.version_key(invalid)


if __name__ == "__main__":
    unittest.main()
