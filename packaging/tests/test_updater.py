import hashlib
import importlib.machinery
import importlib.util
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


loader = importlib.machinery.SourceFileLoader("updater", str(Path(__file__).parents[1] / "lahza-update"))
spec = importlib.util.spec_from_loader(loader.name, loader)
updater = importlib.util.module_from_spec(spec)
loader.exec_module(updater)


def release(version="0.4.2"):
    return {
        "tag_name": f"v{version}",
        "assets": [
            {"name": name, "browser_download_url": f"{updater.REPOSITORY}/releases/download/v{version}/{name}"}
            for name in (f"lahza_{version}_amd64.deb", "SHA256SUMS")
        ],
    }


class UpdaterTests(unittest.TestCase):
    def test_unsupported_or_missing_installation_fails_without_network(self):
        for outputs in (["arm64"], ["amd64", "deinstall ok config-files\n0.4.1"]):
            with self.subTest(outputs=outputs), \
                    patch.object(updater.sys, "argv", ["lahza-update"]), \
                    patch.object(updater, "output", side_effect=outputs), \
                    patch.object(updater, "fetch") as fetch:
                self.assertEqual(updater.main(), 1)
                fetch.assert_not_called()

    def test_network_failure_does_not_install(self):
        with patch.object(updater.sys, "argv", ["lahza-update", "--install"]), \
                patch.object(updater, "output", side_effect=["amd64", "install ok installed\n0.4.1"]), \
                patch.object(updater, "warn_about_local_install"), \
                patch.object(updater, "fetch", side_effect=OSError("offline")), \
                patch.object(updater, "install_release") as apply:
            self.assertEqual(updater.main(), 1)
            apply.assert_not_called()

    def test_rejects_incomplete_or_untrusted_release(self):
        for change in ("missing", "foreign", "prerelease", "tag"):
            data = release()
            if change == "missing":
                data["assets"].pop()
            elif change == "foreign":
                data["assets"][0]["browser_download_url"] = "https://example.com/app.deb"
            elif change == "prerelease":
                data["prerelease"] = True
            else:
                data["tag_name"] = "v../../other"
            with self.subTest(change=change), self.assertRaises(ValueError):
                updater.release_assets(data, "amd64")

    def test_checksum_requires_exact_unique_filename(self):
        digest = "a" * 64
        self.assertEqual(updater.expected_checksum(f"{digest}  app.deb\n", "app.deb"), digest)
        for manifest in (f"{digest}  other.deb", "bad  app.deb", f"{digest}  app.deb\n{digest}  app.deb"):
            with self.assertRaises(ValueError):
                updater.expected_checksum(manifest, "app.deb")

    def test_install_verification_and_cleanup(self):
        version, name, assets = updater.release_assets(release(), "amd64")
        payload = b"test package"
        digest = hashlib.sha256(payload).hexdigest()
        for failure in (None, "checksum", "metadata", "apt"):
            manifest = f"{digest if failure != 'checksum' else '0' * 64}  {name}"
            fields = ["wrong" if failure == "metadata" else "lahza", version, "amd64"]
            paths = []

            def apt(command, check):
                package = Path(command[-1])
                paths.append(package)
                self.assertEqual(command[:3], ["sudo", "apt", "install"])
                self.assertEqual(package.read_bytes(), payload)
                self.assertEqual(package.parent.stat().st_mode & 0o777, 0o755)
                if failure == "apt":
                    raise subprocess.CalledProcessError(1, command)

            with self.subTest(failure=failure), \
                    patch.object(updater, "fetch", side_effect=[io.BytesIO(manifest.encode()), io.BytesIO(payload)]), \
                    patch.object(updater, "output", side_effect=fields), \
                    patch.object(updater.os, "geteuid", return_value=1000), \
                    patch.object(updater.subprocess, "run", side_effect=apt) as run:
                if failure:
                    with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                        updater.install_release(version, name, assets, "amd64")
                else:
                    updater.install_release(version, name, assets, "amd64")
                self.assertEqual(run.call_count, 0 if failure in ("checksum", "metadata") else 1)
                for path in paths:
                    self.assertFalse(path.exists())

    def test_only_newer_versions_install(self):
        # Use real Debian version comparison, with network and installation mocked.
        for installed, latest, install, expected in (
            ("0.4.1", "0.4.2", True, 1),
            ("0.4.2", "0.4.2", True, 0),
            ("0.4.3", "0.4.2", True, 0),
            ("0.4.9", "0.4.10", True, 1),
            ("0.4.1", "0.4.2", False, 0),
        ):
            with self.subTest(installed=installed, latest=latest, install=install), \
                    patch.object(updater.sys, "argv", ["lahza-update", "--install" if install else "--check"]), \
                    patch.object(updater, "output", side_effect=["amd64", f"install ok installed\n{installed}"]), \
                    patch.object(updater, "warn_about_local_install"), \
                    patch.object(updater, "fetch", return_value=io.BytesIO(updater.json.dumps(release(latest)).encode())), \
                    patch.object(updater, "install_release") as apply:
                self.assertEqual(updater.main(), 0)
                self.assertEqual(apply.call_count, expected)


if __name__ == "__main__":
    unittest.main()
