"""Capture and compare native macOS candidate build inputs without running the binary."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys

if __package__:
    from .quadriflow_compile_inputs import bounded_depfile, hash_input, parse_depfile
else:
    from quadriflow_compile_inputs import bounded_depfile, hash_input, parse_depfile


SCRIPT = Path(__file__).resolve()
RECIPE = SCRIPT.with_name("quadriflow-platform-smoke.py")
RECEIPT = "macos-build-inputs.json"
SCHEMA = "meshharbor-quadriflow-macos-build-inputs-v1"
MAX_RECEIPT_BYTES = 8 * 1024 * 1024
MAX_INPUT_BYTES = 512 * 1024 * 1024
CORE_DYLIBS = {"/usr/lib/libc++.1.dylib", "/usr/lib/libSystem.B.dylib"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_json(path, maximum=MAX_RECEIPT_BYTES):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > maximum:
        raise ValueError(f"missing, indirect or oversized evidence: {path.name}")
    with path.open("rb") as handle:
        data = handle.read(maximum + 1)
    require(len(data) <= maximum, "evidence exceeds read bound")
    return json.loads(data)


def write_json(path, data):
    with path.open("xb") as handle:
        handle.write((json.dumps(data, indent=2, sort_keys=True) + "\n").encode())


def output(*argv):
    return subprocess.check_output(argv, text=True, timeout=60).strip()


def digest(path):
    return hash_input(path, 128 * 1024 * 1024, maximum=MAX_INPUT_BYTES)["sha256"]


def option(argv, flag):
    require(argv.count(flag) == 1 and argv.index(flag) + 1 < len(argv), f"missing or ambiguous {flag}")
    return argv[argv.index(flag) + 1]


def link(capture, argv):
    target = Path(option(argv, "-o"))
    if target.name != "quadriflow":
        return subprocess.run(argv, check=False).returncode
    require(target.resolve() == Path.cwd() / "quadriflow", "unexpected QuadriFlow link target")
    capture.mkdir()
    link_map = capture / "link.map"
    command = [*argv, "-Wl,-t", "-Xlinker", "-map", "-Xlinker", str(link_map)]
    write_json(capture / "invocation.json", {"directory": str(Path.cwd()), "argv": argv})
    with (capture / "trace.txt").open("xb") as trace:
        subprocess.run(command, stdout=trace, check=True, timeout=180)
    require(link_map.is_file() and 0 < link_map.stat().st_size <= MAX_RECEIPT_BYTES,
            "native link did not produce a bounded map")
    require(0 < (capture / "trace.txt").stat().st_size <= MAX_RECEIPT_BYTES,
            "native link did not produce a bounded trace")
    return 0


class Inputs:
    def __init__(self, roots):
        self.roots = roots
        self.files = {}
        self.total_bytes = 0
        self.aliases = {str(root): name for name, root in roots.items()}
        self.aliases.update({
            "/opt/homebrew/include/boost": "boost/include/boost",
            "/opt/homebrew/include/eigen3": "eigen/include/eigen3",
        })

    def normalize(self, value):
        for prefix in sorted(self.aliases, key=len, reverse=True):
            if prefix in value:
                value = value.replace(prefix, "$" + self.aliases[prefix])
        return value

    def record(self, path):
        path = path.resolve(strict=True)
        for name, root in self.roots.items():
            if path.is_relative_to(root):
                logical = f"{name}/{path.relative_to(root).as_posix()}"
                if logical not in self.files:
                    require(len(self.files) < 10_000, "too many native build inputs")
                    fingerprint = hash_input(path, 4 * MAX_INPUT_BYTES - self.total_bytes, maximum=MAX_INPUT_BYTES)
                    self.files[logical] = fingerprint
                    self.total_bytes += fingerprint["size_bytes"]
                return logical
        raise ValueError(f"observed input outside approved audit roots: {path}")


def linker_entry(inputs, directory, line):
    archive = re.fullmatch(r"(.+\.a)\(([^()]+)\)", line)
    path, member = archive.groups() if archive else (line, None)
    logical = inputs.record(directory / path)
    return f"{logical}({member})" if member else logical


def mach_o(binary):
    require(output("xcrun", "lipo", "-archs", binary) == "arm64", "candidate is not thin arm64")
    links = []
    for line in output("xcrun", "otool", "-L", binary).splitlines()[1:]:
        match = re.match(r"\s+(\S+) \(compatibility version [^,]+, current version ([^)]+)\)$", line)
        require(match, "could not identify a Mach-O dylib load")
        links.append({"install_name": match[1], "current_version": match[2]})
    require({item["install_name"] for item in links} == CORE_DYLIBS, "unreviewed Mach-O dylib set")
    metadata = output("xcrun", "vtool", "-show-build", binary)
    versions = dict(re.findall(r"(?m)^\s*(platform|minos|sdk)\s+(\S+)$", metadata))
    require(set(versions) == {"platform", "minos", "sdk"} and versions["platform"] == "MACOS",
            "missing native deployment metadata")
    return {"dylibs": sorted(links, key=lambda item: item["install_name"]), "build_version": versions}


def observe(work, capture, sdk, toolchain, boost, eigen, compiler_env=None):
    build = work / "build"
    inputs = Inputs({
        "build": build, "source": work / "upstream", "boost": boost, "eigen": eigen,
        "sdk": sdk, "toolchain": toolchain, "system-tools": Path("/usr/bin"),
    })
    inputs.aliases.update({str(work): "work", str(sdk.parent / "MacOSX.sdk"): "sdk"})
    compile_commands = read_json(build / "compile_commands.json")
    require(isinstance(compile_commands, list) and len(compile_commands) == 22,
            "pinned source must produce 22 compile records")
    units = {}
    depfiles = set()
    for unit in compile_commands:
        directory = Path(unit["directory"])
        command = shlex.split(unit["command"])
        obj = (directory / option(command, "-o")).resolve()
        dep = obj.with_name(obj.name + ".d")
        target, sources = parse_depfile(bounded_depfile(dep))
        require((build / unit["output"]).resolve() == obj
                and obj in {(build / target).resolve(), (directory / target).resolve()},
                "compiler depfile target did not match compile record")
        obj_name = inputs.record(obj)
        require(obj_name.startswith("build/") and obj_name not in units, "duplicate or external compile target")
        source = inputs.record(directory / unit["file"])
        dependencies = {inputs.record(directory / source_path) for source_path in sources}
        require(source in dependencies, "compiler depfile did not include its source")
        units[obj_name] = {
            "source": source, "inputs": sorted(dependencies),
            "command": [inputs.normalize(arg) for arg in command],
        }
        depfiles.add(dep)
    require(depfiles == {path.resolve() for path in build.rglob("*.o.d")}, "unexpected compiler depfile set")

    invocation = read_json(capture / "invocation.json")
    require(Path(invocation["directory"]) == build, "unexpected native link working directory")
    link_args = invocation["argv"]
    require(Path(option(link_args, "-o")).name == "quadriflow", "unexpected native link output")
    for arg in link_args[1:]:
        if arg.endswith((".o", ".a")):
            inputs.record(build / arg)
    trace = (capture / "trace.txt").read_text().splitlines()
    require(len(trace) <= 20_000, "excessive native linker trace")
    seen = sorted({linker_entry(inputs, build, line.strip()) for line in trace if line.strip()})
    require(any(name.startswith("sdk/") for name in seen), "native link trace omitted SDK libraries")
    require(any(name.startswith("toolchain/") for name in seen), "native link trace omitted toolchain inputs")
    selected = []
    for line in (capture / "link.map").read_text().splitlines():
        if line.startswith("# Sections:"):
            break
        match = re.match(r"^\[\s*\d+\]\s+(.+)$", line)
        if match and match[1] != "linker synthesized":
            selected.append(linker_entry(inputs, build, match[1]))
    require({item for item in selected if item in units}
            == {name for name in units if name.startswith("build/CMakeFiles/quadriflow.dir/")},
            "native linker map did not retain the expected QuadriFlow compile outputs")
    require(set(selected) <= set(seen), "native map and link trace disagree on retained inputs")

    tools = {}
    for tool in ("clang", "ld", "ar", "ranlib"):
        path = Path(output("xcrun", "--find", tool)).resolve()
        tools[tool] = inputs.record(path)
    binary = work / "artifact/quadriflow"
    require(digest(binary) == digest(build / "quadriflow"), "staged executable differs from linked output")
    return {
        "executable_sha256": digest(binary),
        "compile": units, "files": inputs.files, "tools": tools,
        "mach_o": mach_o(binary),
        "link": {"command": [inputs.normalize(arg) for arg in link_args],
                 "opened": seen, "retained": sorted(set(selected))},
        "cmake": {
            "version": output("cmake", "--version").splitlines()[0],
            "sha256": digest(Path(shutil.which("cmake")).resolve(strict=True)),
        },
        "macos_build": output("sw_vers", "-buildVersion"),
        "sdk_version": output("xcrun", "--sdk", "macosx", "--show-sdk-version"),
        "brew_versions": {"boost": boost.name, "eigen": eigen.name},
        "build_flags": compiler_env or {},
        "recipe_sha256": digest(RECIPE),
        "collector_sha256": digest(SCRIPT),
        "depfile_parser_sha256": digest(SCRIPT.with_name("quadriflow_compile_inputs.py")),
    }


def build(runner):
    require(platform.system() == "Darwin" and platform.machine() == "arm64",
            "macOS ARM64 is required for native candidate evidence")
    runner = runner.resolve(strict=True)
    work = runner / "quadriflow-platform-smoke"
    capture = runner / "quadriflow-native-link"
    require(not work.exists() and not capture.exists(), "native build needs a clean private build directory")
    sdk = Path(output("xcrun", "--sdk", "macosx", "--show-sdk-path")).resolve(strict=True)
    clang = Path(output("xcrun", "--find", "clang")).resolve(strict=True)
    toolchain = clang.parents[2]
    boost = Path(output("brew", "--prefix", "boost")).resolve(strict=True)
    eigen = Path(output("brew", "--prefix", "eigen")).resolve(strict=True)
    env = dict(os.environ)
    prefix_map = f"-ffile-prefix-map={work}=/meshharbor-quadriflow"
    env.update({
        "RUNNER_TEMP": str(runner), "CMAKE_GENERATOR": "Unix Makefiles",
        "CMAKE_EXPORT_COMPILE_COMMANDS": "ON", "SDKROOT": str(sdk),
        "CMAKE_CXX_LINKER_LAUNCHER": f"{sys.executable};{SCRIPT};link;{capture}",
        "CFLAGS": prefix_map, "CXXFLAGS": prefix_map, "LDFLAGS": "",
        "MACOSX_DEPLOYMENT_TARGET": "15.0",
    })
    subprocess.run([sys.executable, RECIPE, "--build-only"], env=env, check=True)
    observed = observe(work, capture, sdk, toolchain, boost, eigen, {
        "CFLAGS": "-ffile-prefix-map=$work=/meshharbor-quadriflow",
        "CXXFLAGS": "-ffile-prefix-map=$work=/meshharbor-quadriflow",
    })
    provenance = read_json(work / "artifact/provenance.json")
    receipt_path = work / "artifact" / RECEIPT
    write_json(receipt_path, {
        "schema": SCHEMA, "replica": provenance["build_replica"],
        "source_commit": provenance["meshharbor_source_commit"], "observed": observed,
        "dependency_closure_verified": False,
    })
    checksum_path = work / "artifact/SHA256SUMS"
    with checksum_path.open("a", encoding="utf-8") as checksums:
        checksums.write(f"{digest(receipt_path)}  {RECEIPT}\n")
    print(f"Captured {len(observed['compile'])} actual compile units and {len(observed['link']['opened'])} linker inputs.")


def load_receipt(directory):
    receipt = read_json(directory / RECEIPT)
    observed = receipt["observed"]
    require(receipt["schema"] == SCHEMA and receipt["dependency_closure_verified"] is False,
            "invalid evidence schema or unsupportable closure claim")
    require(observed["executable_sha256"] == digest(directory / "quadriflow"),
            "build evidence does not match its executable SHA-256")
    require(len(observed["compile"]) == 22 and observed["files"] and observed["link"]["opened"]
            and observed["link"]["retained"], "incomplete native input receipt")
    return receipt


def compare(primary, independent, source_commit, result, invocation=None):
    require(primary.resolve() != independent.resolve(), "build replica directories must be distinct")
    first, second = load_receipt(primary), load_receipt(independent)
    require(first["replica"] == "primary" and second["replica"] == "independent"
            and first["source_commit"] == second["source_commit"] == source_commit, "build replica identity mismatch")
    mismatches = sorted(key for key in first["observed"].keys() | second["observed"].keys()
                        if first["observed"].get(key) != second["observed"].get(key))
    require(not mismatches, f"independent executable or native build inputs differ: {', '.join(mismatches)}")
    write_json(result, {
        "schema": SCHEMA, "source_commit": source_commit,
        "executable_sha256": first["observed"]["executable_sha256"],
        "primary_receipt_sha256": digest(primary / RECEIPT),
        "independent_receipt_sha256": digest(independent / RECEIPT),
        "invocation": invocation,
        "dependency_closure_verified": False,
    })


def verify_pair(candidate, source_commit, run_id, run_attempt):
    pair = read_json(candidate / "macos-build-inputs-pair.json")
    primary = load_receipt(candidate)
    independent_path = candidate / "macos-independent-build-inputs.json"
    independent = read_json(independent_path)
    require(pair["schema"] == SCHEMA and pair["dependency_closure_verified"] is False
            and independent["schema"] == SCHEMA and independent["dependency_closure_verified"] is False
            and pair["source_commit"] == primary["source_commit"] == independent["source_commit"] == source_commit
            and primary["replica"] == "primary" and independent["replica"] == "independent"
            and primary["observed"] == independent["observed"]
            and pair["executable_sha256"] == primary["observed"]["executable_sha256"]
            and pair["primary_receipt_sha256"] == digest(candidate / RECEIPT)
            and pair["independent_receipt_sha256"] == digest(independent_path),
            "attested pair does not match the uploaded receipts and executable")
    invocation = pair["invocation"]
    require(invocation["run_id"] == run_id and invocation["run_attempt"] == run_attempt
            and invocation["primary_artifact_id"] != invocation["independent_artifact_id"],
            "attested pair belongs to a different build invocation")


def check_sbom(directory, document):
    observed = load_receipt(directory)["observed"]
    packages = {item["SPDXID"]: item for item in document["packages"]}
    require(len(packages) == len(document["packages"]), "SPDX repeats a package identifier")
    hashes = [entry for entry in document["files"] if entry["fileName"] == "./bin/quadriflow"]
    require(document["spdxVersion"] == "SPDX-2.3" and len(hashes) == 1
            and any(item["algorithm"] == "SHA256" and item["checksumValue"] == observed["executable_sha256"]
                    for item in hashes[0]["checksums"]), "SPDX does not identify the measured executable")
    required = {"quadriflow", "boost", "eigen", "lemon", "pcg32", "apple-sdk", "libcxx", "libsystem"}
    source_prefixes = ("source/src/", "source/3rd/lemon-1.3.1/", "source/3rd/pcg32/")
    require(all(any(path.startswith(prefix) for prefix in source_prefixes)
                for path in observed["files"] if path.startswith("source/")),
            "new upstream component requires SBOM review")
    for toolchain_runtime in observed["link"]["retained"]:
        if toolchain_runtime.startswith("toolchain/") and ".a(" in toolchain_runtime:
            required.add("compiler-runtime")
    require({f"SPDXRef-Package-{name}" for name in required} <= packages.keys(),
            "SPDX is missing one or more observed build or runtime components")
    for name, version in observed["brew_versions"].items():
        require(packages[f"SPDXRef-Package-{name}"]["versionInfo"] == version,
                f"SPDX version differs from observed Homebrew {name}")
    require({item["install_name"] for item in observed["mach_o"]["dylibs"]} == CORE_DYLIBS,
            "unreviewed Mach-O dylib set")
    linked = {item["relatedSpdxElement"] for item in document["relationships"]
              if item["spdxElementId"] == "SPDXRef-Package-ReviewPayload" and item["relationshipType"] == "DYNAMIC_LINK"}
    require(linked == {"SPDXRef-Package-libcxx", "SPDXRef-Package-libsystem"},
            "SPDX dynamic dependencies disagree with Mach-O load commands")
    for item in observed["mach_o"]["dylibs"]:
        name = "libcxx" if item["install_name"] == "/usr/lib/libc++.1.dylib" else "libsystem"
        require(packages[f"SPDXRef-Package-{name}"]["versionInfo"] == item["current_version"],
                f"SPDX version differs from the Mach-O load for {name}")
    relationships = {(item["spdxElementId"], item["relationshipType"], item["relatedSpdxElement"])
                     for item in document["relationships"]}
    payload = "SPDXRef-Package-ReviewPayload"
    require(all((payload, "DEPENDS_ON", f"SPDXRef-Package-{name}") in relationships
                for name in ("boost", "eigen", "lemon", "pcg32")),
            "SPDX does not link the observed components to the executable payload")
    require("compiler-runtime" not in required or
            (payload, "STATIC_LINK", "SPDXRef-Package-compiler-runtime") in relationships,
            "SPDX omits the retained compiler runtime link")
    print("SPDX executable SHA-256 and reviewed component coverage match; license and closure approval are separate.")


def main():
    if len(sys.argv) > 3 and sys.argv[1] == "link":
        return link(Path(sys.argv[2]), sys.argv[3:])
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--runner-temp", required=True, type=Path)
    compare_parser = commands.add_parser("compare")
    for name in ("primary", "independent", "output"):
        compare_parser.add_argument("--" + name, required=True, type=Path)
    compare_parser.add_argument("--source-commit", required=True)
    for name in ("run-id", "run-attempt", "primary-artifact-id", "independent-artifact-id"):
        compare_parser.add_argument("--" + name, type=int)
    verify_parser = commands.add_parser("verify-pair")
    verify_parser.add_argument("--candidate", required=True, type=Path)
    verify_parser.add_argument("--source-commit", required=True)
    verify_parser.add_argument("--run-id", required=True, type=int)
    verify_parser.add_argument("--run-attempt", required=True, type=int)
    sbom_parser = commands.add_parser("check-sbom")
    sbom_parser.add_argument("--candidate", required=True, type=Path)
    sbom_parser.add_argument("--sbom", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "build":
            build(args.runner_temp)
        elif args.command == "compare":
            require(re.fullmatch(r"[0-9a-f]{40}", args.source_commit), "invalid source commit")
            receipt_ids = {name: getattr(args, name) for name in (
                "run_id", "run_attempt", "primary_artifact_id", "independent_artifact_id",
            )}
            if any(value is not None for value in receipt_ids.values()):
                require(all(value is not None and value > 0 for value in receipt_ids.values())
                        and args.primary_artifact_id != args.independent_artifact_id, "invalid hosted invocation IDs")
            else:
                receipt_ids = None
            compare(args.primary, args.independent, args.source_commit, args.output, receipt_ids)
        elif args.command == "verify-pair":
            verify_pair(args.candidate, args.source_commit, args.run_id, args.run_attempt)
        else:
            check_sbom(args.candidate, read_json(args.sbom))
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        parser.exit(1, f"ERROR: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
