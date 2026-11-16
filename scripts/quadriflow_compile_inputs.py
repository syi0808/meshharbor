#!/usr/bin/env python3
"""Collect diagnostic-only POSIX compiler inputs from explicit GCC/Clang depfiles.

Run after a successful, quiescent build. Supply the working directory used for
relative depfile paths as --build-root, the expected number of translation
units, each depfile, and canonical roots covering the observed files. This
records compile-time input bytes, not build provenance, linked objects,
runtime dependencies, or a complete build closure. No command is executed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys

MAX_UNITS = 512
MAX_DEPFILE_BYTES = 2 * 1024 * 1024
MAX_DEPFILE_WORDS = 20_000
MAX_FILES = 10_000
MAX_REFERENCES = 100_000
MAX_REPORT_PATH_BYTES = 16 * 1024 * 1024
MAX_FILE_BYTES = 32 * 1024 * 1024
MAX_HASH_BYTES = 256 * 1024 * 1024
ROOT_LABEL = re.compile(r"[a-z][a-z0-9-]{0,39}")


def parse_depfile(raw: bytes) -> tuple[str, list[str]]:
    """Parse one compiler-emitted Make dependency rule, never Make syntax."""
    text = raw.decode("utf-8").replace("\r\n", "\n")
    if any((ord(c) < 32 and c not in "\n\t") or ord(c) == 127 for c in text):
        raise ValueError("invalid control character in depfile")
    text = re.sub(r"\\\n[ \t]*", " ", text)
    rules = [row for row in text.split("\n") if row.strip()]
    if len(rules) != 1:
        raise ValueError("expected a single compiler dependency rule (no -MP rules)")
    before, after = [], []
    words, current, separator = before, [], False
    row = rules[0]

    def flush():
        if current:
            words.append("".join(current))
            current.clear()
            if len(words) > MAX_DEPFILE_WORDS:
                raise ValueError("too many words in dependency rule")

    i = 0
    while i < len(row):
        char = row[i]
        if char == "\\":
            i += 1
            if i == len(row) or row[i] not in " \t\\#:":
                raise ValueError("unsupported depfile escape")
            current.append(row[i])
        elif char == "$":
            i += 1
            if i == len(row) or row[i] != "$":
                raise ValueError("unexpected Make variable in dependency rule")
            current.append("$")
        elif char == ":":
            if separator:
                raise ValueError("unexpected rule delimiter in dependency rule")
            flush()
            separator = True
            words = after
        elif char in " \t":
            flush()
        elif char == "#":
            raise ValueError("unexpected Make comment in dependency rule")
        else:
            current.append(char)
        i += 1
    flush()
    if not separator or len(before) != 1 or not after:
        raise ValueError("expected one output and nonempty prerequisites")
    return before[0], after


def canonical_root(path: Path) -> Path:
    if not path.is_absolute() or path.resolve(strict=True) != path or not path.is_dir():
        raise ValueError("every input root must be a canonical absolute directory")
    return path


def _match_root(path, ordered_roots):
    for name, root in ordered_roots:
        if path.is_relative_to(root):
            return name, root
    raise ValueError("observed path is outside all declared roots")


def identify(value, build_root, ordered_roots):
    path = Path(value)
    observed = path if path.is_absolute() else build_root / path
    lexical = Path(os.path.normpath(observed))
    # Check the lexical path before following a symlink. A root-crossing alias
    # is not enough evidence to attribute a header to either package. Resolve
    # the original spelling, since a symlink followed by '..' can change it.
    label, root = _match_root(lexical, ordered_roots)
    path = observed.resolve(strict=True)
    actual_label, _ = _match_root(path, ordered_roots)
    if actual_label != label:
        raise ValueError("observed input crosses a declared root via a symlink")
    if not path.is_file():
        raise ValueError("observed input is not a regular file")
    relative = path.relative_to(root).as_posix()
    if any(ord(c) < 32 or ord(c) == 127 for c in relative):
        raise ValueError("observed relative path contains controls")
    return label, relative, path


def bounded_depfile(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError("dependency record is not a regular file")
    with path.open("rb") as stream:
        raw = stream.read(MAX_DEPFILE_BYTES + 1)
    if len(raw) > MAX_DEPFILE_BYTES:
        raise ValueError("compiler dependency record exceeds the size limit")
    return raw


def _stability_info(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def hash_input(path, remaining):
    with path.open("rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise ValueError("compiler input is not a regular file")
        cap = min(MAX_FILE_BYTES, remaining)
        if not 0 <= before.st_size <= cap:
            raise ValueError("observed compiler input exceeds the hash budget")
        digest = hashlib.sha256()
        size = 0
        for chunk in iter(lambda: stream.read(64 * 1024), b""):
            size += len(chunk)
            if size > cap:
                raise ValueError("observed compiler input exceeds the hash budget")
            digest.update(chunk)
        after = os.fstat(stream.fileno())
    if size != before.st_size or _stability_info(before) != _stability_info(after):
        raise ValueError("observed compiler input changed during hashing")
    return {"size_bytes": size, "sha256": digest.hexdigest()}


def collect(build_root, roots, depfiles, expected_units):
    if type(expected_units) is not int or not 1 <= expected_units <= MAX_UNITS:
        raise ValueError("expected unit count is outside the supported range")
    if len(depfiles) != expected_units:
        raise ValueError("supplied depfiles do not match the expected unit count")
    if len(roots) > 32:
        raise ValueError("too many declared roots")
    build_root = canonical_root(build_root)
    mapped = {"build": build_root}
    for label, root in roots:
        if not ROOT_LABEL.fullmatch(label) or label in mapped:
            raise ValueError("invalid or duplicate root label")
        canonical = canonical_root(root)
        if canonical in mapped.values():
            raise ValueError("duplicate declared root directory")
        mapped[label] = canonical
    sorted_roots = sorted(mapped.items(), key=lambda item: len(item[1].parts), reverse=True)

    files, objects, total, references, path_bytes = {}, {}, 0, 0, 0
    for depfile in depfiles:
        target, deps = parse_depfile(bounded_depfile(depfile))
        ident, rel, _ = identify(target, build_root, sorted_roots)
        if ident != "build" or not rel.endswith(".o"):
            raise ValueError("object is not a POSIX build-root .o file")
        if rel in objects:
            raise ValueError("multiple records describe one object")
        inputs = set()
        source = None
        for dep in deps:
            label, rel_path, physical = identify(dep, build_root, sorted_roots)
            name = f"{label}/{rel_path}"
            if source is None:
                source = name
            if name not in inputs:
                references += 1
                path_bytes += len(json.dumps(name, ensure_ascii=True))
                if references > MAX_REFERENCES or path_bytes > MAX_REPORT_PATH_BYTES:
                    raise ValueError("reported input references exceed the output budget")
            if name not in files:
                if len(files) >= MAX_FILES:
                    raise ValueError("too many observed compiler inputs")
                fingerprint = hash_input(physical, MAX_HASH_BYTES - total)
                files[name] = fingerprint
                total += fingerprint["size_bytes"]
            inputs.add(name)
        objects[rel] = {"first_prerequisite": source, "inputs": sorted(inputs)}

    return {
        "schema_version": 1,
        "method": "posix-compiler-make-depfiles",
        "diagnostic_only": True,
        "dependency_closure_verified": False,
        "link_inputs_collected": False,
        "runtime_inputs_collected": False,
        "installable": False,
        "release_ready": False,
        "objects": objects,
        "inputs": files,
    }


def _root_argument(value):
    label, sep, root = value.partition("=")
    if not sep or not label or not root:
        raise argparse.ArgumentTypeError("expected name=/canonical/absolute/path")
    return label, Path(root)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-root", type=Path, required=True)
    parser.add_argument("--root", type=_root_argument, action="append", default=[])
    parser.add_argument("--depfile", type=Path, action="append", required=True)
    parser.add_argument("--expected-units", type=int, required=True)
    args = parser.parse_args(argv)
    if os.name != "posix":
        parser.error("only POSIX compiler depfiles are supported")
    try:
        evidence = collect(args.build_root, args.root, args.depfile, args.expected_units)
    except (OSError, RuntimeError):
        # OSError may include an input or host filesystem path.
        parser.error("could not read or resolve a declared input")
    except ValueError as exc:
        parser.error(str(exc))
    json.dump(evidence, sys.stdout, ensure_ascii=True, indent=2, sort_keys=True)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
