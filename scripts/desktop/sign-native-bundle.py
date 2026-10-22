"""Sign an assembled macOS bundle inside-out using a supplied Developer ID."""
from pathlib import Path
import os
import subprocess
import sys

bundle = Path(sys.argv[1])
identity = sys.argv[2]
framework = bundle / "Contents/Frameworks/Sparkle.framework"
if framework.exists():
    targets = sorted((p for p in framework.rglob("*") if not p.is_symlink()
                      and (p.suffix in (".app", ".xpc") or p.is_file() and os.access(p, os.X_OK))),
                     key=lambda p: len(p.parts), reverse=True)
    for target in [*targets, framework]:
        subprocess.run(["codesign", "--force", "--timestamp", "--options", "runtime", "--sign", identity, str(target)], check=True)
subprocess.run(["codesign", "--force", "--timestamp", "--options", "runtime", "--entitlements", str(Path(__file__).with_name("native-entitlements.plist")), "--sign", identity, str(bundle)], check=True)
