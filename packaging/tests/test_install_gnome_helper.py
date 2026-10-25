import ast
import importlib.util
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "install_gnome_helper", Path(__file__).resolve().parents[1] / "install-gnome-helper.py"
)
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class HelperInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.source.mkdir()
        self.uuid = "lahza-input@com.lahza"
        (self.source / "metadata.json").write_text(json.dumps({"uuid": self.uuid, "version": 2}))
        (self.source / "extension.js").write_text("import './prompter.js';")
        (self.source / "prompter.js").write_text("// capture exclusion")
        self.data = self.root / "data"
        self.settings = {
            "enabled-extensions": ["first@example", "second@example"],
            "disabled-extensions": ["disabled@example", self.uuid],
            "disable-user-extensions": False,
        }
        self.commands = []
        self.active = False

    def run_command(self, *args, check=True):
        self.commands.append(args)
        if args[:2] == ("gsettings", "get"):
            value = self.settings[args[3]]
            output = str(value).lower() if isinstance(value, bool) else repr(value)
            return subprocess.CompletedProcess(args, 0, output, "")
        if args[:2] == ("gsettings", "set"):
            self.settings[args[3]] = ast.literal_eval(args[4])
        if args[:2] == ("gnome-extensions", "info"):
            return subprocess.CompletedProcess(
                args, 0 if self.active else 2,
                "State: ACTIVE\n" if self.active else "", "not discovered"
            )
        return subprocess.CompletedProcess(args, 0, "", "")

    def install(self):
        return helper.install_helper(self.source, self.data, self.run_command)

    def copy_installed(self):
        destination = self.data / "gnome-shell/extensions" / self.uuid
        shutil.copytree(self.source, destination)

    def test_new_install_packs_capture_module_and_preserves_other_extensions(self):
        message = self.install()
        pack = next(command for command in self.commands if command[:2] == ("gnome-extensions", "pack"))
        self.assertIn("--extra-source=prompter.js", pack)
        self.assertEqual(self.settings["enabled-extensions"], ["first@example", "second@example", self.uuid])
        self.assertEqual(self.settings["disabled-extensions"], ["disabled@example"])
        self.assertFalse(self.settings["disable-user-extensions"])
        self.assertIn("Log out of GNOME and back in", message)

    def test_unchanged_active_install_does_not_reinstall_or_duplicate_settings(self):
        self.copy_installed()
        self.active = True
        self.settings["enabled-extensions"].append(self.uuid)
        self.settings["disabled-extensions"].remove(self.uuid)
        message = self.install()
        self.assertFalse(any(command[:2] == ("gnome-extensions", "pack") for command in self.commands))
        self.assertFalse(any(command[:2] == ("gsettings", "set") for command in self.commands))
        self.assertIn("is enabled", message)

    def test_changed_module_requires_login_even_if_old_extension_is_active(self):
        self.copy_installed()
        self.active = True
        (self.source / "prompter.js").write_text("// updated capture exclusion")
        self.assertIn("Log out of GNOME and back in", self.install())
        self.assertTrue(any(command[:2] == ("gnome-extensions", "install") for command in self.commands))

    def test_global_disable_is_preserved(self):
        self.settings["disable-user-extensions"] = True
        message = self.install()
        self.assertTrue(self.settings["disable-user-extensions"])
        self.assertFalse(any(command[:4] == ("gsettings", "set", "org.gnome.shell", "disable-user-extensions") for command in self.commands))
        self.assertFalse(any(command[:2] == ("gnome-extensions", "enable") for command in self.commands))
        self.assertIn("globally disabled", message)

    def test_invalid_settings_do_not_overwrite_preferences(self):
        self.settings["disabled-extensions"] = "invalid response"
        with self.assertRaisesRegex(RuntimeError, "disabled-extensions"):
            self.install()
        self.assertFalse(any(command[:2] == ("gsettings", "set") for command in self.commands))

    def test_non_gnome_session_does_not_install_or_change_settings(self):
        with patch.dict(helper.os.environ, {"XDG_CURRENT_DESKTOP": "KDE"}), patch.object(helper, "install_helper") as install:
            self.assertEqual(helper.main(), 0)
            install.assert_not_called()


if __name__ == "__main__":
    unittest.main()
