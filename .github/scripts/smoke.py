"""Verify a delivered archive and run native CLI paths without touching input devices."""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import stat
import subprocess
import tarfile
import tempfile
import zipfile


from package import TARGETS


def run(binary, args, expected, directory):
    command = [str(binary), *args]
    result = subprocess.run(command, cwd=directory, capture_output=True, text=True, timeout=15)
    print(json.dumps({"command": command, "status": result.returncode,
                      "stdout": result.stdout, "stderr": result.stderr}), flush=True)
    if result.returncode != expected:
        raise RuntimeError(f"expected exit {expected}, got {result.returncode}: {command}")


def smoke(target, archive):
    suffix, (program, peer) = TARGETS[target]
    name = f"edgebridge-{target}"
    if archive.name != name + suffix:
        raise ValueError(f"unexpected archive name: {archive.name}")
    checksum = archive.with_name(name + ".sha256").read_text(encoding="utf-8").split()
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if checksum != [digest, archive.name]:
        raise ValueError("archive SHA256 does not match the checksum file")
    root = Path(__file__).resolve().parents[2]
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root,
                            check=True, capture_output=True, text=True).stdout.strip()
    with tempfile.TemporaryDirectory(prefix="edgebridge-package-smoke-") as temporary:
        unpacked = Path(temporary)
        if suffix == ".zip":
            with zipfile.ZipFile(archive) as source:
                for entry in source.namelist():
                    path = PurePosixPath(entry)
                    if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != name:
                        raise ValueError(f"unexpected archive path: {entry}")
                source.extractall(unpacked)
        else:
            with tarfile.open(archive, "r:gz") as source:
                for entry in source.getmembers():
                    path = PurePosixPath(entry.name)
                    if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != name:
                        raise ValueError(f"unexpected archive path: {entry.name}")
                source.extractall(unpacked, filter="data")
        package = unpacked / name
        metadata = json.loads((package / "build-info.json").read_text(encoding="utf-8"))
        if metadata["target"] != target or metadata["commit"] != commit:
            raise ValueError(f"package target/commit mismatch: {metadata}")
        for filename in (program, peer):
            binary = package / filename
            if not binary.is_file():
                raise ValueError(f"missing binary: {filename}")
            if suffix == ".tar.gz" and not binary.stat().st_mode & stat.S_IXUSR:
                raise ValueError(f"executable mode was lost: {filename}")
        if target == "x86_64-pc-windows-msvc":
            run(package / program, ["--help"], 0, package)
            run(package / program, ["--check-config", "--config", "config/server.example.toml"], 0, package)
            run(package / program, ["--check-config", "--config", "config/client.example.toml"], 78, package)
            run(package / program, ["--capture-log", "--duration-secs", "0"], 64, package)
        else:
            # Both paths exit before permission APIs, network connection or Quartz injection.
            run(package / program, [], 64, package)
            run(package / program, ["--config", "config/server.example.toml"], 78, package)
        run(package / peer, [], 64, package)
    print(json.dumps({"pass": True, "target": target, "commit": commit,
                      "sha256": digest, "temporary_directory_removed": not unpacked.exists()}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--archive", required=True, type=Path)
    args = parser.parse_args()
    smoke(args.target, args.archive.resolve())


if __name__ == "__main__":
    main()
