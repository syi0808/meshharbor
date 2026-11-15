import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import quadriflow_apt_package_evidence as evidence


def dpkg_row(binary, version, architecture, source, source_version, status="installed"):
    return "\t".join(
        [binary, version, architecture, source, source_version, status]
    ) + "\n"


class AptPackageEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.docs = self.root / "usr" / "share" / "doc"
        self.docs.mkdir(parents=True)
        self.output = self.root / "evidence"
        self.rows = {
            "libboost-dev": dpkg_row(
                "libboost-dev", "1.83.0.1ubuntu2", "all", "boost-defaults", "1.83.0-2.1ubuntu3"
            ),
            "libboost1.83-dev:amd64": dpkg_row(
                "libboost1.83-dev:amd64",
                "1.83.0-2.1ubuntu3",
                "amd64",
                "boost1.83",
                "1.83.0-2.1ubuntu3",
            ),
        }
        self.notice_bytes = (
            b"Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n"
            b"License: Boost-1.0\n"
        )
        self._write_notice("libboost1.83-dev", self.notice_bytes)
        (self.docs / "libboost-dev").mkdir()
        (self.docs / "libboost-dev" / "copyright").symlink_to(
            self.docs / "libboost1.83-dev" / "copyright"
        )

    def tearDown(self):
        self.temp.cleanup()

    def _write_notice(self, package, data):
        directory = self.docs / package
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "copyright").write_bytes(data)

    def _runner(self, owner_packages=None, rows=None):
        owner_packages = {"libboost1.83-dev:amd64"} if owner_packages is None else owner_packages
        rows = rows or self.rows

        def run(command, **kwargs):
            if "--show" in command:
                requested = command[-1]
                if requested not in rows:
                    return subprocess.CompletedProcess(command, 1, "", "not installed")
                return subprocess.CompletedProcess(command, 0, rows[requested], "")
            if "-S" in command:
                path = command[-1]
                if owner_packages:
                    owners = f"{', '.join(sorted(owner_packages))}: {path}\n"
                    return subprocess.CompletedProcess(command, 0, owners, "")
                return subprocess.CompletedProcess(command, 1, "", "no path found")
            raise AssertionError(f"unexpected command: {command}")

        return run

    def test_capture_preserves_binary_source_versions_and_notice_bytes(self):
        result = evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )
        manifest = json.loads((result / "evidence.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["capture_status"], "complete_for_explicit_packages")
        self.assertFalse(manifest["dependency_closure_verified"])
        self.assertFalse(manifest["repository_origin_verified"])
        self.assertFalse(manifest["release_ready"])
        package = manifest["packages"][0]
        self.assertEqual(package["binary_version"], "1.83.0.1ubuntu2")
        self.assertEqual(package["source_package"], "boost-defaults")
        self.assertEqual(package["source_version"], "1.83.0-2.1ubuntu3")
        self.assertEqual(package["package_status"], "installed")
        notice = package["copyright_notice"]
        self.assertEqual(notice["resolved_path"], "libboost1.83-dev/copyright")
        self.assertEqual(notice["owner_packages"], ["libboost1.83-dev:amd64"])
        copied = result / notice["evidence_path"]
        self.assertEqual(copied.read_bytes(), self.notice_bytes)
        self.assertEqual(notice["sha256"], hashlib.sha256(self.notice_bytes).hexdigest())

    def test_explicit_packages_are_sorted_and_duplicate_names_rejected(self):
        rows = dict(self.rows)
        rows["libboost1.83-dev:amd64"] = self.rows["libboost1.83-dev:amd64"]
        records = self._runner(rows=rows)
        self._write_notice("libboost1.83-dev", self.notice_bytes)
        evidence.collect(
            ["libboost1.83-dev:amd64", "libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=records,
        )
        manifest = json.loads((self.output / "evidence.json").read_text(encoding="utf-8"))
        self.assertEqual(
            [package["binary_package"] for package in manifest["packages"]],
            ["libboost-dev", "libboost1.83-dev:amd64"],
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "duplicate"):
            evidence.collect(
                ["libboost-dev", "libboost-dev"],
                self.root / "duplicate",
                documentation_root=self.docs,
                runner=records,
            )

    def test_unpacked_package_is_not_reported_as_installed(self):
        rows = dict(self.rows)
        rows["libboost-dev"] = dpkg_row(
            "libboost-dev",
            "1.83.0.1ubuntu2",
            "all",
            "boost-defaults",
            "1.83.0-2.1ubuntu3",
            "unpacked",
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "not fully installed"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(rows=rows),
            )
        self.assertFalse(self.output.exists())

    def test_qualified_request_accepts_unqualified_binary_name_when_architecture_matches(self):
        rows = {
            "libboost-dev:amd64": dpkg_row(
                "libboost-dev", "1.83.0.1ubuntu2", "amd64", "boost-defaults", "1.83.0-2.1ubuntu3"
            )
        }
        evidence.collect(
            ["libboost-dev:amd64"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(rows=rows),
        )
        manifest = json.loads((self.output / "evidence.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["packages"][0]["architecture"], "amd64")

    def test_shared_copyright_owners_are_split_and_sorted(self):
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(owner_packages={"libboost1.83-dev:i386", "libboost1.83-dev:amd64"}),
        )
        manifest = json.loads((self.output / "evidence.json").read_text(encoding="utf-8"))
        self.assertEqual(
            manifest["packages"][0]["copyright_notice"]["owner_packages"],
            ["libboost1.83-dev:amd64", "libboost1.83-dev:i386"],
        )

    def test_qualified_request_checks_architecture_even_if_dpkg_name_is_unqualified(self):
        rows = {
            "libboost-dev:amd64": dpkg_row(
                "libboost-dev", "1.83.0.1ubuntu2", "all", "boost-defaults", "1.83.0-2.1ubuntu3"
            )
        }
        with self.assertRaisesRegex(evidence.EvidenceError, "requested architecture"):
            evidence.collect(
                ["libboost-dev:amd64"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(rows=rows),
            )
        self.assertFalse(self.output.exists())

    def test_invalid_debian_version_fails_closed(self):
        rows = dict(self.rows)
        rows["libboost-dev"] = dpkg_row(
            "libboost-dev", "1.83_bad", "all", "boost-defaults", "1.83.0-2"
        )
        with self.assertRaisesRegex(evidence.EvidenceError, "binary version"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(rows=rows),
            )
        self.assertFalse(self.output.exists())

    def test_missing_source_version_fails_without_success_artifact(self):
        rows = dict(self.rows)
        rows["libboost-dev"] = "libboost-dev\t1.83.0.1ubuntu2\tall\tboost-defaults\t\tinstalled\n"
        with self.assertRaisesRegex(evidence.EvidenceError, "source version"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(rows=rows),
            )
        self.assertFalse(self.output.exists())

    def test_copyright_owner_path_must_match_requested_path(self):
        def mismatched_runner(command, **kwargs):
            if "--show" in command:
                return subprocess.CompletedProcess(command, 0, self.rows["libboost-dev"], "")
            return subprocess.CompletedProcess(
                command,
                0,
                "libboost1.83-dev:amd64: /usr/share/doc/other/copyright\n",
                "",
            )
        with self.assertRaisesRegex(evidence.EvidenceError, "different copyright path"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=mismatched_runner,
            )
        self.assertFalse(self.output.exists())

    def test_missing_copyright_owner_fails_closed(self):
        with self.assertRaisesRegex(evidence.EvidenceError, "copyright owner"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(owner_packages=set()),
            )
        self.assertFalse(self.output.exists())

    def test_safe_shared_notice_path_with_spaces_is_consistent_with_schema(self):
        shared = self.docs / "Shared Files"
        shared.mkdir()
        (shared / "copyright").write_bytes(self.notice_bytes)
        (self.docs / "libboost-dev" / "copyright").unlink()
        (self.docs / "libboost-dev" / "copyright").symlink_to(shared / "copyright")
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )
        manifest = json.loads((self.output / "evidence.json").read_text(encoding="utf-8"))
        self.assertEqual(
            manifest["packages"][0]["copyright_notice"]["resolved_path"],
            "Shared Files/copyright",
        )

    def test_notice_symlink_escape_is_rejected(self):
        outside = self.root / "outside"
        outside.write_bytes(self.notice_bytes)
        (self.docs / "libboost-dev" / "copyright").unlink()
        (self.docs / "libboost-dev" / "copyright").symlink_to(outside)
        with self.assertRaisesRegex(evidence.EvidenceError, "escapes"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(),
            )
        self.assertFalse(self.output.exists())

    def test_missing_or_binary_notice_fails_closed(self):
        for contents in (b"", b"a\0b", b"\xff"):
            with self.subTest(contents=contents):
                self._write_notice("libboost1.83-dev", contents)
                with self.assertRaises(evidence.EvidenceError):
                    evidence.collect(
                        ["libboost-dev"],
                        self.output,
                        documentation_root=self.docs,
                        runner=self._runner(),
                    )
                self.assertFalse(self.output.exists())
        (self.docs / "libboost1.83-dev" / "copyright").unlink()
        with self.assertRaisesRegex(evidence.EvidenceError, "missing"):
            evidence.collect(
                ["libboost-dev"],
                self.output,
                documentation_root=self.docs,
                runner=self._runner(),
            )

    def test_unknown_package_and_overwrite_are_rejected(self):
        with self.assertRaisesRegex(evidence.EvidenceError, "safe dpkg"):
            evidence.collect(
                ["../outside"], self.output, documentation_root=self.docs, runner=self._runner()
            )
        existing = self.root / "existing"
        existing.mkdir()
        with self.assertRaisesRegex(evidence.EvidenceError, "overwrite"):
            evidence.collect(
                ["libboost-dev"], existing, documentation_root=self.docs, runner=self._runner()
            )

    def test_bundle_verifier_fails_on_unreadable_subtree(self):
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )

        def unreadable_walk(path, *, followlinks, onerror):
            onerror(PermissionError("fixture denies directory access"))
            return iter(())

        with patch.object(evidence.os, "walk", side_effect=unreadable_walk):
            with self.assertRaisesRegex(evidence.EvidenceError, "could not be scanned completely"):
                evidence.verify_bundle(self.output)

    def test_bundle_verifier_detects_modified_notice_bytes(self):
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )
        notice = self.output / "notices" / "libboost-dev" / "copyright"
        notice.write_bytes(b"X" + self.notice_bytes[1:])
        with self.assertRaisesRegex(evidence.EvidenceError, "hash mismatch"):
            evidence.verify_bundle(self.output)

    def test_bundle_verifier_checks_architecture_and_notice_encoding(self):
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )
        manifest_path = self.output / "evidence.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["packages"][0]["binary_package"] = "libboost-dev:amd64"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "architecture"):
            evidence.verify_bundle(self.output)
        manifest["packages"][0]["binary_package"] = "libboost-dev"
        notice = self.output / manifest["packages"][0]["copyright_notice"]["evidence_path"]
        invalid = b"\xff" + self.notice_bytes[1:]
        notice.write_bytes(invalid)
        manifest["packages"][0]["copyright_notice"]["bytes"] = len(invalid)
        manifest["packages"][0]["copyright_notice"]["sha256"] = hashlib.sha256(invalid).hexdigest()
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "not UTF-8"):
            evidence.verify_bundle(self.output)

    def test_bundle_verifier_rejects_unknown_fields_and_duplicate_json_keys(self):
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )
        manifest_path = self.output / "evidence.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["unreviewed"] = "value"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "unknown fields"):
            evidence.verify_bundle(self.output)
        manifest_path.write_text('{"schema_version":1,"schema_version":1}', encoding="utf-8")
        with self.assertRaisesRegex(evidence.EvidenceError, "duplicate JSON field"):
            evidence.verify_bundle(self.output)

    def test_successful_collection_remains_explicitly_non_release_ready(self):
        evidence.collect(
            ["libboost-dev"],
            self.output,
            documentation_root=self.docs,
            runner=self._runner(),
        )
        manifest = evidence.verify_bundle(self.output)
        self.assertFalse(manifest["dependency_closure_verified"])
        self.assertFalse(manifest["repository_origin_verified"])
        self.assertFalse(manifest["release_ready"])

    def test_schema_matches_fail_closed_output_contract(self):
        schema = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "schemas"
                / "quadriflow-apt-package-evidence.schema.json"
            ).read_text(encoding="utf-8")
        )
        self.assertEqual(schema["properties"]["release_ready"], {"const": False})
        self.assertEqual(schema["properties"]["dependency_closure_verified"], {"const": False})
        self.assertEqual(schema["properties"]["limitations"]["const"], evidence.LIMITATIONS)
        self.assertIn("source_version", schema["$defs"]["package"]["required"])
        self.assertEqual(
            schema["$defs"]["package"]["properties"]["package_status"],
            {"const": "installed"},
        )
        self.assertIn("sha256", schema["$defs"]["copyrightNotice"]["required"])
        self.assertFalse(schema["additionalProperties"])
        resolved_pattern = (
            schema["$defs"]["copyrightNotice"]["properties"]["resolved_path"]["pattern"]
        )
        self.assertRegex("Shared Files/copyright", resolved_pattern)
        self.assertNotRegex("../copyright", resolved_pattern)
        evidence_pattern = (
            schema["$defs"]["copyrightNotice"]["properties"]["evidence_path"]["pattern"]
        )
        self.assertRegex("notices/libboost-dev_amd64/copyright", evidence_pattern)


if __name__ == "__main__":
    unittest.main()
