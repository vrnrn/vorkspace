"""Download a pinned Sparkle distribution. Does not generate keys or publish."""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.request

VERSION = "2.10.0"
DIGEST = "c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c"
output = Path(sys.argv[1] if len(sys.argv) > 1 else "target/sparkle")
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix="vorkspace-sparkle-") as directory:
    archive = Path(directory) / "sparkle.tar.xz"
    urllib.request.urlretrieve(f"https://github.com/sparkle-project/Sparkle/releases/download/{VERSION}/Sparkle-{VERSION}.tar.xz", archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != DIGEST:
        raise RuntimeError("Sparkle archive checksum did not match")
    subprocess.run(["tar", "-xf", str(archive), "-C", str(output)], check=True)
print(output.resolve())
