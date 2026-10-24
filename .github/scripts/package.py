#!/usr/bin/env python3
"""Build the downloadable EdgeBridge package for one native target.

Usage: python .github/scripts/package.py --target <TARGET> --output <DIR>

Prints a JSON receipt {archive, checksum, commit, target} on success.
Any failure exits nonzero without leaving an archive or checksum behind.
"""

import argparse
import gzip
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

# target -> (archive suffix, binaries shipped at the package root)
TARGETS = {
    "x86_64-pc-windows-msvc": (
        ".zip",
        ("edgebridge-server.exe", "edgebridge-test-peer.exe"),
    ),
    "aarch64-apple-darwin": (
        ".tar.gz",
        ("edgebridge-client", "edgebridge-test-peer"),
    ),
}

EXECUTABLE_MODE = 0o755
FILE_MODE = 0o644
FIXED_MTIME = 315532800  # 1980-01-01, the zip epoch; keeps archives reproducible


class PackageError(Exception):
    pass


def _collect(repo_root: Path, target: str):
    """Return [(archive-relative path, source path, is_executable)] or raise."""
    if target not in TARGETS:
        raise PackageError(
            f"unsupported target {target!r}; expected one of {', '.join(TARGETS)}"
        )
    _, binaries = TARGETS[target]
    entries = []

    release_dir = repo_root / "target" / target / "release"
    for name in binaries:
        entries.append((name, release_dir / name, True))

    entries.append(("README.md", repo_root / "README.md", False))

    for subdir, suffix in (("config", ".toml"), ("docs", ".md")):
        found = sorted(
            p
            for p in (repo_root / subdir).glob(f"*{suffix}")
            if p.is_file() and not p.is_symlink()
        ) if (repo_root / subdir).is_dir() else []
        if not found:
            raise PackageError(f"no {suffix} files found in {repo_root / subdir}")
        entries.extend((f"{subdir}/{p.name}", p, False) for p in found)

    missing = {"config/client.example.toml", "config/server.example.toml"} - {
        name for name, _, _ in entries
    }
    if missing:
        raise PackageError(f"required example config missing: {', '.join(sorted(missing))}")

    for _, src, _ in entries:
        if not src.is_file():
            raise PackageError(f"required input missing: {src}")
    return entries


def package(repo_root, target, output_dir, commit, rustc):
    """Create edgebridge-<target>.<zip|tar.gz> and .sha256 in output_dir.

    Returns {"archive", "checksum", "commit", "target"} with absolute paths.
    Raises PackageError (before writing anything) on unsupported target or
    missing inputs.
    """
    repo_root = Path(repo_root)
    output_dir = Path(output_dir)
    entries = _collect(repo_root, target)
    if not commit or not rustc:
        raise PackageError("commit and rustc metadata are required")

    suffix, _ = TARGETS[target]
    root_name = f"edgebridge-{target}"
    archive_path = output_dir / f"{root_name}{suffix}"
    checksum_path = output_dir / f"{root_name}.sha256"

    build_info = json.dumps(
        {"commit": commit, "target": target, "rustc": rustc}, indent=2
    ).encode() + b"\n"

    members = [(f"{root_name}/build-info.json", build_info, False)]
    for rel, src, is_exec in entries:
        members.append((f"{root_name}/{rel}", src.read_bytes(), is_exec))
    members.sort(key=lambda m: m[0])

    output_dir.mkdir(parents=True, exist_ok=True)
    tmp_archive = None
    tmp_checksum = None
    try:
        fd, tmp_archive = tempfile.mkstemp(dir=output_dir, prefix=".pkg-")
        os.close(fd)
        if suffix == ".zip":
            _write_zip(tmp_archive, root_name, members)
        else:
            _write_targz(tmp_archive, root_name, members)

        digest = hashlib.sha256(Path(tmp_archive).read_bytes()).hexdigest()
        fd, tmp_checksum = tempfile.mkstemp(dir=output_dir, prefix=".sum-")
        with os.fdopen(fd, "w", newline="\n") as f:
            f.write(f"{digest}  {archive_path.name}\n")

        os.replace(tmp_archive, archive_path)
        tmp_archive = None
        os.replace(tmp_checksum, checksum_path)
        tmp_checksum = None
    except BaseException:
        archive_path.unlink(missing_ok=True)
        checksum_path.unlink(missing_ok=True)
        raise
    finally:
        for tmp in (tmp_archive, tmp_checksum):
            if tmp:
                Path(tmp).unlink(missing_ok=True)

    return {
        "archive": str(archive_path.resolve()),
        "checksum": str(checksum_path.resolve()),
        "commit": commit,
        "target": target,
    }


def _write_zip(path, root_name, members):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, data, is_exec in members:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            mode = EXECUTABLE_MODE if is_exec else FILE_MODE
            info.external_attr = (0o100000 | mode) << 16
            zf.writestr(info, data)


def _write_targz(path, root_name, members):
    with open(path, "wb") as raw:
        with gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as gz:
            with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tf:
                dirs = {root_name}
                for name, _, _ in members:
                    parts = name.split("/")
                    for i in range(1, len(parts)):
                        dirs.add("/".join(parts[:i]))
                for d in sorted(dirs):
                    info = tarfile.TarInfo(d)
                    info.type = tarfile.DIRTYPE
                    info.mode = 0o755
                    info.mtime = FIXED_MTIME
                    tf.addfile(info)
                for name, data, is_exec in members:
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    info.mode = EXECUTABLE_MODE if is_exec else FILE_MODE
                    info.mtime = FIXED_MTIME
                    tf.addfile(info, io.BytesIO(data))


def _run(repo_root: Path, *args: str) -> str:
    result = subprocess.run(
        list(args), cwd=repo_root, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)

    repo_root = Path(__file__).resolve().parents[2]
    try:
        if args.target not in TARGETS:
            raise PackageError(
                f"unsupported target {args.target!r}; expected one of {', '.join(TARGETS)}"
            )
        commit = _run(repo_root, "git", "rev-parse", "HEAD")
        rustc = _run(repo_root, "rustc", "--version")
        receipt = package(repo_root, args.target, args.output, commit, rustc)
    except (PackageError, subprocess.CalledProcessError, OSError) as exc:
        print(f"package.py: error: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(receipt))
    return 0


if __name__ == "__main__":
    sys.exit(main())
