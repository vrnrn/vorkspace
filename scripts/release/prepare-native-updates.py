"""Download a pinned Sparkle distribution. Does not generate keys or publish."""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.request

# Keep the supported macOS 11 floor: Sparkle 2.10 requires macOS 12.
VERSION = "2.9.6"
DIGEST = "52bf9e88cdd972fc0c81501377a880e90d47031bd8ca5462488f843e2609e192"
output = Path(sys.argv[1] if len(sys.argv) > 1 else "target/sparkle")
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix="vorkspace-sparkle-") as directory:
    archive = Path(directory) / "sparkle.tar.xz"
    urllib.request.urlretrieve(f"https://github.com/sparkle-project/Sparkle/releases/download/{VERSION}/Sparkle-{VERSION}.tar.xz", archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != DIGEST:
        raise RuntimeError("Sparkle archive checksum did not match")
    subprocess.run(["tar", "-xf", str(archive), "-C", str(output)], check=True)
print(output.resolve())
