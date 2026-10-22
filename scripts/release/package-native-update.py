"""Notarize a signed native app and prepare its Sparkle feed, without publishing."""
import argparse
from pathlib import Path
import plistlib
import subprocess
import tempfile
from urllib.parse import urlparse


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--notary-profile", required=True, help="Existing notarytool Keychain profile")
    parser.add_argument("--download-url-prefix", required=True, help="HTTPS directory where archives will be published")
    parser.add_argument("--sparkle", type=Path, default=Path("target/sparkle"))
    args = parser.parse_args()
    parsed = urlparse(args.download_url_prefix)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        parser.error("The download directory must be an HTTPS URL without credentials, query or fragment")
    info = plistlib.loads((args.bundle / "Contents/Info.plist").read_bytes())
    if info.get("VorkspaceDataMode") != "vorkspace-data" or not info.get("SUPublicEDKey") or not info.get("SUFeedURL"):
        parser.error("Bundle must have explicit release update configuration")
    run("codesign", "--verify", "--deep", "--strict", args.bundle)
    # Refuse ad-hoc signatures before submitting anything to Apple.
    run("codesign", "--verify", "-R=anchor apple generic", args.bundle)
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / f"vorkspace-native-{info['CFBundleVersion']}.zip"
    if archive.exists():
        parser.error(f"Refusing to replace existing update: {archive}")
    with tempfile.TemporaryDirectory(prefix="native-notarize-") as tmp:
        submission = Path(tmp) / "submission.zip"
        run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", args.bundle, submission)
        run("xcrun", "notarytool", "submit", submission, "--keychain-profile", args.notary_profile, "--wait")
        run("xcrun", "stapler", "staple", args.bundle)
        run("xcrun", "stapler", "validate", args.bundle)
        run("spctl", "--assess", "--type", "execute", args.bundle)
        run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", args.bundle, archive)
    # generate_appcast reads the Ed25519 key from Keychain and verifies that its
    # public key matches the bundled key. Never pass private keys in argv.
    run(args.sparkle / "bin/generate_appcast", "--download-url-prefix", args.download_url_prefix.rstrip("/") + "/", args.output)
    print(f"Prepared {archive} and its appcast. Nothing was uploaded.")


if __name__ == "__main__":
    main()
