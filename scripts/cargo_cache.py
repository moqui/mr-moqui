#!/usr/bin/env python3
"""Share dependency artifacts, not stale workspace outputs, between checkouts."""

import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def build(command, directory, environment, private):
    target = Path(environment["CARGO_TARGET_DIR"])
    if not target.is_absolute():
        target = directory / target
    target.mkdir(parents=True, exist_ok=True)
    # Hold through the copy, not just cargo build. All shared-cache builds must
    # use this wrapper; Cargo's own lock ends before an executable can be copied.
    with (target / ".moqui-build.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        manifest_args = []
        if "--manifest-path" in command:
            index = command.index("--manifest-path")
            manifest_args = command[index:index + 2]
        owner = str(directory.resolve()) + " " + " ".join(manifest_args)
        marker = target / ".moqui-checkout"
        if not marker.exists() or marker.read_text() != owner:
            # Cargo can falsely report path packages fresh after A -> B -> A.
            # Invalidate only local packages; registry/git dependencies stay warm.
            metadata = json.loads(subprocess.check_output(
                ["cargo", "metadata", "--format-version", "1", *manifest_args,
                 *[flag for flag in ("--locked", "--offline", "--frozen") if flag in command]],
                cwd=directory, env=environment, text=True))
            packages = [package["name"] for package in metadata["packages"] if package["source"] is None]
            if not packages:
                raise RuntimeError("Cargo metadata contains no local packages")
            clean = ["cargo", "clean", *manifest_args]
            for package in packages:
                clean.extend(["-p", package])
            subprocess.run(clean, cwd=directory, env=environment, check=True)
            marker.write_text(owner)
        subprocess.run(command, cwd=directory, env=environment, check=True)
        profile = "release" if "--release" in command else "debug"
        source = target / profile / "moqui"
        private.parent.mkdir(parents=True, exist_ok=True)
        temporary = private.with_suffix(".tmp")
        shutil.copy2(source, temporary)
        temporary.replace(private)


if __name__ == "__main__":
    build(sys.argv[2:], Path.cwd(), os.environ.copy(), Path(sys.argv[1]))
