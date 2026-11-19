import copy
import hashlib
from pathlib import Path
import shutil
import tempfile
import unittest

from scripts import quadriflow_macos_build_inputs as evidence


COMMIT = "a" * 40


class MacosBuildInputsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.primary = self.root / "primary"
        self.independent = self.root / "independent"
        self.binary = b"native candidate"
        self.sha = hashlib.sha256(self.binary).hexdigest()
        self.observed = {
            "executable_sha256": self.sha,
            "compile": {f"unit-{index}": {} for index in range(22)},
            "files": {"source/src/main.cpp": {"sha256": "a" * 64, "size_bytes": 20}},
            "link": {"opened": ["sdk/usr/lib/libc++.tbd"], "retained": ["build/main.o"]},
            "mach_o": {"dylibs": [
                {"install_name": "/usr/lib/libc++.1.dylib", "current_version": "1.0.0"},
                {"install_name": "/usr/lib/libSystem.B.dylib", "current_version": "2.0.0"},
            ]},
            "brew_versions": {"boost": "1.92.0_1", "eigen": "5.0.1"},
        }
        self.inputs = {
            "run_id": 10, "run_attempt": 2, "primary_artifact_id": 20, "independent_artifact_id": 21,
        }
        self.create_receipt(self.primary, "primary", self.observed)
        self.create_receipt(self.independent, "independent", self.observed)

    def create_receipt(self, directory, replica, observed):
        directory.mkdir(exist_ok=True)
        (directory / "quadriflow").write_bytes(self.binary)
        path = directory / evidence.RECEIPT
        path.unlink(missing_ok=True)
        evidence.write_json(path, {
            "schema": evidence.SCHEMA, "replica": replica, "source_commit": COMMIT,
            "observed": observed, "dependency_closure_verified": False,
        })

    def test_equal_builds_bind_both_receipts_and_hosted_invocation(self):
        result = self.primary / "macos-build-inputs-pair.json"
        evidence.compare(self.primary, self.independent, COMMIT, result, self.inputs)
        independent_copy = self.primary / "macos-independent-build-inputs.json"
        shutil.copyfile(self.independent / evidence.RECEIPT, independent_copy)
        evidence.verify_pair(self.primary, COMMIT, 10, 2)
        with self.assertRaisesRegex(ValueError, "different build invocation"):
            evidence.verify_pair(self.primary, COMMIT, 10, 3)
        independent_copy.write_bytes(independent_copy.read_bytes() + b"\n")
        with self.assertRaisesRegex(ValueError, "uploaded receipts"):
            evidence.verify_pair(self.primary, COMMIT, 10, 2)

    def test_equal_binaries_do_not_hide_a_changed_header_or_link_input(self):
        for key, change in (
            ("files", {"source/src/main.cpp": {"sha256": "b" * 64, "size_bytes": 20}}),
            ("link", {"opened": ["sdk/usr/lib/libSystem.tbd"], "retained": ["build/main.o"]}),
        ):
            with self.subTest(key=key):
                self.create_receipt(self.independent, "independent", {**self.observed, key: change})
                result = self.root / "pair.json"
                with self.assertRaisesRegex(ValueError, key):
                    evidence.compare(self.primary, self.independent, COMMIT, result)
                self.assertFalse(result.exists())

    def test_report_cannot_be_replayed_for_different_binary(self):
        (self.independent / "quadriflow").write_bytes(b"different")
        with self.assertRaisesRegex(ValueError, "executable SHA-256"):
            evidence.compare(self.primary, self.independent, COMMIT, self.root / "pair.json")

    def test_compiler_runtime_and_external_dylibs_are_checked_against_spdx(self):
        packages = {
            "quadriflow": "commit", "boost": "1.92.0_1", "eigen": "5.0.1", "lemon": "1.3.1", "pcg32": "commit",
            "apple-sdk": "sdk", "libcxx": "1.0.0", "libsystem": "2.0.0",
        }
        def relationship(kind, name):
            return {
                "spdxElementId": "SPDXRef-Package-ReviewPayload", "relationshipType": kind,
                "relatedSpdxElement": f"SPDXRef-Package-{name}",
            }

        document = {
            "spdxVersion": "SPDX-2.3",
            "files": [{"fileName": "./bin/quadriflow", "checksums": [{"algorithm": "SHA256", "checksumValue": self.sha}]}],
            "packages": [{"SPDXID": f"SPDXRef-Package-{name}", "versionInfo": version} for name, version in packages.items()],
            "relationships": [relationship("DYNAMIC_LINK", name) for name in ("libcxx", "libsystem")]
            + [relationship("DEPENDS_ON", name) for name in ("boost", "eigen", "lemon", "pcg32")],
        }
        evidence.check_sbom(self.primary, document)
        altered = copy.deepcopy(document)
        altered["files"][0]["checksums"][0]["checksumValue"] = "b" * 64
        with self.assertRaisesRegex(ValueError, "measured executable"):
            evidence.check_sbom(self.primary, altered)
        native = copy.deepcopy(self.observed)
        native["link"]["retained"].append("toolchain/lib/darwin/libclang_rt.osx.a(builtins.o)")
        self.create_receipt(self.primary, "primary", native)
        with self.assertRaisesRegex(ValueError, "missing"):
            evidence.check_sbom(self.primary, document)
        document["packages"].append({"SPDXID": "SPDXRef-Package-compiler-runtime", "versionInfo": "reviewed"})
        with self.assertRaisesRegex(ValueError, "retained compiler runtime"):
            evidence.check_sbom(self.primary, document)
        document["relationships"].append(relationship("STATIC_LINK", "compiler-runtime"))
        evidence.check_sbom(self.primary, document)
