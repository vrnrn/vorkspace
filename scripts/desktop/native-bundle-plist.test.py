import base64
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("bundle", Path(__file__).with_name("native-bundle-plist.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class BundleTests(unittest.TestCase):
    def setUp(self):
        self.plist = dict(CFBundleShortVersionString="0.13.8", CFBundleVersion="0.13.8", VorkspaceDataMode="vorkspace-data")
    def test_explicit_signed_feed_and_build_number(self):
        with tempfile.TemporaryDirectory() as framework:
            env = dict(VORKSPACE_UPDATE_FEED="https://example.com/native.xml", VORKSPACE_UPDATE_PUBLIC_KEY=base64.b64encode(bytes(32)).decode(), VORKSPACE_SPARKLE_FRAMEWORK=framework, VORKSPACE_BUILD_NUMBER="42.1")
            result = module.configure(self.plist, env)
            self.assertEqual(result["CFBundleVersion"], "42.1")
            self.assertTrue(result["SUEnableAutomaticChecks"])
            for key in ("VORKSPACE_UPDATE_FEED", "VORKSPACE_UPDATE_PUBLIC_KEY", "VORKSPACE_SPARKLE_FRAMEWORK"):
                invalid = dict(env); del invalid[key]
                with self.assertRaises(ValueError): module.configure(self.plist, invalid)
            for feed in ("http://example.com/feed", "https://user:secret@example.com/feed", "https:///missing"):
                with self.assertRaises(ValueError): module.configure(self.plist, dict(env, VORKSPACE_UPDATE_FEED=feed))
            with self.assertRaises(ValueError): module.configure(dict(self.plist, VorkspaceDataMode="dev-data"), env)
    def test_development_has_no_update_feed(self):
        result = module.configure(self.plist, {})
        self.assertNotIn("SUFeedURL", result)
        with self.assertRaises(ValueError): module.configure(self.plist, {"VORKSPACE_BUILD_NUMBER":"<xml>"})

if __name__ == "__main__": unittest.main()
