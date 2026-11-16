#!/usr/bin/env python3
"""Collect diagnostic-only evidence from GNU ld -t -t and GNU readelf.

Use a trace captured during the exact build producing the ELF. Standalone
inputs cannot prove that association. Neither trace nor ELF is executed.
"""

from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

from quadriflow_compile_inputs import ROOT_LABEL, canonical_root, hash_input

TRACE_CAP = 1024 * 1024
FILE_BUDGET = 256 * 1024 * 1024
MAX_EVENTS = 2048
PATH_TOKEN = re.compile(r"[-a-zA-Z0-9_./+]{1,2048}")
ARCHIVE_RECORD = re.compile(r"\(([^()]+\.a)\)([-a-zA-Z0-9_./+]{1,2048})")
ELF_NEEDED = re.compile(r"\s*0x[0-9a-f]+ \(NEEDED\)\s+Shared library: \[([-a-zA-Z0-9_.+]+)\]\s*")
ELF_INTERPRETER = re.compile(r"\s*\[Requesting program interpreter: (/[a-zA-Z0-9_./+-]+)\]\s*")


def trace_records(text):
    lines = text.split("\n")
    if lines[-1] != "":
        raise ValueError("incomplete linker trace")
    if not 1 <= len(lines) - 1 <= MAX_EVENTS:
        raise ValueError("invalid linker trace event count")
    for line in lines[:-1]:
        if match := ARCHIVE_RECORD.fullmatch(line):
            member = match[2]
            if member.startswith("/") or any(x in {"", ".", ".."} for x in member.split("/")):
                raise ValueError("unsupported GNU archive member name")
            yield match[1], member
        elif PATH_TOKEN.fullmatch(line):
            yield line, None
        else:
            raise ValueError("unsupported GNU linker trace record")


def system_alias(path):
    # usrmerge path aliases can be printed by GNU ld even with /usr/lib used
    # as the canonical root. Do not explain arbitrary symlinks as aliases.
    for alias, expected in ((Path("/lib"), Path("/usr/lib")), (Path("/lib64"), Path("/usr/lib64"))):
        if path.is_relative_to(alias) and alias.resolve() == expected:
            return expected / path.relative_to(alias)
    return path


def ident(value, base, roots):
    spelling = Path(value)
    given = spelling if spelling.is_absolute() else base / spelling
    lexical = system_alias(Path(os.path.normpath(given)))

    def key(path):
        for label, root in roots:
            if path.is_relative_to(root):
                return label, path.relative_to(root).as_posix()
        raise ValueError("linker path is outside declared roots")

    source, spelled = key(lexical)
    physical = given.resolve(strict=True)
    resolved, relative = key(physical)
    if source != resolved and not (
        source == "system-loader" and resolved == "system-lib" and lexical.parent == Path("/usr/lib64")
        and roots.count(("system-loader", Path("/usr/lib64"))) == 1
        and roots.count(("system-lib", Path("/usr/lib"))) == 1
        and lexical.name == relative.rsplit("/", 1)[-1] == "ld-linux-x86-64.so.2"
    ):
        raise ValueError("linker input crosses declared roots")
    if not physical.is_file():
        raise ValueError("linker input is not a regular file")
    if not PATH_TOKEN.fullmatch(spelled) or not PATH_TOKEN.fullmatch(relative):
        raise ValueError("unsupported linker file spelling")
    return f"{source}/{spelled}", f"{resolved}/{relative}", physical


def readelf_lines(program, option, elf):
    result = subprocess.run([program, option, str(elf)], check=True, capture_output=True, timeout=20,
                            env={**os.environ, "LC_ALL": "C"})
    if len(result.stdout) > TRACE_CAP or result.stderr:
        raise ValueError("unexpected readelf output")
    return result.stdout.decode("utf-8").splitlines()


def collect(build, more_roots, trace, elf, readelf):
    build = canonical_root(build)
    elf = elf.absolute()
    if elf.is_symlink() or not elf.resolve(strict=True).is_relative_to(build):
        raise ValueError("ELF is not directly inside the build tree")
    if len(more_roots) > 16:
        raise ValueError("too many declared linker roots")
    roots = {"build": build}
    for label, path in more_roots:
        canonical = canonical_root(path)
        if not ROOT_LABEL.fullmatch(label) or label in roots or canonical in roots.values():
            raise ValueError("invalid or duplicate linker root")
        roots[label] = canonical
    ordered = sorted(roots.items(), key=lambda item: len(item[1].parts), reverse=True)

    if trace.is_symlink() or not trace.is_file():
        raise ValueError("trace is not a regular file")
    with trace.open("rb") as stream:
        raw = stream.read(TRACE_CAP + 1)
    if len(raw) > TRACE_CAP:
        raise ValueError("linker trace exceeds size limit")

    before = hash_input(elf, FILE_BUDGET)
    dynamic = readelf_lines(readelf, "-dW", elf)
    phdrs = readelf_lines(readelf, "-lW", elf)
    if hash_input(elf, FILE_BUDGET) != before:
        raise ValueError("ELF changed during metadata inspection")
    needed = [ELF_NEEDED.fullmatch(line) for line in dynamic if "(NEEDED)" in line]
    interpreter = [ELF_INTERPRETER.fullmatch(line) for line in phdrs if "Requesting program interpreter" in line]
    if not needed or not all(needed) or len(interpreter) != 1 or interpreter[0] is None:
        raise ValueError("unsupported dynamic GNU/Linux ELF metadata")
    direct_needed = [m[1] for m in needed]

    opened, selected, files = [], [], {}
    remaining = FILE_BUDGET
    regular_archives = set()
    for value, member in trace_records(raw.decode("utf-8")):
        spelling, resolved, physical = ident(value, build, ordered)
        if member is not None:
            if resolved not in regular_archives:
                raise ValueError("GNU selected member lacked a prior regular archive open")
            selected.append({"archive": resolved, "member": member, "after_open_count": len(opened)})
            continue
        opened.append({"spelling": spelling, "resolved": resolved})
        if resolved not in files:
            fingerprint = hash_input(physical, remaining)
            remaining -= fingerprint["size_bytes"]
            files[resolved] = fingerprint
        if value.endswith(".a"):
            with physical.open("rb") as stream:
                if stream.read(8) != b"!<arch>\n":
                    raise ValueError("unsupported non-regular archive in GNU linker trace")
            regular_archives.add(resolved)
    interpreter_path = ident(interpreter[0][1], build, ordered)
    return {
        "schema_version": 1,
        "method": "gnu-ld-double-trace-and-readelf",
        "diagnostic_only": True,
        "dependency_closure_verified": False,
        "runtime_resolution_collected": False,
        "release_ready": False,
        "elf": before,
        "opened_inputs": opened,
        "selected_archive_members": selected,
        "files": files,
        "direct_dt_needed": direct_needed,
        "interpreter": {"spelling": interpreter_path[0], "resolved": interpreter_path[1]},
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=Path, required=True)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--root", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        roots = [(key, Path(value)) for key, value in (item.split("=", 1) for item in args.root)]
        readelf = shutil.which("readelf")
        if not readelf:
            raise ValueError("GNU readelf was not available")
        result = collect(args.build, roots, args.trace, args.elf, readelf)
    except ValueError as exc:
        parser.error(str(exc))
    except (OSError, RuntimeError, subprocess.SubprocessError):
        parser.error("could not read a declared file or inspect GNU/Linux ELF metadata")
    json.dump(result, sys.stdout, sort_keys=True, indent=2)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
