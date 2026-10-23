import base64
from contextlib import contextmanager
import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("bundle", Path(__file__).with_name("native-bundle-plist.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

@contextmanager
def framework_directory(minimum="10.13"):
    with tempfile.TemporaryDirectory() as directory:
        resources = Path(directory) / "Resources"
        resources.mkdir()
        (resources / "Info.plist").write_bytes(plistlib.dumps({"LSMinimumSystemVersion": minimum}))
        yield directory

class BundleTests(unittest.TestCase):
    def setUp(self):
        self.plist = dict(CFBundleShortVersionString="1.0.0", CFBundleVersion="1.0.0", VorkspaceDataMode="vorkspace-data")
    def test_explicit_signed_feed_and_build_number(self):
        with framework_directory() as framework:
            env = dict(VORKSPACE_UPDATE_FEED="https://example.com/native.xml", VORKSPACE_UPDATE_PUBLIC_KEY=base64.b64encode(bytes(32)).decode(), VORKSPACE_SPARKLE_FRAMEWORK=framework, VORKSPACE_BUILD_NUMBER="42.1")
            result = module.configure(self.plist, env)
            self.assertEqual(result["CFBundleVersion"], "42.1")
            self.assertTrue(result["SUEnableAutomaticChecks"])
            self.assertTrue(result["SUVerifyUpdateBeforeExtraction"])
            invalid = dict(env); del invalid["VORKSPACE_SPARKLE_FRAMEWORK"]
            with self.assertRaises(ValueError): module.configure(self.plist, invalid)
            for feed in ("http://example.com/feed", "https://user:secret@example.com/feed", "https:///missing"):
                with self.assertRaises(ValueError): module.configure(self.plist, dict(env, VORKSPACE_UPDATE_FEED=feed))
            with self.assertRaises(ValueError): module.configure(dict(self.plist, VorkspaceDataMode="dev-data"), env)
    def test_framework_enables_the_stable_release_feed_and_public_key(self):
        with framework_directory() as framework:
            result = module.configure(self.plist, {"VORKSPACE_SPARKLE_FRAMEWORK": framework})
            self.assertEqual(result["SUFeedURL"], module.RELEASE_FEED)
            self.assertEqual(result["SUPublicEDKey"], module.RELEASE_PUBLIC_KEY.read_text().strip())
            self.assertEqual(len(base64.b64decode(result["SUPublicEDKey"], validate=True)), 32)
            with self.assertRaises(ValueError):
                module.configure(dict(self.plist, VorkspaceDataMode="dev-data"), {"VORKSPACE_SPARKLE_FRAMEWORK": framework})
    def test_framework_cannot_raise_the_existing_users_minimum_os(self):
        with framework_directory("12.0") as framework:
            with self.assertRaises(ValueError):
                module.configure(self.plist, {"VORKSPACE_SPARKLE_FRAMEWORK": framework})
        with framework_directory("11.0") as framework:
            result = module.configure(self.plist, {"VORKSPACE_SPARKLE_FRAMEWORK": framework})
            self.assertEqual(result["LSMinimumSystemVersion"], "11.0")
    def test_development_has_no_update_feed(self):
        result = module.configure(self.plist, {})
        self.assertNotIn("SUFeedURL", result)
        with self.assertRaises(ValueError): module.configure(self.plist, {"VORKSPACE_BUILD_NUMBER":"<xml>"})

if __name__ == "__main__": unittest.main()
