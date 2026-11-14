import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import quadriflow_notices as q


class NoticeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "LICENSE.txt").write_text("Example upstream notice\n", encoding="utf-8")
        self.inventory = {
            "source_commit": q.PIN,
            "packages": [
                {
                    "id": "quadriflow",
                    "name": "QuadriFlow",
                    "version": q.PIN,
                    "manager": "source",
                    "root": str(self.source),
                    "notices": ["LICENSE.txt"],
                }
            ],
        }

    def tearDown(self):
        self.temp.cleanup()

    def test_repeatable_bytes_across_outputs(self):
        for name in ("a", "b"):
            report = q.build(self.inventory, self.root / name, 0)
            self.assertFalse(report["release_ready"])
            self.assertFalse(report["dependency_closure_verified"])
        a = {
            p.relative_to(self.root / "a").as_posix(): p.read_bytes()
            for p in (self.root / "a").rglob("*")
            if p.is_file()
        }
        b = {
            p.relative_to(self.root / "b").as_posix(): p.read_bytes()
            for p in (self.root / "b").rglob("*")
            if p.is_file()
        }
        self.assertEqual(a, b)
        self.assertEqual(a["third-party-notices/quadriflow/LICENSE.txt"], b"Example upstream notice\n")
        self.assertIn(b"third-party-notices/quadriflow/", a["THIRD-PARTY-NOTICES.md"])
        sbom = json.loads(a["sbom.spdx.json"])
        self.assertEqual(sbom["spdxVersion"], "SPDX-2.3")
        self.assertEqual(sbom["creationInfo"]["created"], "1970-01-01T00:00:00Z")
        self.assertEqual(sbom["files"][0]["licenseInfoInFile"], ["NOASSERTION"])
        for line in a["SHA256SUMS"].decode().splitlines():
            digest, name = line.split("  ", 1)
            self.assertEqual(digest, hashlib.sha256(a[name]).hexdigest())

    def test_existing_output_preserved(self):
        out = self.root / "out"
        out.mkdir()
        (out / "keep").write_text("keep", encoding="utf-8")
        with self.assertRaises(FileExistsError):
            q.build(self.inventory, out, 0)
        self.assertEqual((out / "keep").read_text(encoding="utf-8"), "keep")

    def test_missing_notice_fails_before_output(self):
        self.inventory["packages"][0]["notices"] = ["MISSING"]
        with self.assertRaises(FileNotFoundError):
            q.build(self.inventory, self.root / "out", 0)
        self.assertFalse((self.root / "out").exists())

    def test_traversal_and_control_rejected(self):
        for rel in ("../LICENSE.txt", "/LICENSE.txt", "x/../LICENSE.txt", "a\\b", "a\nb", "a//b"):
            with self.subTest(rel=rel), self.assertRaises(ValueError):
                q.safe_file(self.source, rel)

    def test_symlink_notice_rejected(self):
        (self.source / "alias").symlink_to(self.source / "LICENSE.txt")
        with self.assertRaises(ValueError):
            q.read_notice(self.source, "alias")

    def test_symlink_ancestor_rejected(self):
        (self.source / "dir").symlink_to(self.source, target_is_directory=True)
        with self.assertRaises(ValueError):
            q.read_notice(self.source, "dir/LICENSE.txt")

    def test_limits(self):
        with patch.object(q, "MAX_FILE_BYTES", 1), self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "one", 0)
        with patch.object(q, "MAX_TOTAL_BYTES", 1), self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "two", 0)
        with patch.object(q, "MAX_FILES", 0), self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "three", 0)

    def test_wrong_revision(self):
        self.inventory["source_commit"] = "0" * 40
        with self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "out", 0)

    def test_duplicate_package_and_notice(self):
        self.inventory["packages"] *= 2
        with self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "out", 0)
        self.inventory["packages"] = self.inventory["packages"][:1]
        self.inventory["packages"][0]["notices"] *= 2
        with self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "out", 0)

    def test_binary_and_empty_notices(self):
        for value in (b"", b"a\0b", b"\xff"):
            (self.source / "LICENSE.txt").write_bytes(value)
            with self.assertRaises((ValueError, UnicodeDecodeError)):
                q.build(self.inventory, self.root / "out", 0)

    def test_manager_path_contracts(self):
        for manager, relative in [
            ("vcpkg", "share/eigen3/copyright"),
            ("apt", "libeigen3-dev/copyright"),
        ]:
            (self.root / relative).parent.mkdir(parents=True)
            (self.root / relative).write_text("notice", encoding="utf-8")
            package = "eigen3" if manager == "vcpkg" else "libeigen3-dev"
            self.assertEqual(q.discover(str(self.root), manager, package), [relative])
        self.assertEqual(q.discover(str(self.source), "homebrew"), ["LICENSE.txt"])

    def test_portable_paths(self):
        for rel in ("CON", "a/NUL.txt", "a?.txt", "CON .txt", "COPYING.", "NOTICE "):
            with self.subTest(rel=rel), self.assertRaises(ValueError):
                q.safe_file(self.source, rel)

    def test_invalid_epoch(self):
        for epoch in (-1, True, "0", 4_102_444_801):
            with self.subTest(epoch=epoch), self.assertRaises(ValueError):
                q.build(self.inventory, self.root / "out", epoch)

    def test_unresolved_license_ref(self):
        self.inventory["packages"][0]["license_declared"] = "LicenseRef-Unresolved"
        with self.assertRaises(ValueError):
            q.build(self.inventory, self.root / "out", 0)

    def test_bounded_discovery(self):
        with patch.object(q, "MAX_SCAN_ENTRIES", 0), self.assertRaises(ValueError):
            q.discover(str(self.source), "source")

    def test_malformed_inventory_records_fail_closed(self):
        for bad_inventory in (
            [],
            {"source_commit": q.PIN, "packages": [None]},
            {"source_commit": q.PIN, "packages": [{"name": "missing id"}]},
            {"source_commit": q.PIN, "packages": [{**self.inventory["packages"][0], "notices": [None]}]},
        ):
            with self.subTest(inventory=bad_inventory), self.assertRaises(ValueError):
                q.build(bad_inventory, self.root / "out", 0)

    def test_noncanonical_output_parent_rejected(self):
        alias = self.root / "alias"
        alias.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            q.build(self.inventory, alias / "out", 0)

    def test_unknown_inventory_fields_rejected(self):
        inventory = dict(self.inventory, extra="ignored input")
        with self.assertRaises(ValueError):
            q.build(inventory, self.root / "out", 0)
        package = dict(self.inventory["packages"][0], extra="unreviewed package input")
        with self.assertRaises(ValueError):
            q.build({"source_commit": q.PIN, "packages": [package]}, self.root / "other", 0)

    def test_duplicate_json_fields_rejected(self):
        with self.assertRaises(ValueError):
            q.strict_json_object([("source_commit", q.PIN), ("source_commit", q.PIN)])


class ReleaseManifestSchemaTests(unittest.TestCase):
    def test_archive_schema_matches_runtime_ustar_contract(self):
        repository_root = Path(__file__).resolve().parents[1]
        schema = json.loads(
            (repository_root / "schemas" / "quadriflow-release-manifest.schema.json").read_text(
                encoding="utf-8"
            )
        )
        archive = schema["$defs"]["archive"]["properties"]
        self.assertCountEqual(
            archive["filename"]["enum"],
            [
                "quadriflow-linux-x86_64.tar",
                "quadriflow-macos-aarch64.tar",
                "quadriflow-windows-x86_64.tar",
            ],
        )
        self.assertEqual(archive["format"], {"const": "tar"})
        self.assertTrue(archive["url"]["pattern"].endswith(r"\.tar$"))
        self.assertEqual(archive["size_bytes"]["maximum"], 128 * 1024 * 1024)

        runtime_source = (
            repository_root / "crates" / "core" / "src" / "quadriflow_manifest.rs"
        ).read_text(encoding="utf-8")
        for filename in archive["filename"]["enum"]:
            self.assertIn(filename, runtime_source)
        self.assertIn("pub enum QuadriFlowArchiveFormat", runtime_source)
        self.assertIn("    Tar,", runtime_source)


if __name__ == "__main__":
    unittest.main()
