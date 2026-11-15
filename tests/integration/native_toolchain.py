"""Common compiler inputs and compile deadlines for native fixture runners."""

import os
import platform
import sys


def clang():
    if sys.platform != "linux":
        return ["clang"]
    arguments = [os.environ.get("RINZU_CLANG", "clang"), "-D_GNU_SOURCE"]
    if platform.machine() == "x86_64":
        arguments += ["-target", "x86_64-unknown-linux-gnu"]
    if sysroot := os.environ.get("RINZU_SYSROOT"):
        arguments += ["--sysroot", sysroot]
    return arguments


def tool_timeout(default):
    return int(os.environ.get("RINZU_TEST_TOOL_TIMEOUT", default))
