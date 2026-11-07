"""Reject native app executables built for a different macOS deployment target."""

import re
import subprocess
import sys


def verify_load_commands(commands, binary):
    # Fat binaries report one load-command list per architecture.
    slices = re.split(r"(?m)^.*\(architecture [^)]+\):\n", commands)[1:] or [commands]
    for commands in slices:
        minimums = re.findall(r"^\s*minos\s+(\S+)", commands, re.MULTILINE)
        minimums += re.findall(
            r"cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\S+)", commands
        )
        if len(minimums) != 1 or minimums[0] not in ("26.0", "26.0.0"):
            raise ValueError(f"{binary}: expected macOS 26.0 in every slice; found {minimums}")


def main():
    for binary in sys.argv[1:]:
        commands = subprocess.check_output(["otool", "-arch", "all", "-l", binary], text=True)
        verify_load_commands(commands, binary)


if __name__ == "__main__":
    main()
