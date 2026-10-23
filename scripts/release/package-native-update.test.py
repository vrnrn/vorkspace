"""Packaging contracts, with an opt-in real Sparkle signature roundtrip."""
import base64
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import tarfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

SCRIPT = Path(__file__).with_name("package-native-update.py")
spec = importlib.util.spec_from_file_location("package_update", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="vorkspace-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / "vorkspace.app"
        (self.bundle / "Contents").mkdir(parents=True)
        self.info = dict(CFBundleVersion="1.0.0", CFBundleShortVersionString="1.0.0",
                         CFBundleIdentifier="com.vorkspace.package-test",
                         CFBundleExecutable="vorkspace", CFBundlePackageType="APPL",
                         VorkspaceDataMode="vorkspace-data", SUFeedURL="https://example.test/appcast.xml",
                         SUPublicEDKey=base64.b64encode(bytes(32)).decode())
        self.write_info()
        self.key = self.root / "private.key"
        self.key.write_text("ephemeral test key")
        self.key.chmod(0o600)
        self.argv = [str(SCRIPT), str(self.bundle), str(self.root / "updates"),
                     "--download-url-prefix", "https://example.test/downloads",
                     "--ed-key-file", str(self.key)]

    def write_info(self):
        (self.bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(self.info))

    def run_tool(self, *extra):
        with patch.object(sys, "argv", self.argv + list(extra)), contextlib.redirect_stdout(io.StringIO()):
            module.main()

    def fake_packaging(self, *command):
        if str(command[0]).endswith("bin/generate_appcast"):
            output = self.root / "updates"
            archive = output / "vorkspace-native-1.0.0.zip"
            archive.write_bytes(b"test update")
            (output / "appcast.xml").write_text('''<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>
<enclosure url="https://example.test/vorkspace-native-1.0.0.zip" length="11" sparkle:edSignature="test signature"/>
</item></channel></rss>''')

    def test_adhoc_package_needs_only_sparkle_signature(self):
        with patch.object(module, "run", side_effect=self.fake_packaging) as run:
            self.run_tool()
        commands = [tuple(str(arg) for arg in call.args) for call in run.call_args_list]
        self.assertEqual(commands[0][:4], ("codesign", "--verify", "--deep", "--strict"))
        self.assertFalse(any(command[0] in ("xcrun", "spctl") for command in commands))
        self.assertIn(("--ed-key-file", str(self.key)), tuple(zip(commands[-1], commands[-1][1:])))
        self.assertNotIn(self.key.read_text(), " ".join(" ".join(command) for command in commands))

    def test_notarization_is_explicit(self):
        with patch.object(module, "run", side_effect=self.fake_packaging) as run:
            self.run_tool("--notary-profile", "explicit-profile")
        commands = [tuple(str(arg) for arg in call.args) for call in run.call_args_list]
        self.assertIn(("codesign", "--verify", "-R=anchor apple generic", str(self.bundle)), commands)
        self.assertTrue(any(command[:3] == ("xcrun", "notarytool", "submit") for command in commands))

    def test_unsafe_key_permissions_fail_before_packaging(self):
        self.key.chmod(0o644)
        with patch.object(module, "run") as run, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                self.run_tool()
        run.assert_not_called()
        self.assertFalse((self.root / "updates").exists())

    def test_untrusted_bundle_version_cannot_escape_output(self):
        self.info["CFBundleVersion"] = "../../outside"
        self.write_info()
        with patch.object(module, "run") as run, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                self.run_tool()
        run.assert_not_called()

    def test_compatibility_keys_must_be_paired(self):
        with patch.object(module, "run") as run, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                self.run_tool("--compat-signing-key", str(self.key))
        run.assert_not_called()

    @unittest.skipUnless(sys.platform == "darwin" and os.environ.get("VORKSPACE_TEST_SPARKLE"),
                         "requires macOS and VORKSPACE_TEST_SPARKLE pointing at the pinned distribution")
    def test_real_adhoc_archive_has_verified_sparkle_signature(self):
        sparkle = Path(os.environ["VORKSPACE_TEST_SPARKLE"]).resolve()
        keygen = self.root / "keygen.swift"
        keygen.write_text('''import Foundation
import CryptoKit
let key = Curve25519.Signing.PrivateKey()
let path = CommandLine.arguments[1]
try key.rawRepresentation.base64EncodedData().write(to: URL(fileURLWithPath: path))
try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path)
print(key.publicKey.rawRepresentation.base64EncodedString())
''')
        self.info["SUPublicEDKey"] = subprocess.check_output(["swift", str(keygen), str(self.key)], text=True).strip()
        self.write_info()
        executable = self.bundle / "Contents/MacOS/vorkspace"
        executable.parent.mkdir()
        subprocess.run(["xcrun", "clang", "-x", "c", "-o", str(executable), "-"],
                       input="int main(void) { return 0; }\n", text=True, check=True, capture_output=True)
        subprocess.run(["codesign", "--force", "--sign", "-", str(self.bundle)], check=True, capture_output=True)
        compatibility = shutil.which("minisign")
        if compatibility:
            compat_private, compat_public = self.root / "desktop.key", self.root / "desktop.pub"
            subprocess.run([compatibility, "-G", "-W", "-p", str(compat_public), "-s", str(compat_private)], check=True, capture_output=True)
            self.argv += ["--compat-signing-key", str(compat_private), "--compat-public-key", str(compat_public)]
        self.run_tool("--sparkle", str(sparkle))
        output = self.root / "updates"
        archive = output / "vorkspace-native-1.0.0.zip"
        enclosure = ET.parse(output / "appcast.xml").find("./channel/item/enclosure")
        self.assertIsNotNone(enclosure)
        signature = enclosure.attrib["{http://www.andymatuschak.org/xml-namespaces/sparkle}edSignature"]
        self.assertEqual(int(enclosure.attrib["length"]), archive.stat().st_size)
        verify = [str(sparkle / "bin/sign_update"), "--verify", "--ed-key-file", str(self.key), str(archive), signature]
        subprocess.run(verify, check=True, capture_output=True)
        contents = bytearray(archive.read_bytes())
        contents[len(contents) // 2] ^= 1
        archive.write_bytes(contents)
        rejected = subprocess.run(verify, capture_output=True)
        self.assertNotEqual(rejected.returncode, 0, "Modified update archive passed signature verification")

        if compatibility:
            feed = json.loads((output / "latest.json").read_text())
            self.assertEqual(feed["version"], "1.0.0")
            self.assertTrue(feed["platforms"])
            self.assertTrue(all(name.startswith("darwin-") for name in feed["platforms"]))
            entry = next(iter(feed["platforms"].values()))
            compat_archive = output / "vorkspace-1.0.0.app.tar.gz"
            self.assertEqual(entry["url"], "https://example.test/downloads/" + compat_archive.name)
            self.assertEqual(Path(str(compat_archive) + ".sig").read_text().strip(), entry["signature"])
            detached = self.root / "decoded.minisig"
            detached.write_bytes(base64.b64decode(entry["signature"], validate=True))
            compat_verify = [compatibility, "-V", "-p", str(compat_public), "-x", str(detached), "-m", str(compat_archive)]
            subprocess.run(compat_verify, check=True, capture_output=True)
            with tarfile.open(compat_archive) as extracted:
                members = extracted.getnames()
                self.assertIn("vorkspace.app/Contents/Info.plist", members)
                self.assertTrue(all(name == "vorkspace.app" or name.startswith("vorkspace.app/") for name in members))
            archive_bytes = bytearray(compat_archive.read_bytes())
            archive_bytes[len(archive_bytes) // 2] ^= 1
            compat_archive.write_bytes(archive_bytes)
            self.assertNotEqual(subprocess.run(compat_verify, capture_output=True).returncode, 0)

            # The existing desktop key wrapping is accepted without key migration.
            wrapped_private, wrapped_public = self.root / "wrapped.key", self.root / "wrapped.pub"
            wrapped_private.write_bytes(base64.b64encode(compat_private.read_bytes()))
            wrapped_private.chmod(0o600)
            wrapped_public.write_bytes(base64.b64encode(compat_public.read_bytes()))
            self.argv[2] = str(self.root / "wrapped-updates")
            self.argv[self.argv.index("--compat-signing-key") + 1] = str(wrapped_private)
            self.argv[self.argv.index("--compat-public-key") + 1] = str(wrapped_public)
            self.run_tool("--sparkle", str(sparkle))
            self.assertTrue((self.root / "wrapped-updates/latest.json").is_file())

            # A different valid public key must prevent publication of latest.json.
            wrong_private, wrong_public = self.root / "wrong-desktop.key", self.root / "wrong-desktop.pub"
            subprocess.run([compatibility, "-G", "-W", "-p", str(wrong_public), "-s", str(wrong_private)], check=True, capture_output=True)
            self.argv[2] = str(self.root / "wrong-compatibility-key-updates")
            self.argv[self.argv.index("--compat-public-key") + 1] = str(wrong_public)
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.run_tool("--sparkle", str(sparkle))
            self.assertFalse((self.root / "wrong-compatibility-key-updates/latest.json").exists())

        # Sparkle's generator only warns and returns success for the wrong key.
        # The packaging command must still reject its unsigned feed entry.
        subprocess.run(["swift", str(keygen), str(self.key)], check=True, capture_output=True)
        self.argv[2] = str(self.root / "wrong-key-updates")
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.run_tool("--sparkle", str(sparkle))


if __name__ == "__main__":
    unittest.main()
