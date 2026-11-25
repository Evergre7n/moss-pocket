"""Build the Windows release without embedding the builder's local paths."""

import argparse
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    target_dir = (args.target_dir or repo / "target").resolve()
    # Rust chooses the last matching prefix. Keep more specific mappings last.
    paths = {
        Path.home(): "/build/home",
        Path(env.get("CARGO_HOME", Path.home() / ".cargo")).resolve(): "/cargo",
        repo: "/src/moss-pocket",
        target_dir: "/build/target",
    }
    flags = env.get("CARGO_ENCODED_RUSTFLAGS", "").split("\x1f")
    if "RUSTFLAGS" in env:
        if any(flags):
            parser.error("Use only one of RUSTFLAGS or CARGO_ENCODED_RUSTFLAGS.")
        flags = env.pop("RUSTFLAGS").split()
    flags = [flag for flag in flags if flag]
    flags.extend(f"--remap-path-prefix={old}={new}" for old, new in paths.items())
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    native = " ".join(f'"-ffile-prefix-map={old.as_posix()}={new}"' for old, new in paths.items())
    for key in ("CFLAGS", "CXXFLAGS"):
        env[key] = f"{env.get(key, '')} {native}".strip()
    return subprocess.run(
        ["cargo", "build", "--locked", "--release", "-p", "mp-app",
         "--target-dir", str(target_dir)],
        cwd=repo, env=env, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        check=False,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
