#!/usr/bin/env python3
"""Validate release versions, package native binaries, and verify release assets."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import tarfile
import tomllib
import zipfile

TARGETS = (
    "x86_64-pc-windows-msvc", "aarch64-apple-darwin",
    "x86_64-apple-darwin", "x86_64-unknown-linux-gnu",
)
IDENTIFIER = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER = re.compile(rf"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-{IDENTIFIER}(?:\.{IDENTIFIER})*)?")


def version_from_manifest(manifest=Path("Cargo.toml")):
    version = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["version"]
    if not SEMVER.fullmatch(version):
        raise ValueError("expected SemVer without build metadata: 0.4.0 or 0.5.0-rc.1")
    return version


def validate_tag(tag, version):
    if tag != f"v{version}":
        raise ValueError(f"tag {tag!r} must match Cargo.toml exactly: v{version}")


def asset_name(version, target):
    if not SEMVER.fullmatch(version) or target not in TARGETS:
        raise ValueError("invalid version or target")
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"agentdrop-v{version}-{target}{suffix}"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def package(version, target, source=Path("."), destination=Path("dist")):
    asset = asset_name(version, target)
    binary = "agentdrop.exe" if "windows" in target else "agentdrop"
    files = {
        binary: source / "target" / target / "release" / binary,
        "README.md": source / "README.md", "LICENSE": source / "LICENSE",
        "v2-design.md": source / "docs/v2-design.md",
    }
    for path in files.values():
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"missing or empty package file: {path}")
    destination.mkdir(parents=True, exist_ok=True)
    output = destination / asset
    root = asset.removesuffix(".tar.gz").removesuffix(".zip")
    if asset.endswith(".zip"):
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, path in files.items():
                archive.write(path, f"{root}/{name}")
    else:
        with tarfile.open(output, "w:gz") as archive:
            for name, path in files.items():
                info = archive.gettarinfo(str(path), f"{root}/{name}")
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mode = 0o755 if name == binary else 0o644
                with path.open("rb") as stream:
                    archive.addfile(info, stream)
    (destination / f"{asset}.sha256").write_text(f"{digest(output)}  {asset}\n", encoding="utf-8")
    return output


def checksums(version, directory):
    expected = {asset_name(version, target) for target in TARGETS}
    actual = {p.name for p in directory.iterdir() if p.name.endswith((".zip", ".tar.gz"))}
    if actual != expected:
        raise ValueError(f"asset set mismatch: missing={expected - actual}, extra={actual - expected}")
    lines = []
    for name in sorted(expected):
        line = f"{digest(directory / name)}  {name}\n"
        if (directory / f"{name}.sha256").read_text(encoding="utf-8") != line:
            raise ValueError(f"checksum mismatch: {name}")
        lines.append(line)
    (directory / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate")
    validate.add_argument("--tag")
    pack = commands.add_parser("package")
    pack.add_argument("--target", choices=TARGETS, required=True)
    commands.add_parser("checksums")
    args = parser.parse_args()
    version = version_from_manifest()
    if args.command == "validate":
        if args.tag is not None:
            validate_tag(args.tag, version)
        output = f"version={version}\nprerelease={str('-' in version).lower()}\n"
        print(output, end="")
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
                stream.write(output)
    elif args.command == "package":
        print(package(version, args.target))
    else:
        checksums(version, Path("dist"))


if __name__ == "__main__":
    main()
