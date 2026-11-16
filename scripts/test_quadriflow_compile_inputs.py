import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import quadriflow_compile_inputs as q


class CompileInputTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name).resolve()
        self.build = root / "build"
        self.source = root / "source"
        self.package = self.source / "nested-package"
        self.build.mkdir()
        self.package.mkdir(parents=True)
        self.roots = [("src", self.source), ("vendor-package", self.package)]
        self.unit = self.source / "main.cpp"
        self.header = self.package / "input #$.hpp"
        self.unit.write_bytes(b"int answer = 42;\n")
        self.header.write_bytes(b"// test header\r\n")
        (self.build / "main.o").touch()
        self.record = self.build / "main.o.d"
        self.record.write_text(
            "main.o: ../source/main.cpp \\\n  ../source/nested-package/input\\ \\#$$.hpp\n",
            encoding="utf-8",
        )

    def collect(self):
        return q.collect(self.build, self.roots, [self.record], 1)

    def test_hashes_inputs_under_most_specific_root_without_absolute_paths(self):
        doc = self.collect()
        self.assertEqual(
            doc["objects"],
            {"main.o": {"inputs": ["src/main.cpp", "vendor-package/input #$.hpp"],
                        "first_prerequisite": "src/main.cpp"}},
        )
        self.assertEqual(doc["inputs"]["vendor-package/input #$.hpp"], {
            "sha256": hashlib.sha256(self.header.read_bytes()).hexdigest(),
            "size_bytes": self.header.stat().st_size,
        })
        self.assertNotIn(str(self.build), json.dumps(doc))
        self.assertTrue(doc["diagnostic_only"])
        for key in (
            "dependency_closure_verified", "link_inputs_collected",
            "runtime_inputs_collected", "installable", "release_ready",
        ):
            self.assertFalse(doc[key])

    def test_regular_records_order_independent_and_hash_shared_inputs_once(self):
        (self.build / "other.o").touch()
        second = self.build / "other.o.d"
        second.write_text("other.o: ../source/main.cpp\n")
        a = q.collect(self.build, self.roots, [self.record, second], 2)
        b = q.collect(self.build, self.roots[::-1], [second, self.record], 2)
        self.assertEqual(json.dumps(a, sort_keys=True), json.dumps(b, sort_keys=True))
        self.assertEqual(len(a["inputs"]), 2)
        self.assertEqual(len(a["objects"]), 2)

    def test_top_level_object_and_subdirectory_prerequisite_have_separate_bases(self):
        subdir = self.build / "subdir"
        subdir.mkdir()
        (subdir / "main.o").touch()
        (subdir / "generated.h").write_bytes(b"right file")
        (self.build / "generated.h").write_bytes(b"decoy file")
        self.record.write_text("subdir/main.o: generated.h\n")
        report = q.collect(
            self.build, self.roots, [self.record], 1, dependency_base=subdir,
        )
        self.assertEqual(report["objects"]["subdir/main.o"]["inputs"], ["build/subdir/generated.h"])
        self.assertEqual(report["inputs"]["build/subdir/generated.h"]["sha256"],
                         hashlib.sha256(b"right file").hexdigest())
        with self.assertRaisesRegex(ValueError, "outside all declared roots"):
            q.collect(
                self.build, self.roots, [self.record], 1, dependency_base=self.build.parent,
            )

    def test_duplicate_output_and_wrong_expected_count_fail(self):
        with self.assertRaisesRegex(ValueError, "unit count"):
            q.collect(self.build, self.roots, [self.record], 2)
        with self.assertRaisesRegex(ValueError, "multiple records"):
            q.collect(self.build, self.roots, [self.record, self.record], 2)

    def test_unclaimed_missing_and_root_crossing_symlink_inputs_fail(self):
        outside = self.build.parent / "outside.h"
        outside.touch()
        self.record.write_text("main.o: ../outside.h\n")
        with self.assertRaisesRegex(ValueError, "outside all declared roots"):
            self.collect()
        self.record.write_text("main.o: ../source/notfound.h\n")
        with self.assertRaises(FileNotFoundError):
            self.collect()
        alias = self.source / "alias.hpp"
        alias.symlink_to(self.header)
        self.record.write_text("main.o: ../source/alias.hpp\n")
        with self.assertRaisesRegex(ValueError, "crosses a declared root"):
            self.collect()
        alias.unlink()
        alias.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "outside all declared roots"):
            self.collect()

    def test_resolves_original_path_when_symlink_precedes_parent_segment(self):
        actual_dir = self.source / "actual"
        (actual_dir / "child").mkdir(parents=True)
        (self.source / "alias").symlink_to(actual_dir / "child")
        (self.source / "picked.h").write_bytes(b"not the compiler input")
        used = actual_dir / "picked.h"
        used.write_bytes(b"compiler input")
        self.record.write_text("main.o: ../source/alias/../picked.h\n")
        report = self.collect()
        self.assertNotIn("src/picked.h", report["inputs"])
        self.assertEqual(
            report["inputs"]["src/actual/picked.h"]["sha256"],
            hashlib.sha256(used.read_bytes()).hexdigest(),
        )
        outside = self.build.parent / "outside"
        (outside / "child").mkdir(parents=True)
        (outside / "picked.h").write_bytes(b"must not be collected")
        (self.source / "alias").unlink()
        (self.source / "alias").symlink_to(outside / "child")
        with self.assertRaisesRegex(ValueError, "outside all declared roots"):
            self.collect()

    def test_invalid_roots_and_non_object_targets_fail(self):
        for roots in ([("src", self.source), ("src", self.package)],
                      [("src", self.source), ("same-path", self.source)],
                      [("build", self.source)], [("bad/path", self.source)]):
            with self.subTest(roots=roots), self.assertRaises(ValueError):
                q.collect(self.build, roots, [self.record], 1)
        self.record.write_text("main.o.d: ../source/main.cpp\n")
        with self.assertRaisesRegex(ValueError, "POSIX build-root"):
            self.collect()

    def test_parser_accepts_crlf_quotes_and_rejects_make_language(self):
        self.assertEqual(q.parse_depfile(b"ob\\ j.o: ../he\\:ader.h \\\r\n\t a\\#b c$$d\r\n"),
                         ("ob j.o", ["../he:ader.h", "a#b", "c$d"]))
        for record in (
            b"", b"a.o:", b"a.o b.o: a.cpp", b"a.o: a.cpp\na.cpp:",
            b"a.o: ${SRC}/a.cpp", b"a.o: a.cpp # ignored.h",
            b"a.o: a\\z", b"a.o: a\x00.cpp", b"C:\\file.o: C:\\file.cpp",
        ):
            with self.subTest(record=record), self.assertRaises(ValueError):
                q.parse_depfile(record)

    def test_limits_and_symlink_depfile(self):
        for knob in ("MAX_FILE_BYTES", "MAX_DEPFILE_BYTES", "MAX_DEPFILE_WORDS",
                     "MAX_FILES", "MAX_REFERENCES", "MAX_REPORT_PATH_BYTES", "MAX_HASH_BYTES"):
            with self.subTest(knob=knob), patch.object(q, knob, 1), self.assertRaises(ValueError):
                self.collect()
        other = self.build / "alias.o.d"
        other.symlink_to(self.record)
        with self.assertRaisesRegex(ValueError, "not a regular"):
            q.collect(self.build, self.roots, [other], 1)

    def test_cli_rejects_missing_path_without_printing_host_path_or_partial_report(self):
        self.record.write_text("main.o: ../source/my-private-path.hpp\n")
        with contextlib.redirect_stdout(io.StringIO()) as stdout, \
                contextlib.redirect_stderr(io.StringIO()) as stderr, self.assertRaises(SystemExit):
            q.main(["--build-root", str(self.build), "--root", f"src={self.source}",
                    "--depfile", str(self.record), "--expected-units", "1"])
        self.assertEqual(stdout.getvalue(), "")
        self.assertNotIn(str(self.build.parent), stderr.getvalue())
        self.assertNotIn("my-private-path", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
