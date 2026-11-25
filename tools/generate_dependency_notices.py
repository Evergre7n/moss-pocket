"""Generate the third-party notices shipped with the Windows release."""

import argparse
from collections import defaultdict
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from urllib.parse import quote


ROOT = Path(__file__).resolve().parents[1]
SUPPLEMENTS = ROOT / "docs/licenses/dependencies"
TARGET = "x86_64-pc-windows-gnullvm"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
NOTICE_NAME = re.compile(r"^(licen[sc]es?|copying|copyright|notice|authors)([-_.]|$)", re.I)
PACKAGE_LINE = re.compile(r"^(\S+) v(\S+)")
EGUI_PACKAGES = {
    "ecolor", "eframe", "egui", "egui-wgpu", "egui-winit", "egui_glow",
    "egui_system_font_provider", "emath", "epaint", "epaint_default_fonts",
}
FONT_NOTICES = (
    "fonts/Hack-Regular.txt",
    "fonts/UFL.txt",
    "fonts/emoji-icon-font-mit-license.txt",
    "fonts/egui-icons.txt",
    "fonts/OFL.txt",
)
OPUS_NOTICES = ("opus/COPYING", "opus/AUTHORS")
KHRONOS_REGISTRIES = {
    "api/xml/gl.xml": ("<comment>", "</comment>", "OPENGL-NOTICE.txt"),
    "api/xml/wgl.xml": ("<comment>", "</comment>", "WGL-NOTICE.txt"),
    "api_egl/api/egl.xml": ("<!--", "-->", "EGL-NOTICE.txt"),
}


def run(executable, *args, launcher=None):
    command = [str(executable), *args]
    if launcher:
        command = [str(launcher), "--exec", *command]
    return subprocess.check_output(
        command, cwd=ROOT, encoding="utf-8",
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )


def release_packages(cargo, launcher):
    tree = run(
        cargo, "tree", "-p", "mp-app", "--locked", "--offline", "--target", TARGET,
        "--edges", "normal,build", "--prefix", "none", "--format", "{p}",
        launcher=launcher,
    )
    keys = set()
    for line in tree.splitlines():
        if not line:
            continue
        match = PACKAGE_LINE.match(line)
        if not match:
            raise ValueError(f"Unexpected cargo tree entry: {line}")
        keys.add(match.groups())
    metadata = json.loads(run(
        cargo, "metadata", "--locked", "--offline", "--format-version", "1",
        "--filter-platform", TARGET, launcher=launcher,
    ))
    packages = {}
    for pkg in metadata["packages"]:
        key = (pkg["name"], pkg["version"])
        if key in keys:
            if key in packages:
                raise ValueError(f"Ambiguous package in the release graph: {key}")
            packages[key] = pkg
    if keys != packages.keys():
        raise ValueError(f"Missing package metadata: {sorted(keys - packages.keys())}")
    return [pkg for _, pkg in sorted(packages.items()) if pkg["source"]]


def supplement_group(name, version):
    if name in EGUI_PACKAGES and version == "0.37.0":
        return "egui-0.37.0"
    if name.startswith("symphonia") and version == "0.5.5":
        return "symphonia-0.5.5"
    return f"{name}-{version}"


def write_bytes(output, relative_path, data):
    path = output / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def links(paths):
    return ", ".join(f"[{path.name}]({path.as_posix()})" for path in paths)


def collect_crate(output, pkg, checksum, supplements):
    name, version = pkg["name"], pkg["version"]
    label = f"{name}-{version}"
    if pkg["source"] != REGISTRY or not checksum:
        raise ValueError(f"Unrecognized source for {label}: {pkg['source']}")
    manifest = Path(pkg["manifest_path"])
    registry_dir = manifest.parents[1]
    archive = registry_dir.parents[1] / "cache" / registry_dir.name / f"{label}.crate"
    archive_contents = archive.read_bytes()
    if hashlib.sha256(archive_contents).hexdigest() != checksum:
        raise ValueError(f"Cargo.lock checksum mismatch: {label}")

    extra = set()
    if name == "opusic-sys" and version == "0.6.0":
        extra.update(OPUS_NOTICES)
    if name == "epaint_default_fonts" and version == "0.37.0":
        extra.update(FONT_NOTICES)
    if name == "khronos_api" and version == "3.1.0":
        extra.update(KHRONOS_REGISTRIES)
    if pkg["license_file"]:
        extra.add(pkg["license_file"])

    notice_dir = Path("licenses/crates") / label
    written = []
    missing = extra.copy()
    with tarfile.open(fileobj=io.BytesIO(archive_contents), mode="r:gz") as crate:
        for member in crate:
            if not member.isfile():
                continue
            path = Path(member.name).relative_to(label)
            rel = path.as_posix()
            if rel not in extra and (len(path.parts) != 1 or not NOTICE_NAME.match(path.name)):
                continue
            if name == "self_cell" and version == "1.3.0" and rel == "LICENSE-GPLv2":
                continue
            data = crate.extractfile(member).read()
            if rel in KHRONOS_REGISTRIES:
                begin, end, filename = KHRONOS_REGISTRIES[rel]
                header = data.decode("utf-8").split(begin, 1)[1].split(end, 1)[0].strip("\r\n")
                if "Copyright" not in header:
                    raise ValueError(f"Missing Khronos notice header: {rel}")
                path = Path(filename)
                data = (header + "\n").encode("utf-8")
            destination = notice_dir / path
            write_bytes(output, destination, data)
            written.append(destination)
            missing.discard(rel)
    if missing:
        raise ValueError(f"Missing license files in {label}: {sorted(missing)}")

    for entry in supplements[supplement_group(name, version)]:
        source = Path(entry["file"])
        data = (SUPPLEMENTS / source).read_bytes()
        if hashlib.sha256(data).hexdigest() != entry["sha256"]:
            raise ValueError(f"Supplementary license checksum mismatch: {source}")
        destination = Path("licenses/upstream") / source
        write_bytes(output, destination, data)
        written.append(destination)
    if not written:
        raise ValueError(f"No license or copyright notice found for {label}")
    return sorted(written)


def copy_runtime_notices(output, rustc, llvm_mingw, launcher):
    version = run(rustc, "-vV", launcher=launcher)
    fields = dict(line.split(": ", 1) for line in version.splitlines()[1:])
    if fields["host"] != TARGET:
        raise ValueError(f"Expected a {TARGET} rustc, found {fields['host']}")
    sysroot = Path(run(rustc, "--print", "sysroot", launcher=launcher).strip())
    rust_docs = sysroot / "share/doc/rust"
    rust_licenses = [
        ("COPYRIGHT-library.html", rust_docs / "COPYRIGHT-library.html"),
        ("Apache-2.0.txt", rust_docs / "licenses/Apache-2.0.txt"),
        ("MIT.txt", rust_docs / "licenses/MIT.txt"),
    ]
    for filename, source in rust_licenses:
        write_bytes(output, Path("licenses/native/Rust") / filename, source.read_bytes())

    if llvm_mingw.name != "llvm-mingw-20260616-ucrt-x86_64":
        raise ValueError("Review the release runtime notices for the selected LLVM-MinGW build")
    write_bytes(
        output, Path("licenses/native/LLVM/LICENSE.TXT"),
        (llvm_mingw / "LICENSE.TXT").read_bytes(),
    )
    mingw_docs = llvm_mingw / "x86_64-w64-mingw32/share/mingw32"
    for filename in (
        "COPYING", "COPYING.MinGW-w64.txt", "COPYING.MinGW-w64-runtime.txt",
        "COPYING.winpthreads.txt", "COPYING.winstorecompat.txt",
    ):
        write_bytes(
            output, Path("licenses/native/MinGW-w64") / filename,
            (mingw_docs / filename).read_bytes(),
        )
    return fields["release"], fields["commit-hash"]


def generate(output, packages, rustc, llvm_mingw, launcher):
    lockfile = (ROOT / "Cargo.lock").read_bytes()
    locks = tomllib.loads(lockfile.decode("utf-8"))["package"]
    checksums = defaultdict(dict)
    for pkg in locks:
        checksums[pkg.get("source")][(pkg["name"], pkg["version"])] = pkg.get("checksum")
    sources = json.loads((SUPPLEMENTS / "sources.json").read_text(encoding="utf-8-sig"))
    supplements = defaultdict(list)
    for entry in sources:
        supplements[Path(entry["file"]).parts[0]].append(entry)
    rust_version, rust_commit = copy_runtime_notices(output, rustc, llvm_mingw, launcher)

    index = [
        "# Third-party notices for MOSS-Pocket",
        "",
        f"This Windows release targets `{TARGET}`. The list below records the {len(packages)}",
        "third-party crates in the locked application graph, including build dependencies.",
        f"Cargo.lock SHA-256 is `{hashlib.sha256(lockfile).hexdigest()}`.",
        "",
        "## Audio and fonts",
        "",
        "- Symphonia 0.5.5 and its listed components are available in Source Code Form under",
        "  the Mozilla Public License 2.0. Get the exact source archives from the links",
        "  below or browse the upstream source at the published revision.",
        "  [MPL 2.0 text](licenses/upstream/symphonia-0.5.5/LICENSE)",
        "  [Symphonia source](https://github.com/pdeljanov/Symphonia/tree/6d533f26150953a882a6a111ebd13f0abf7129d5)",
        "- The executable includes Opus 1.5.2, built from the reference implementation",
        "  shipped by opusic-sys 0.6.0. Its terms and patent notices are in the",
        "  [Opus COPYING file](licenses/crates/opusic-sys-0.6.0/opus/COPYING).",
        "  [Opus upstream source](https://opus-codec.org/downloads/)",
        "- egui embeds Hack Regular, Ubuntu Light and its emoji-icon-font subset.",
        "  The upstream font notices accompany epaint_default_fonts below.",
        "  Named Windows fonts such as Segoe UI and Microsoft YaHei load from the",
        "  installed system font directory. The archive contains no Windows font files.",
        "",
        "## Native runtime",
        "",
        f"- Rust standard library {rust_version}. [Copyright notices](licenses/native/Rust/COPYRIGHT-library.html)",
        f"  [Rust source](https://github.com/rust-lang/rust/tree/{rust_commit}/library)",
        "- LLVM-MinGW 20260616 with LLVM 22.1.8 provides compiler builtins and",
        "  MinGW-w64 startup/runtime code. Windows supplies the UCRT and system DLLs.",
        "  [LLVM license](licenses/native/LLVM/LICENSE.TXT)",
        "  [MinGW-w64 runtime notices](licenses/native/MinGW-w64/COPYING.MinGW-w64-runtime.txt)",
        "  [LLVM-MinGW source](https://github.com/mstorsjo/llvm-mingw/tree/20260616)",
        "  [MinGW-w64 source](https://github.com/mingw-w64/mingw-w64/tree/c28e9555bb8800c53449f42a465ad9a5676fce88)",
        "",
        "## Rust dependencies",
        "",
        "Each entry lists the upstream license expression and links to the released source",
        "archive and the accompanying notices. Legacy slash-separated license alternatives",
        "are reproduced as declared by upstream. This release uses the Apache-2.0 option",
        "for self_cell 1.3.0.",
        "",
    ]
    for pkg in packages:
        name, version = pkg["name"], pkg["version"]
        files = collect_crate(
            output, pkg, checksums[pkg["source"]].get((name, version)), supplements,
        )
        archive = f"https://static.crates.io/crates/{name}/{name}-{quote(version)}.crate"
        index.append(f"- **{name} {version}**. {pkg['license']}. [Source]({archive}), {links(files)}.")
    notice = output / "THIRD-PARTY-NOTICES.md"
    notice.write_text("\n".join(index) + "\n", encoding="utf-8", newline="\n")
    print(f"Generated notices for {len(packages)} third-party crates.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--llvm-mingw", type=Path, required=True)
    parser.add_argument("--cargo", type=Path)
    parser.add_argument("--rustc", type=Path)
    parser.add_argument("--launcher", type=Path, help="Optional Windows headless process launcher.")
    args = parser.parse_args()
    cargo = args.cargo or shutil.which("cargo")
    rustc = args.rustc or shutil.which("rustc")
    if not cargo or not rustc:
        parser.error("Cargo and rustc must be available on PATH or supplied as arguments")
    output = args.output.resolve()
    if output.exists():
        parser.error(f"Output already exists; choose a new directory: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    packages = release_packages(cargo, args.launcher)
    with tempfile.TemporaryDirectory(dir=output.parent) as temp:
        staging = Path(temp) / "notices"
        generate(staging, packages, rustc, args.llvm_mingw.resolve(), args.launcher)
        staging.rename(output)


if __name__ == "__main__":
    main()
