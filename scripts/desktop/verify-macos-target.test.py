import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "target", pathlib.Path(__file__).with_name("verify-macos-target.py")
)
target = importlib.util.module_from_spec(spec)
spec.loader.exec_module(target)


class DeploymentTarget(unittest.TestCase):
    def test_accepts_every_macos_26_slice(self):
        target.verify_load_commands(
            "app (architecture arm64):\nminos 26.0\n"
            "app (architecture x86_64):\nminos 26.0.0\n", "universal"
        )

    def test_rejects_missing_older_or_newer_targets(self):
        for commands in (
            "", "minos 13.0", "minos 26.1", "minos 26.0\nminos 15.0",
            "cmd LC_VERSION_MIN_MACOSX\ncmdsize 16\nversion 11.0\nsdk 26.0",
            "app (architecture arm64):\nminos 26.0\napp (architecture x86_64):\n",
        ):
            with self.subTest(commands=commands), self.assertRaises(ValueError):
                target.verify_load_commands(commands, "app")


if __name__ == "__main__":
    unittest.main()
