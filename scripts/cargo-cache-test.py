#!/usr/bin/env python3
"""Real, offline Cargo cache/checkout isolation tests; no full Moqui build."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
# Also exercise the regression controller's implementation of the same protocol.
HELPER = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "scripts/cargo_cache.py"


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)
    return result


with tempfile.TemporaryDirectory(prefix="moqui-shared-cache-") as temporary:
    root = Path(temporary)
    dependency = root / "dependency"
    (dependency / "src").mkdir(parents=True)
    (dependency / "Cargo.toml").write_text(
        '[package]\nname="cache_dep"\nversion="0.1.0"\nedition="2021"\n')
    (dependency / "src/lib.rs").write_text('pub fn value() -> u32 { 42 }\n')
    run(["git", "init", "-q", str(dependency)])
    run(["git", "-C", str(dependency), "add", "."])
    run(["git", "-C", str(dependency), "-c", "user.name=Test", "-c",
         "user.email=test@example.com", "commit", "-qm", "fixture"])
    environment = {**os.environ, "CARGO_TARGET_DIR": str(root / "shared target"),
                   "CARGO_HOME": str(root / "cargo-home"), "CARGO_TERM_VERBOSE": "true"}
    for name in ("a", "b"):
        checkout = root / name
        for directory in ("src", "local/src", "runtime/conf", "scripts"):
            (checkout / directory).mkdir(parents=True)
        (checkout / "runtime/conf/MoquiDevConf.xml").write_text("<moqui-conf/>")
        shutil.copy2(ROOT / "moq", checkout / "moq")
        shutil.copy2(HELPER, checkout / "scripts/cargo_cache.py")
        # Excluded path dependency models add-on builds, where the framework
        # crates are outside the generated application's workspace.
        (checkout / "Cargo.toml").write_text(
            '[package]\nname="moqui"\nversion="0.1.0"\nedition="2021"\n'
            '[workspace]\nexclude=["local"]\n[dependencies]\nlocal={path="local"}\n'
            'cache_dep={git="file://' + str(dependency) + '"}\n')
        (checkout / "src/main.rs").write_text(
            'fn main() { println!("{}: {}", local::value(), cache_dep::value()); }\n')
        (checkout / "local/Cargo.toml").write_text(
            '[package]\nname="local"\nversion="0.1.0"\nedition="2021"\n')
        (checkout / "local/src/lib.rs").write_text(
            f'pub fn value() -> &\'static str {{ "{name}" }}\n')

    for index, name in enumerate(("a", "b", "a", "a")):
        result = run([str(root / name / "moq"), "run", "--debug"],
                     env=environment, cwd=root)
        assert result.stdout.strip() == f"{name}: 42", result.stdout + result.stderr
        assert not (root / name / "target/debug").exists()
        if index:
            assert "Fresh cache_dep" in result.stderr, result.stderr
        if index == 3:
            assert "Fresh moqui" in result.stderr and "Fresh local" in result.stderr, result.stderr
    # Parallel cooperating builds must each launch their own private binary.
    processes = [subprocess.Popen([str(root / name / "moq"), "run", "--debug"],
                                 env=environment, cwd=root, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True) for name in ("a", "b")]
    for name, process in zip(("a", "b"), processes):
        stdout, stderr = process.communicate()
        assert process.returncode == 0 and stdout.strip() == f"{name}: 42", stdout + stderr
        private = root / name / "target/moq-bin/debug/moqui"
        assert run([str(private)]).stdout.strip() == f"{name}: 42"

print("PASS: dependency reuse, A -> B -> A, same-checkout incrementality, parallel private binaries")
