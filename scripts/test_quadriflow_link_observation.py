from __future__ import annotations

import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest

from quadriflow_link_observation import collect, ident, trace_records


class GNUTraceFormat(unittest.TestCase):
    def test_archive_open_and_selection_are_distinct(self):
        entries = list(trace_records("first.o\ncomponents/libchosen.a\n(components/libchosen.a)chosen.o\ncomponents/libidle.a\n"))
        self.assertEqual(entries, [
            ("first.o", None), ("components/libchosen.a", None),
            ("components/libchosen.a", "chosen.o"), ("components/libidle.a", None),
        ])

    def test_malformed_or_unsupported_trace(self):
        for record in ("", "file.o", "(lib.a)../escape.o\n", "(lib.a)/abs.o\n",
                       "lib.a(other.o)\n", "bad\x00name\n", "warning: extra output\n", "\n"):
            with self.subTest(record=record), self.assertRaises(ValueError):
                list(trace_records(record))

    @unittest.skipUnless(os.name == "posix", "requires POSIX symbolic links")
    def test_root_escape_is_independent_of_file_existence(self):
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary).resolve()
            build = parent / "build"
            host = parent / "host"
            build.mkdir()
            host.mkdir()
            (host / "same.o").write_bytes(b"other")
            (build / "same.o").write_bytes(b"inside")
            (build / "symlink.o").symlink_to(host / "same.o")
            roots = [("build", build), ("host", host)]
            self.assertEqual(ident("same.o", build, roots)[:2], ("build/same.o", "build/same.o"))
            self.assertEqual(ident(str(host / "same.o"), build, roots)[1], "host/same.o")
            with self.assertRaisesRegex(ValueError, "crosses declared roots"):
                ident("symlink.o", build, roots)
            (parent / "undeclared.o").touch()
            with self.assertRaisesRegex(ValueError, "outside declared roots"):
                ident("../undeclared.o", build, roots)


@unittest.skipUnless(
    platform.system() == "Linux" and platform.machine() == "x86_64"
    and all(shutil.which(x) for x in ("gcc", "ar", "readelf"))
    and Path("/usr/lib64").is_dir() and Path("/usr/lib").is_dir(),
    "requires GNU/Linux x86-64 linker and readelf, with usrmerge roots",
)
class RealGNUFixture(unittest.TestCase):
    def build(self, directory):
        # ELF compared and inspected, never executed.
        for name, body in {
            "main": "int chosen(void); int needed(void); int main(void) { return chosen() + needed(); }\n",
            "chosen": "int chosen(void) { return 0; }\n",
            "idle": "int idle(void) { return 0; }\n",
            "needed": "int needed(void) { return 0; }\n",
        }.items():
            (directory / f"{name}.c").write_text(body, encoding="ascii")

        def run(*args):
            return subprocess.run(args, cwd=directory, check=True, capture_output=True)

        run("gcc", "-c", "chosen.c", "-o", "chosen.o")
        run("gcc", "-c", "idle.c", "-o", "idle.o")
        run("gcc", "-c", "main.c", "-o", "main.o")
        run("ar", "rcs", "libchosen.a", "chosen.o")
        run("ar", "rcs", "libidle.a", "idle.o")
        run("gcc", "-shared", "-fPIC", "-Wl,-soname,libneeded.so", "needed.c", "-o", "libneeded.so")
        run("gcc", "-shared", "-fPIC", "-Wl,-soname,libunused.so", "idle.c", "-o", "libunused.so")
        link = ("gcc", "main.o", "libchosen.a", "libidle.a", "-Wl,--as-needed", "./libneeded.so", "./libunused.so")
        run(*link, "-o", "normal")
        raw = run(*link, "-Wl,-t,-t", "-o", "traced").stdout
        trace = directory / "ld.stdout"
        trace.write_bytes(raw)
        self.assertEqual((directory / "traced").read_bytes(), (directory / "normal").read_bytes())
        roots = [("system-lib", Path("/usr/lib")), ("system-loader", Path("/usr/lib64"))]
        return collect(directory, roots, trace, directory / "traced", shutil.which("readelf"))

    def test_two_real_links_include_only_selected_archive_member_and_needed_dso(self):
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            observation = self.build(Path(first).resolve())
            self.assertEqual(observation, self.build(Path(second).resolve()))
        self.assertFalse(observation["dependency_closure_verified"])
        self.assertFalse(observation["runtime_resolution_collected"])
        self.assertFalse(observation["release_ready"])
        self.assertIn("build/libidle.a", observation["files"])
        self.assertIn("build/libunused.so", observation["files"])
        self.assertIn("libneeded.so", observation["direct_dt_needed"])
        self.assertNotIn("libunused.so", observation["direct_dt_needed"])
        self.assertEqual([
            (event["archive"], event["member"])
            for event in observation["selected_archive_members"] if event["archive"].startswith("build/")
        ], [("build/libchosen.a", "chosen.o")])


if __name__ == "__main__":
    unittest.main()
