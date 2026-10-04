import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).parents[1]
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_version_and_tag_validation(self):
        for version in ["0.4.0", "1.2.3-rc.1", "1.2.3-beta"]:
            self.assertTrue(release.SEMVER.fullmatch(version))
            release.validate_tag("v" + version, version)
        for bad in ["01.2.3", "1.2", "1.2.3-01", "1.2.3;exit", "../1.2.3"]:
            self.assertFalse(release.SEMVER.fullmatch(bad))
        for tag in ["v0.5.0", "0.4.0"]:
            with self.assertRaises(ValueError):
                release.validate_tag(tag, "0.4.0")

    def test_archives_modes_checksums_and_missing_assets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ["README.md", "LICENSE", "docs/v2-design.md"]:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture", encoding="utf-8")
            for target in release.TARGETS:
                binary = "agentdrop.exe" if "windows" in target else "agentdrop"
                path = root / "target" / target / "release" / binary
                path.parent.mkdir(parents=True)
                path.write_bytes(b"test binary\x00\xff")
                archive = release.package("0.4.0", target, root, root / "dist")
                if archive.name.endswith(".zip"):
                    with zipfile.ZipFile(archive) as contents:
                        member = next(n for n in contents.namelist() if n.endswith(binary))
                        self.assertEqual(contents.read(member), path.read_bytes())
                else:
                    with tarfile.open(archive) as contents:
                        member = next(m for m in contents.getmembers() if m.name.endswith(binary))
                        self.assertEqual(member.mode, 0o755)
                        with contents.extractfile(member) as stream:
                            self.assertEqual(stream.read(), path.read_bytes())
            release.checksums("0.4.0", root / "dist")
            self.assertEqual(len((root / "dist/SHA256SUMS").read_text().splitlines()), 4)
            archive.write_bytes(b"corrupted")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                release.checksums("0.4.0", root / "dist")
            archive.unlink()
            with self.assertRaisesRegex(ValueError, "asset set mismatch"):
                release.checksums("0.4.0", root / "dist")

    @unittest.skipIf(os.name == "nt", "publish job runs on Linux")
    def test_publish_order_retries_and_failure_guards(self):
        # A CLI double validates publication sequencing without creating real releases.
        mock = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
if Path(sys.argv[0]).name == "git":
    if args[0] == "rev-parse":
        print("moved" if args[1] != "HEAD" and os.environ["CASE"] == "moved" else "commit")
    sys.exit(0)
with open("calls.jsonl", "a") as log:
    log.write(json.dumps(args) + "\\n")
if args[1] == "view":
    if os.environ["CASE"] in ["draft", "published"]:
        print("true" if os.environ["CASE"] == "draft" else "false")
    else:
        sys.exit(1)
if args[1] == "upload" and os.environ["CASE"] == "upload-fails":
    sys.exit(1)
'''
        for case in ["new", "draft", "published", "moved", "upload-fails"]:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "bin").mkdir()
                (root / "dist").mkdir()
                (root / "dist/asset.zip").write_bytes(b"archive")
                for name in ["gh", "git"]:
                    path = root / "bin" / name
                    path.write_text(mock)
                    path.chmod(0o755)
                env = dict(os.environ, PATH=str(root / "bin") + os.pathsep + os.environ["PATH"],
                           CASE=case, RELEASE_TAG="v0.4.0-rc.1", PRERELEASE="true", RUNNER_TEMP=str(root))
                result = subprocess.run(["bash", str(ROOT / "scripts/publish-release.sh")], cwd=root,
                                        env=env, capture_output=True, text=True)
                calls = [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()] if (root / "calls.jsonl").exists() else []
                operations = [call[1] for call in calls]
                if case in ["new", "draft"]:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(operations, ["view", "create", "upload", "edit"] if case == "new" else ["view", "upload", "edit"])
                    self.assertIn("--latest=false", calls[-1])
                    self.assertIn("--prerelease=true", calls[-1])
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertNotIn("edit", operations)
                    if case in ["published", "moved"]:
                        self.assertNotIn("upload", operations)


if __name__ == "__main__":
    unittest.main()
