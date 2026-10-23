"""Validate native bundle metadata before any signing or distribution."""
import base64
import os
from pathlib import Path
import plistlib
import re
import sys
from urllib.parse import urlparse

RELEASE_FEED = "https://github.com/vrnrn/vorkspace/releases/latest/download/appcast.xml"
RELEASE_PUBLIC_KEY = Path(__file__).resolve().parents[1] / "release/sparkle-public-key.txt"


def configure(plist, env):
    result = dict(plist)
    for key in ("CFBundleShortVersionString", "CFBundleVersion"):
        value = env.get("VORKSPACE_BUILD_NUMBER", result[key]) if key == "CFBundleVersion" else result[key]
        if not re.fullmatch(r"\d+(?:\.\d+){0,2}", value):
            raise ValueError(f"Invalid {key}")
        result[key] = value
    result["LSMinimumSystemVersion"] = "11.0"
    result["NSMicrophoneUsageDescription"] = "Vox uses your selected microphone only while a voice call is active."
    feed, key, framework = (env.get(k) for k in ("VORKSPACE_UPDATE_FEED", "VORKSPACE_UPDATE_PUBLIC_KEY", "VORKSPACE_SPARKLE_FRAMEWORK"))
    if any((feed, key, framework)):
        if not framework:
            raise ValueError("An update feed requires the Sparkle framework")
        feed = feed or RELEASE_FEED
        key = key or RELEASE_PUBLIC_KEY.read_text().strip()
        url = urlparse(feed)
        if url.scheme != "https" or not url.hostname or url.username or url.password or url.fragment:
            raise ValueError("Update feed must be an HTTPS URL without credentials")
        if len(base64.b64decode(key, validate=True)) != 32:
            raise ValueError("Sparkle requires a 32-byte Ed25519 public key")
        if result.get("VorkspaceDataMode") != "vorkspace-data":
            raise ValueError("Development bundles cannot use a release update feed")
        if not Path(framework).is_dir():
            raise ValueError("Sparkle framework is missing")
        framework_info = plistlib.loads((Path(framework) / "Resources/Info.plist").read_bytes())
        minimum = framework_info.get("LSMinimumSystemVersion", "")
        if not re.fullmatch(r"\d+(?:\.\d+){0,2}", minimum):
            raise ValueError("Sparkle framework must declare its minimum macOS version")
        minimum_parts = tuple(map(int, minimum.split(".")))
        if minimum_parts + (0,) * (3 - len(minimum_parts)) > (11, 0, 0):
            raise ValueError("Sparkle framework requires a newer macOS version than the application")
        result.update(SUFeedURL=feed, SUPublicEDKey=key, SUEnableAutomaticChecks=True,
                      SUAutomaticallyUpdate=False, SUAllowsAutomaticUpdates=True,
                      SUVerifyUpdateBeforeExtraction=True)
    return result


if __name__ == "__main__":
    path = Path(sys.argv[1])
    value = configure(plistlib.loads(path.read_bytes()), os.environ)
    # Keep one-line key/value pairs: the executable reads its mode before AppKit starts.
    data = plistlib.dumps(value).decode().replace("</key>\n\t<string>", "</key><string>")
    path.write_text(data)
