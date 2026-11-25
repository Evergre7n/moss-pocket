"""Package a tested Windows executable and its matching public source tree."""

import argparse
import hashlib
from pathlib import Path
import tempfile
import tomllib
import zipfile


ROOT = Path(__file__).resolve().parents[1]
DOCS = ("docs/format.md", "docs/ASSET-NOTICES.md", "docs/THIRD-PARTY-AUDIO.txt")
BRAND = ("icon.rc", "relay-icon.ico", "relay-icon.png", "relay-wordmark.png",
         "relay-wordmark-light.png")


def tree(base, prefix):
    for path in sorted(base.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Unexpected symbolic link in release inputs: {path}")
        if path.is_file():
            yield path, f"{prefix}/{path.relative_to(base).as_posix()}"


def public_docs():
    for name in ("README.md", "README.en.md", "LICENSE", *DOCS):
        yield ROOT / name, name
    yield from tree(ROOT / "docs/licenses/assets", "docs/licenses/assets")


def source_files():
    for name in ("Cargo.toml", "Cargo.lock", ".gitignore", "crates/mp-app/Cargo.toml",
                 "crates/mp-app/build.rs", "crates/mp-core/Cargo.toml",
                 "docs/THIRD-PARTY-DEPENDENCIES.md", "tools/build_release.py",
                 "tools/generate_dependency_notices.py", "tools/package_release.py",
                 "tools/benchmark.py"):
        yield ROOT / name, name
    for name in BRAND:
        path = f"crates/mp-app/assets/{name}"
        yield ROOT / path, path
    for fixture in ("short-aac.m4a", "short-asf-wma.wma", "short-ogg-opus.opus"):
        name = f"crates/mp-app/tests/fixtures/{fixture}"
        yield ROOT / name, name
    for crate in ("mp-app", "mp-core"):
        for folder in ("src", "tests"):
            name = f"crates/{crate}/{folder}"
            for path, entry in tree(ROOT / name, name):
                if path.suffix == ".rs":
                    yield path, entry
    yield from tree(ROOT / "docs/licenses/dependencies", "docs/licenses/dependencies")


def archive(path, prefix, files):
    manifest = {}
    with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_DEFLATED) as out:
        for source, name in files:
            entry = f"{prefix}/{name}"
            if entry in manifest:
                raise ValueError(f"Duplicate release entry: {entry}")
            content = source.read_bytes()
            manifest[entry] = hashlib.sha256(content).digest()
            info = zipfile.ZipInfo(entry, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 0
            out.writestr(info, content, compress_type=zipfile.ZIP_DEFLATED)
    with zipfile.ZipFile(path) as built:
        if set(built.namelist()) != manifest.keys():
            raise ValueError(f"Unexpected release entries: {path}")
        for entry, digest in manifest.items():
            if hashlib.sha256(built.read(entry)).digest() != digest:
                raise ValueError(f"Archive verification failed: {entry}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--notices", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text("utf-8"))["workspace"]["package"]["version"]
    prefix = f"MOSS-Pocket-{version}"
    notices = args.notices / "THIRD-PARTY-NOTICES.md"
    if not notices.is_file() or not (args.notices / "licenses/crates").is_dir():
        parser.error("Generate the locked Windows third-party notices before packaging.")
    shared = [*public_docs(), (notices, notices.name),
              *tree(args.notices / "licenses", "licenses")]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.output.exists():
        parser.error("The release destination already exists. Choose a new directory.")
    with tempfile.TemporaryDirectory(dir=args.output.parent) as scratch:
        stage = Path(scratch) / prefix
        stage.mkdir()
        build = stage / f"{prefix}-windows-x64.zip"
        source = stage / f"{prefix}-source.zip"
        archive(build, prefix, [(args.exe, "MOSS-Pocket.exe"), *shared])
        archive(source, prefix, [*source_files(), *shared])
        entries = [f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}" for p in (build, source)]
        (stage / "SHA256SUMS.txt").write_text("\n".join(entries) + "\n", encoding="ascii")
        stage.rename(args.output)


if __name__ == "__main__":
    main()
