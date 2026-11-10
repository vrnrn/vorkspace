"""Publish a staged macOS application without a gap at its installed path."""

import ctypes
import errno
import os
import sys


def publish(staged, destination):
    rename = ctypes.CDLL(None, use_errno=True).renamex_np
    rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    source = os.fsencode(staged)
    target = os.fsencode(destination)
    # Darwin's sys/stdio.h defines these flags for renamex_np.
    rename_swap = 0x00000002
    rename_excl = 0x00000004
    if rename(source, target, rename_excl) == 0:
        return
    error = ctypes.get_errno()
    if error == errno.EEXIST:
        # On success the previous bundle is now under the staging directory.
        if rename(source, target, rename_swap) == 0:
            return
        error = ctypes.get_errno()
    raise OSError(error, os.strerror(error), destination)


if __name__ == "__main__":
    try:
        publish(*sys.argv[1:])
    except OSError as error:
        sys.exit(str(error))
