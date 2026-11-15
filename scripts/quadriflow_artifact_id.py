#!/usr/bin/env python3
"""Select exactly one immutable artifact ID from a GitHub Actions run response."""

from __future__ import annotations

import argparse
import json
import re
import stat
import sys
from pathlib import Path

MAX_RESPONSE_BYTES = 1024 * 1024
MAX_ARTIFACT_BYTES = 160 * 1024 * 1024
ARTIFACT_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


class ArtifactSelectionError(ValueError):
    """Raised when an Actions API response is ambiguous or incomplete."""


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ArtifactSelectionError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def select_artifact_id(response: bytes, expected_name: str) -> str:
    if not ARTIFACT_NAME.fullmatch(expected_name):
        raise ArtifactSelectionError("artifact name is not a supported exact name")
    if not response or len(response) > MAX_RESPONSE_BYTES:
        raise ArtifactSelectionError("Actions artifact response is empty or exceeds the size limit")
    try:
        payload = json.loads(response, object_pairs_hook=_unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ArtifactSelectionError("Actions artifact response is not valid JSON") from error

    if not isinstance(payload, dict):
        raise ArtifactSelectionError("Actions artifact response must be a JSON object")
    total = payload.get("total_count")
    artifacts = payload.get("artifacts")
    if type(total) is not int or not isinstance(artifacts, list):
        raise ArtifactSelectionError("Actions artifact response is missing its count or artifact list")
    if total != len(artifacts) or total > 100:
        raise ArtifactSelectionError("Actions artifact response is incomplete or exceeds one page")

    matches = [
        artifact
        for artifact in artifacts
        if isinstance(artifact, dict) and artifact.get("name") == expected_name
    ]
    if len(matches) != 1:
        raise ArtifactSelectionError("expected exactly one artifact with the requested name")

    artifact = matches[0]
    artifact_id = artifact.get("id")
    size = artifact.get("size_in_bytes")
    if type(artifact_id) is not int or artifact_id <= 0:
        raise ArtifactSelectionError("artifact ID must be a positive integer")
    if artifact.get("expired") is not False:
        raise ArtifactSelectionError("selected artifact is expired or has an invalid expiration flag")
    if type(size) is not int or size <= 0 or size > MAX_ARTIFACT_BYTES:
        raise ArtifactSelectionError("selected artifact size is invalid or exceeds the limit")
    return str(artifact_id)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("response", type=Path, help="JSON response from the run-artifacts API")
    parser.add_argument("--name", required=True, help="exact artifact name to select")
    args = parser.parse_args(argv)

    try:
        metadata = args.response.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > MAX_RESPONSE_BYTES:
            raise ArtifactSelectionError("artifact response file is not a bounded regular file")
        artifact_id = select_artifact_id(args.response.read_bytes(), args.name)
    except (OSError, ArtifactSelectionError) as error:
        print(f"artifact selection failed: {error}", file=sys.stderr)
        return 2
    print(f"artifact_id={artifact_id}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
