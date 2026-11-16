#!/usr/bin/env python3
"""Compare declared exact QuadriFlow build material pins, not release evidence.

A caller must collect and independently verify the input digests. The comparison
does not verify the underlying files, detect omitted inputs, create attestations,
validate licenses, grant installer capabilities, or make a package releasable.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

MAX_INPUT_BYTES = 1024 * 1024
MAX_MATERIALS = 512
PLATFORMS = {"linux-x86_64", "macos-aarch64", "windows-x86_64"}
MANDATORY = {
    "meshharbor-source": ("source", "git-sha1"),
    "quadriflow-source": ("source", "git-sha1"),
    "build-recipe": ("recipe", "sha256"),
}
COVERAGE_KINDS = {"compiler", "linker", "runtime", "package", "toolchain"}
COVERAGE_STATES = {"observed", "partial", "unsupported", "not-collected"}
DIGEST_LENGTHS = {"git-sha1": 40, "sha256": 64, "sha512": 128}
IDENTIFIER = re.compile(r"[a-z][a-z0-9-]{0,63}")
TOKEN = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_.:+-]{0,119}")
HEX = re.compile(r"[a-f0-9]+")


def canonical(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=True, separators=(",", ":")) + "\n").encode("ascii")


def require_fields(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= set(value) <= set(required) | set(optional):
        raise ValueError("invalid receipt object fields")


def allowed(value, candidates):
    return isinstance(value, str) and value in candidates


def valid_token(value, pattern):
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def normalize(value):
    require_fields(value, {"schema_version", "platform", "materials", "coverage", "observations"})
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ValueError("unsupported materials receipt schema")
    if not allowed(value["platform"], PLATFORMS):
        raise ValueError("unsupported materials platform")
    raw = value["materials"]
    if not isinstance(raw, list) or not 3 <= len(raw) <= MAX_MATERIALS:
        raise ValueError("invalid materials count")
    materials = {}
    observed_kinds = set()
    for entry in raw:
        require_fields(entry, {"id", "kind", "algorithm", "digest"}, {"version"})
        ident, kind, algorithm, digest = (entry[key] for key in ("id", "kind", "algorithm", "digest"))
        if not valid_token(ident, IDENTIFIER) or ident in materials:
            raise ValueError("invalid or duplicate material identity")
        if not allowed(kind, COVERAGE_KINDS | {"source", "recipe"}):
            raise ValueError("unknown material kind")
        if not allowed(algorithm, DIGEST_LENGTHS):
            raise ValueError("unknown material digest algorithm")
        if not valid_token(digest, HEX) or len(digest) != DIGEST_LENGTHS[algorithm]:
            raise ValueError("invalid lowercase material digest")
        if ident in MANDATORY:
            if MANDATORY[ident] != (kind, algorithm) or "version" in entry:
                raise ValueError("unexpected source or recipe identity")
        elif kind in {"source", "recipe"} or algorithm == "git-sha1":
            raise ValueError("unexpected source or recipe identity")
        if kind in {"package", "toolchain"} and not valid_token(entry.get("version"), TOKEN):
            raise ValueError("package and toolchain pins require an exact version token")
        if "version" in entry and not valid_token(entry["version"], TOKEN):
            raise ValueError("invalid material version token")
        observed_kinds.add(kind)
        materials[ident] = dict(entry)
    if not MANDATORY.keys() <= materials.keys():
        raise ValueError("missing required source or recipe pins")

    coverage = value["coverage"]
    require_fields(coverage, COVERAGE_KINDS)
    for kind, state in coverage.items():
        if not allowed(state, COVERAGE_STATES):
            raise ValueError("unsupported coverage state")
        if state == "observed" and kind not in observed_kinds:
            raise ValueError("observed coverage has no corresponding material")
        if state in {"unsupported", "not-collected"} and kind in observed_kinds:
            raise ValueError("material contradicts declared collection coverage")
    observations = value["observations"]
    require_fields(observations, {"replica", "run_id", "run_attempt", "runner_image"})
    if not allowed(observations["replica"], {"primary", "independent"}):
        raise ValueError("invalid replica role")
    for key in ("run_id", "run_attempt"):
        if type(observations[key]) is not int or not 0 < observations[key] < 2**63:
            raise ValueError("invalid workflow run observation")
    if not valid_token(observations["runner_image"], TOKEN):
        raise ValueError("invalid runner image observation")

    semantic = {
        "platform": value["platform"], "coverage": dict(coverage),
        "materials": [materials[ident] for ident in sorted(materials)],
    }
    return {
        "schema_version": 1,
        "semantic": semantic,
        "semantic_sha256": hashlib.sha256(canonical(semantic)).hexdigest(),
        "observations": dict(observations),
        "diagnostic_only": True,
        "dependency_closure_verified": False,
        "release_eligible": False,
    }


def compare(raw_primary, raw_independent):
    primary, independent = normalize(raw_primary), normalize(raw_independent)
    po, io = primary["observations"], independent["observations"]
    if (po["replica"], io["replica"]) != ("primary", "independent"):
        raise ValueError("expected primary and independent receipts in that order")
    if any(po[key] != io[key] for key in ("run_id", "run_attempt")):
        raise ValueError("replicas must belong to the same invocation")
    if primary["semantic"]["platform"] != independent["semantic"]["platform"]:
        raise ValueError("replicas must target the same platform")
    pins_match = primary["semantic"]["materials"] == independent["semantic"]["materials"]
    coverage_matches = primary["semantic"]["coverage"] == independent["semantic"]["coverage"]
    return {
        "platform": primary["semantic"]["platform"],
        "semantic_match": pins_match and coverage_matches,
        "material_pins_match": pins_match,
        "coverage_matches": coverage_matches,
        "primary_semantic_sha256": primary["semantic_sha256"],
        "independent_semantic_sha256": independent["semantic_sha256"],
        "diagnostic_only": True,
        "dependency_closure_verified": False,
        "release_eligible": False,
    }


def _unique_json_fields(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON field")
        result[key] = value
    return result


def read_input(path):
    if not path.is_file() or path.is_symlink():
        raise ValueError("expected a regular local receipt file")
    with path.open("rb") as handle:
        data = handle.read(MAX_INPUT_BYTES + 1)
    if len(data) > MAX_INPUT_BYTES:
        raise ValueError("receipt exceeds the input size limit")
    return json.loads(data, object_pairs_hook=_unique_json_fields)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("normalize").add_argument("receipt", type=Path)
    comparison = commands.add_parser("compare")
    comparison.add_argument("primary", type=Path)
    comparison.add_argument("independent", type=Path)
    args = parser.parse_args(argv)
    try:
        result = (
            normalize(read_input(args.receipt)) if args.command == "normalize"
            else compare(read_input(args.primary), read_input(args.independent))
        )
    except (OSError, UnicodeError, json.JSONDecodeError, RecursionError):
        parser.error("could not read or decode a bounded local receipt")
    except ValueError as exc:
        parser.error(str(exc))
    sys.stdout.buffer.write(canonical(result))
    return 0 if result.get("semantic_match", True) else 1


if __name__ == "__main__":
    raise SystemExit(main())
