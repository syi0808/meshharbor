import copy
import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

import quadriflow_candidate_materials as q


def receipt(replica="primary"):
    entries = [
        dict(id="meshharbor-source", kind="source", algorithm="git-sha1", digest="a" * 40),
        dict(id="quadriflow-source", kind="source", algorithm="git-sha1", digest="b" * 40),
        dict(id="build-recipe", kind="recipe", algorithm="sha256", digest="c" * 64),
        dict(id="compiler-deps", kind="compiler", algorithm="sha256", digest="d" * 64),
        dict(id="boost", kind="package", algorithm="sha512", digest="e" * 128, version="1.92.0"),
    ]
    return dict(
        schema_version=1, platform="linux-x86_64", materials=entries,
        coverage=dict(compiler="observed", linker="not-collected", runtime="partial",
                      package="observed", toolchain="not-collected"),
        observations=dict(replica=replica, run_id=42, run_attempt=1, runner_image="20261116.7"),
    )


class MaterialsReceiptTests(unittest.TestCase):
    def test_sorted_materials_and_host_observations_do_not_change_semantic_identity(self):
        primary, independent = receipt(), receipt("independent")
        independent["materials"].reverse()
        independent["observations"]["runner_image"] = "20261116.9"
        a, b = q.normalize(primary), q.normalize(independent)
        self.assertNotEqual(a["observations"], b["observations"])
        self.assertEqual(q.canonical(a["semantic"]), q.canonical(b["semantic"]))
        self.assertEqual(a["semantic_sha256"], hashlib.sha256(q.canonical(a["semantic"])).hexdigest())
        self.assertTrue(q.compare(primary, independent)["semantic_match"])

    def test_version_digest_and_coverage_disagreement_are_distinct_from_image_drift(self):
        for key, update in [("version", "1.92.1"), ("digest", "f" * 128)]:
            with self.subTest(key=key):
                replica = receipt("independent")
                replica["materials"][-1][key] = update
                result = q.compare(receipt(), replica)
                self.assertFalse(result["semantic_match"])
                self.assertNotEqual(result["primary_semantic_sha256"],
                                    result["independent_semantic_sha256"])
        replica = receipt("independent")
        replica["coverage"]["runtime"] = "not-collected"
        result = q.compare(receipt(), replica)
        self.assertTrue(result["material_pins_match"])
        self.assertFalse(result["coverage_matches"])
        self.assertFalse(result["semantic_match"])

    def test_partial_or_all_observed_never_qualifies_for_release(self):
        primary = receipt()
        for kind in ["linker", "runtime", "toolchain"]:
            entry = dict(id=kind + "-record", kind=kind, algorithm="sha256", digest="8" * 64)
            if kind == "toolchain":
                entry["version"] = "14.2.0"
            primary["materials"].append(entry)
            primary["coverage"][kind] = "observed"
        for result in (q.normalize(receipt()), q.normalize(primary),
                       q.compare(receipt(), receipt("independent"))):
            self.assertTrue(result["diagnostic_only"])
            self.assertFalse(result["dependency_closure_verified"])
            self.assertFalse(result["release_eligible"])

    def test_bad_schema_materials_missing_coverage_and_private_fields(self):
        def invalid(change):
            value = receipt()
            change(value)
            with self.assertRaises(ValueError):
                q.normalize(value)

        for change in [
            lambda a: a.update(schema_version=True),
            lambda a: a.update(platform={}),
            lambda a: a.update(release_eligible=True),
            lambda a: a["materials"].pop(0),
            lambda a: a["materials"].append(copy.deepcopy(a["materials"][0])),
            lambda a: a["materials"][0].update(algorithm="sha256", digest="1" * 64),
            lambda a: a["materials"][-1].update(digest="F" * 128),
            lambda a: a["materials"][-1].update(id="../boost"),
            lambda a: a["materials"][-1].pop("version"),
            lambda a: a["materials"][-1].update(version="/Users/private"),
            lambda a: a["materials"][-1].update(source_path="/Users/private"),
            lambda a: a["coverage"].update(linker="observed"),
            lambda a: a["coverage"].update(compiler="unsupported"),
            lambda a: a["coverage"].update(runtime="complete"),
            lambda a: a["observations"].update(runner_image="/Users/private"),
            lambda a: a["observations"].update(run_id=True),
        ]:
            invalid(change)

    def test_comparison_requires_distinct_roles_same_attempt_same_platform(self):
        for key, invalid in [("replica", "primary"), ("run_id", 41), ("run_attempt", 2)]:
            independent = receipt("independent")
            independent["observations"][key] = invalid
            with self.assertRaises(ValueError):
                q.compare(receipt(), independent)
        independent = receipt("independent")
        independent["platform"] = "windows-x86_64"
        with self.assertRaisesRegex(ValueError, "same platform"):
            q.compare(receipt(), independent)

    def test_bounded_reader_rejects_repeated_json_fields_and_overflow(self):
        with TemporaryDirectory() as temp:
            path = Path(temp) / "receipt.json"
            path.write_text(json.dumps(receipt()), encoding="utf8")
            self.assertEqual(q.read_input(path), receipt())
            with patch.object(q, "MAX_INPUT_BYTES", 1), self.assertRaisesRegex(ValueError, "size limit"):
                q.read_input(path)
            path.write_text('{"a":{"digest":"a","digest":"b"}}', encoding="utf8")
            with self.assertRaisesRegex(ValueError, "duplicate JSON"):
                q.read_input(path)


if __name__ == "__main__":
    unittest.main()
