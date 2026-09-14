#!/usr/bin/env python3
"""Package a native CLI build, verify checksums after extraction, and smoke-test it."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def version_for_tag(tag):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    if tag != f"cli-v{version}":
        raise ValueError(f"tag {tag!r} does not match Cargo.toml version cli-v{version}")
    return version


def notices(target):
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target,
    ], cwd=ROOT))
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    root_id = next(p["id"] for p in metadata["packages"] if p["name"] == "autotrim")
    pending = [root_id]
    seen = set()
    chunks = ["Third-party notices for this CLI build. Package license expressions are reproduced below.\n"]
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        pending.extend(nodes[key]["dependencies"])
        package = packages[key]
        if key == root_id:
            continue
        chunks.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package.get('license') or 'See package license file'}\nRepository: {package.get('repository') or ''}\n")
        directory = Path(package["manifest_path"]).parent
        files = {p for p in directory.iterdir() if p.is_file() and p.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE"))}
        if package.get("license_file"):
            files.add(directory / package["license_file"])
        for path in sorted(files):
            chunks.append(f"\n--- {path.name} ---\n{path.read_text(errors='replace')}\n")
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    rust_docs = sysroot / "share" / "doc" / "rust"
    for name in ["COPYRIGHT", "LICENSE-MIT", "LICENSE-APACHE"]:
        path = rust_docs / name
        if path.exists():
            chunks.append(f"\n--- Rust toolchain {name} ---\n{path.read_text(errors='replace')}\n")
    return "".join(chunks)


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def package(tag, target, output):
    version = version_for_tag(tag)
    platform = "windows" if "windows" in target else "macos" if "apple" in target else "linux"
    arch = target.split("-")[0]
    name = f"autotrim-{version}-{platform}-{arch}"
    exe = "autotrim.exe" if platform == "windows" else "autotrim"
    source = ROOT / "target" / target / "release" / exe
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="autotrim-release-") as temporary:
        temp = Path(temporary)
        staging = temp / name
        staging.mkdir()
        shutil.copy2(source, staging / exe)
        shutil.copy2(ROOT / "README.md", staging / "README.md")
        shutil.copytree(ROOT / "docs", staging / "docs")
        if (ROOT / "PRIVACY.md").exists():
            shutil.copy2(ROOT / "PRIVACY.md", staging / "PRIVACY.md")
        shutil.copy2(ROOT / "docs" / "cli-releases.md", staging / "INSTALL.md")
        (staging / "NOTICE.txt").write_text("autoTrim CLI " + version + "\nSource: https://github.com/Aaroney13/autoTrim\n" +
            "This repository currently specifies no project license. This notice does not grant a license.\n" +
            "Third-party licenses and notices are in THIRD_PARTY_NOTICES.txt.\n", encoding="utf-8")
        (staging / "THIRD_PARTY_NOTICES.txt").write_text(notices(target), encoding="utf-8")
        (staging / "SHA256SUMS").write_text("".join(f"{digest(p)}  {p.relative_to(staging).as_posix()}\n" for p in sorted(staging.rglob("*")) if p.is_file()), encoding="utf-8")
        archive = output / (name + (".zip" if platform == "windows" else ".tar.gz"))
        if platform == "windows":
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
                for path in sorted(staging.rglob("*")):
                    if path.is_file():
                        z.write(path, f"{name}/{path.relative_to(staging).as_posix()}")
        else:
            with tarfile.open(archive, "w:gz") as t:
                t.add(staging, arcname=name)
        extracted = temp / "extracted"
        extracted.mkdir()
        if platform == "windows":
            with zipfile.ZipFile(archive) as z:
                z.extractall(extracted)
        else:
            with tarfile.open(archive) as t:
                t.extractall(extracted, filter="data")
        artifact = extracted / name
        for line in (artifact / "SHA256SUMS").read_text().splitlines():
            checksum, filename = line.split("  ", 1)
            if digest(artifact / filename) != checksum:
                raise ValueError(f"checksum mismatch: {filename}")
        env = {**os.environ, "AUTOTRIM_DATA_DIR": str(temp / "scratch")}
        got = subprocess.check_output([str(artifact / exe), "--version"], text=True, env=env).strip()
        if got != f"autotrim {version}":
            raise ValueError(f"unexpected version: {got}")
        subprocess.run([str(artifact / exe), "--help"], check=True, stdout=subprocess.DEVNULL, env=env)
        (output / (archive.name + ".sha256")).write_text(f"{digest(archive)}  {archive.name}\n", encoding="utf-8")
        print(f"Verified {archive.name}: checksums, --version, --help")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--target")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--check-version", action="store_true")
    args = parser.parse_args()
    if args.check_version:
        print(version_for_tag(args.tag))
    elif not args.target:
        parser.error("--target is required for packaging")
    else:
        package(args.tag, args.target, args.output)
