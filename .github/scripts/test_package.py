import hashlib
import json
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

import package

WIN = "x86_64-pc-windows-msvc"
MAC = "aarch64-apple-darwin"
ALL_BINARIES = {
    "edgebridge-server.exe",
    "edgebridge-test-peer.exe",
    "edgebridge-client",
    "edgebridge-test-peer",
}


def make_repo(root: Path, targets=(WIN, MAC)):
    (root / "README.md").write_text("readme")
    (root / "config").mkdir()
    (root / "config" / "client.example.toml").write_text("a = 1\n")
    (root / "config" / "server.example.toml").write_text("b = 2\n")
    (root / "docs").mkdir()
    (root / "docs" / "PROTOCOL.md").write_text("# p\n")
    (root / "docs" / "notes.txt").write_text("not shipped")
    (root / "src").mkdir()
    (root / "src" / "main.rs").write_text("fn main(){}")
    (root / "Cargo.toml").write_text("[workspace]")
    for t in targets:
        rel = root / "target" / t / "release"
        rel.mkdir(parents=True)
        (rel / "build-script-junk.d").write_text("junk")
        (rel / "deps").mkdir()
        (rel / "deps" / "x.rlib").write_text("junk")
        for name in ALL_BINARIES:
            # every binary exists in every release dir so wrong selection is detectable
            (rel / name).write_bytes(f"{t}:{name}".encode())


def read_archive(path: Path):
    """Return {name: (bytes, mode)} for files in the archive."""
    out = {}
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as zf:
            for i in zf.infolist():
                if not i.is_dir():
                    out[i.filename] = (zf.read(i), (i.external_attr >> 16) & 0o777)
    else:
        with tarfile.open(path, "r:gz") as tf:
            for m in tf.getmembers():
                if m.isfile():
                    out[m.name] = (tf.extractfile(m).read(), m.mode & 0o777)
    return out


class PackageTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        base = Path(self._tmp.name)
        self.repo = base / "repo"
        self.repo.mkdir()
        self.out = base / "out"
        make_repo(self.repo)

    def build(self, target):
        return package.package(self.repo, target, self.out, "abc123", "rustc 1.90.0")

    def test_windows_package_contents(self):
        receipt = self.build(WIN)
        archive = Path(receipt["archive"])
        self.assertEqual(archive.name, f"edgebridge-{WIN}.zip")
        files = read_archive(archive)
        root = f"edgebridge-{WIN}/"
        self.assertEqual(
            set(files),
            {
                root + "edgebridge-server.exe",
                root + "edgebridge-test-peer.exe",
                root + "README.md",
                root + "build-info.json",
                root + "config/client.example.toml",
                root + "config/server.example.toml",
                root + "docs/PROTOCOL.md",
            },
        )
        # bytes come from the Windows release dir, not the mac one
        self.assertEqual(
            files[root + "edgebridge-server.exe"][0],
            f"{WIN}:edgebridge-server.exe".encode(),
        )
        self.assertEqual(
            files[root + "edgebridge-test-peer.exe"][0],
            f"{WIN}:edgebridge-test-peer.exe".encode(),
        )

    def test_mac_package_contents_and_permissions(self):
        receipt = self.build(MAC)
        archive = Path(receipt["archive"])
        self.assertEqual(archive.name, f"edgebridge-{MAC}.tar.gz")
        files = read_archive(archive)
        root = f"edgebridge-{MAC}/"
        self.assertEqual(
            set(files),
            {
                root + "edgebridge-client",
                root + "edgebridge-test-peer",
                root + "README.md",
                root + "build-info.json",
                root + "config/client.example.toml",
                root + "config/server.example.toml",
                root + "docs/PROTOCOL.md",
            },
        )
        for name in ("edgebridge-client", "edgebridge-test-peer"):
            data, mode = files[root + name]
            self.assertEqual(data, f"{MAC}:{name}".encode())
            self.assertEqual(mode & 0o111, 0o111, name)
        for name in ("README.md", "build-info.json", "docs/PROTOCOL.md"):
            self.assertEqual(files[root + name][1] & 0o444, 0o444, name)
            self.assertEqual(files[root + name][1] & 0o111, 0, name)

    def test_single_root_folder(self):
        for target in (WIN, MAC):
            receipt = self.build(target)
            roots = {n.split("/")[0] for n in read_archive(Path(receipt["archive"]))}
            self.assertEqual(roots, {f"edgebridge-{target}"})

    def test_checksum_matches_archive_bytes(self):
        for target in (WIN, MAC):
            receipt = self.build(target)
            archive = Path(receipt["archive"])
            expected = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertEqual(
                Path(receipt["checksum"]).read_text(),
                f"{expected}  {archive.name}\n",
            )
            self.assertEqual(Path(receipt["checksum"]).name, f"edgebridge-{target}.sha256")

    def test_build_info_and_receipt(self):
        for target in (WIN, MAC):
            receipt = self.build(target)
            self.assertEqual(receipt["commit"], "abc123")
            self.assertEqual(receipt["target"], target)
            files = read_archive(Path(receipt["archive"]))
            info = json.loads(files[f"edgebridge-{target}/build-info.json"][0])
            self.assertEqual(info["commit"], "abc123")
            self.assertEqual(info["target"], target)
            self.assertEqual(info["rustc"], "rustc 1.90.0")

    def test_excludes_other_files(self):
        for target in (WIN, MAC):
            names = read_archive(Path(self.build(target)["archive"]))
            joined = "\n".join(names)
            self.assertNotIn("main.rs", joined)
            self.assertNotIn("Cargo.toml", joined)
            self.assertNotIn("deps", joined)
            self.assertNotIn("notes.txt", joined)

    def test_unsupported_target_fails_without_output(self):
        with self.assertRaises(package.PackageError):
            package.package(self.repo, "x86_64-unknown-linux-gnu", self.out, "c", "r")
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_missing_binary_leaves_no_artifacts(self):
        (self.repo / "target" / MAC / "release" / "edgebridge-client").unlink()
        with self.assertRaises(package.PackageError):
            self.build(MAC)
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_missing_required_files_leave_no_artifacts(self):
        (self.repo / "README.md").unlink()
        with self.assertRaises(package.PackageError):
            self.build(WIN)
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_missing_config_or_docs_fails(self):
        for p in (self.repo / "config").glob("*.toml"):
            p.unlink()
        with self.assertRaises(package.PackageError):
            self.build(WIN)
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_missing_one_example_config_fails(self):
        (self.repo / "config" / "client.example.toml").unlink()
        with self.assertRaises(package.PackageError):
            self.build(MAC)
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_missing_release_dir_for_target_fails(self):
        import shutil

        shutil.rmtree(self.repo / "target" / WIN)
        with self.assertRaises(package.PackageError):
            self.build(WIN)
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))

    def test_cli_rejects_unsupported_target(self):
        self.assertNotEqual(
            package.main(["--target", "bogus", "--output", str(self.out)]), 0
        )
        self.assertFalse(self.out.exists() and any(self.out.iterdir()))


if __name__ == "__main__":
    unittest.main()
