"""Prepare signed native updates and optional installed-client archives locally."""
import argparse
import base64
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from urllib.parse import unquote, urlparse
import xml.etree.ElementTree as ET


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def protected_file(path, parser, label):
    try:
        value = path.lstat()
    except OSError:
        parser.error(f"{label} file is unavailable")
    if not stat.S_ISREG(value.st_mode) or value.st_uid != os.getuid() or stat.S_IMODE(value.st_mode) & 0o077:
        parser.error(f"{label} must be a regular file owned by the current user with owner-only permissions")


@contextmanager
def minisign_file(path, parser, private=False):
    """Accept standard minisign text and the existing outer-base64 key format."""
    if private:
        protected_file(path, parser, "Compatibility signing key")
    contents = path.read_bytes().strip()
    if contents.startswith(b"untrusted comment:"):
        yield path
        return
    try:
        decoded = base64.b64decode(contents, validate=True)
    except ValueError:
        parser.error("Compatibility key must use minisign text or its outer-base64 encoding")
    if not decoded.startswith(b"untrusted comment:"):
        if not private and len(decoded) == 42:
            decoded = b"untrusted comment: minisign public key\n" + contents + b"\n"
        else:
            parser.error("Compatibility key does not contain standard minisign key text")
    with tempfile.TemporaryDirectory(prefix="vorkspace-update-key-") as temporary:
        normalized = Path(temporary) / "key"
        descriptor = os.open(normalized, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(decoded)
        yield normalized


def compatibility_update(args, info, parser):
    archive = args.output / f"vorkspace-{info['CFBundleVersion']}.app.tar.gz"
    signature = Path(str(archive) + ".sig")
    if archive.exists() or signature.exists():
        parser.error("Refusing to replace an existing compatibility archive or signature")
    architectures = subprocess.check_output(["lipo", "-archs", str(args.bundle / "Contents/MacOS" / info["CFBundleExecutable"])], text=True).split()
    names = {"arm64": "aarch64", "x86_64": "x86_64"}
    if not architectures or any(architecture not in names for architecture in architectures):
        parser.error("Compatibility updates require an arm64 or x86_64 macOS executable")
    # Standard .app tar layout is understood by installed desktop updaters.
    subprocess.run(["tar", "-czf", str(archive), "-C", str(args.bundle.parent), "--", args.bundle.name],
                   env={**os.environ, "COPYFILE_DISABLE": "1"}, check=True)
    password = sys.stdin.readline() if args.compat_password_stdin else "\n"
    if not password.endswith("\n"):
        password += "\n"
    with minisign_file(args.compat_signing_key, parser, private=True) as private, minisign_file(args.compat_public_key, parser) as public:
        with tempfile.TemporaryDirectory(prefix="vorkspace-update-signature-") as temporary:
            detached = Path(temporary) / "archive.minisig"
            signed = subprocess.run(["minisign", "-S", "-s", str(private), "-x", str(detached), "-m", str(archive)],
                                    input=password, text=True, capture_output=True)
            if signed.returncode:
                parser.error("Compatibility signing failed; check the signing key and supplied password")
            verified = subprocess.run(["minisign", "-V", "-p", str(public), "-x", str(detached), "-m", str(archive)], capture_output=True)
            if verified.returncode:
                parser.error("Compatibility signature does not match the supplied public key")
            encoded_signature = base64.b64encode(detached.read_bytes()).decode("ascii")
    signature.write_text(encoded_signature + "\n")
    entry = dict(signature=encoded_signature, url=args.download_url_prefix.rstrip("/") + "/" + archive.name)
    platforms = {f"darwin-{names[architecture]}{suffix}": entry
                 for architecture in architectures for suffix in ("", "-app")}
    feed = dict(version=info["CFBundleShortVersionString"], notes="", pub_date=datetime.now(timezone.utc).isoformat(), platforms=platforms)
    # Publish the discoverable root only after signing and verifying the archive.
    with tempfile.NamedTemporaryFile(mode="w", prefix=".latest-", dir=args.output, delete=False) as handle:
        temporary_feed = Path(handle.name)
        json.dump(feed, handle, indent=2)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    try:
        temporary_feed.replace(args.output / "latest.json")
    finally:
        temporary_feed.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--notary-profile", help="Optional existing notarytool Keychain profile")
    parser.add_argument("--ed-key-file", type=Path, help="Owner-only Sparkle Ed25519 seed file; defaults to Sparkle's Keychain key")
    parser.add_argument("--compat-signing-key", type=Path, help="Owner-only minisign key for installed desktop clients")
    parser.add_argument("--compat-public-key", type=Path, help="Existing desktop clients' minisign public key file")
    parser.add_argument("--compat-password-stdin", action="store_true", help="Read the compatibility key password from stdin; defaults to an empty password")
    parser.add_argument("--download-url-prefix", required=True, help="HTTPS directory where archives will be published")
    parser.add_argument("--sparkle", type=Path, default=Path("target/sparkle"))
    args = parser.parse_args()
    if bool(args.compat_signing_key) != bool(args.compat_public_key):
        parser.error("Compatibility signing and public keys must be supplied together")
    if args.compat_password_stdin and not args.compat_signing_key:
        parser.error("A compatibility signing key is required when reading its password")
    if args.compat_signing_key and not shutil.which("minisign"):
        parser.error("Install the open-source minisign command to prepare compatibility updates")
    parsed = urlparse(args.download_url_prefix)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        parser.error("The download directory must be an HTTPS URL without credentials, query or fragment")
    info = plistlib.loads((args.bundle / "Contents/Info.plist").read_bytes())
    if info.get("VorkspaceDataMode") != "vorkspace-data" or not info.get("SUPublicEDKey") or not info.get("SUFeedURL"):
        parser.error("Bundle must have explicit release update configuration")
    if not re.fullmatch(r"\d+(?:\.\d+){0,2}", info.get("CFBundleVersion", "")):
        parser.error("Bundle build number must contain one to three numeric components")
    if not re.fullmatch(r"\d+\.\d+\.\d+", info.get("CFBundleShortVersionString", "")):
        parser.error("Bundle version must contain three numeric components")
    if args.ed_key_file:
        protected_file(args.ed_key_file, parser, "Ed25519 key")
    if args.compat_signing_key:
        protected_file(args.compat_signing_key, parser, "Compatibility signing key")
    run("codesign", "--verify", "--deep", "--strict", args.bundle)
    if args.notary_profile:
        # Apple's notarization service accepts Developer ID signatures only.
        run("codesign", "--verify", "-R=anchor apple generic", args.bundle)
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / f"vorkspace-native-{info['CFBundleVersion']}.zip"
    if archive.exists():
        parser.error(f"Refusing to replace existing update: {archive}")
    if args.notary_profile:
        with tempfile.TemporaryDirectory(prefix="native-notarize-") as tmp:
            submission = Path(tmp) / "submission.zip"
            run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", args.bundle, submission)
            run("xcrun", "notarytool", "submit", submission, "--keychain-profile", args.notary_profile, "--wait")
            run("xcrun", "stapler", "staple", args.bundle)
            run("xcrun", "stapler", "validate", args.bundle)
            run("spctl", "--assess", "--type", "execute", args.bundle)
    run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", args.bundle, archive)
    # Sparkle verifies the public key matches the bundle. Pass a protected file
    # path or let Sparkle use Keychain; never expose private key bytes in argv.
    key_args = ("--ed-key-file", args.ed_key_file) if args.ed_key_file else ()
    run(args.sparkle / "bin/generate_appcast", *key_args, "--download-url-prefix", args.download_url_prefix.rstrip("/") + "/", args.output)
    # generate_appcast exits successfully even when a mismatched key causes an
    # unsigned enclosure. Refuse that output rather than announcing an unusable
    # update; the stock verifier checks its archive signature as well.
    signatures = [enclosure.get("{http://www.andymatuschak.org/xml-namespaces/sparkle}edSignature")
                  for feed in args.output.glob("*.xml")
                  for enclosure in ET.parse(feed).iter("enclosure")
                  if Path(unquote(urlparse(enclosure.get("url", "")).path)).name == archive.name
                  and enclosure.get("length") == str(archive.stat().st_size)]
    if not signatures or not all(signatures):
        parser.error("Sparkle did not sign this update; verify the signing key matches the bundle's SUPublicEDKey")
    for signature in set(signatures):
        run(args.sparkle / "bin/sign_update", "--verify", *key_args, archive, signature)
    if args.compat_signing_key:
        compatibility_update(args, info, parser)
    print(f"Prepared {archive} and its appcast. Nothing was uploaded.")


if __name__ == "__main__":
    main()
