# Offline apt package evidence snapshot

`scripts/quadriflow_apt_package_evidence.py` can capture the installed binary package version, architecture, source package/version, and the exact bytes of each requested Debian copyright notice. It is a local, offline inventory helper. It does **not** install packages, query a repository, discover a complete dependency closure, identify the archive/source origin, or decide license compatibility or release readiness.

On an Ubuntu/Debian host with the requested packages installed, name every package you want included explicitly:

```sh
python3 scripts/quadriflow_apt_package_evidence.py collect \
  --package libeigen3-dev \
  --package libboost-dev \
  --output /tmp/meshharbor-apt-evidence

python3 scripts/quadriflow_apt_package_evidence.py verify \
  /tmp/meshharbor-apt-evidence
```

If the headers actually used by the build come from additional binary packages, include those package names too. This tool deliberately reports `dependency_closure_verified: false` and `release_ready: false`; passing its offline bundle verification is only a check that the explicitly listed package metadata and copied notice bytes are internally consistent. The bundle's `evidence.json` includes a SHA-256 and package owner for each copyright file. The full notice bytes are retained under `notices/` for review. The JSON schema documents fields; the offline `verify` command additionally enforces cross-field checks, file hashes, bounds, and the no-extra-files contract. Always run it after copying a bundle.

The collector assumes the local dpkg database and documentation tree are quiescent while read. It is not a sandbox against concurrent hostile filesystem mutation.

No candidate workflow invokes this helper. Its output does not alter or supplement prior candidate runs, signed provenance, the product manifest, or release decisions. A future build integration would need a separately reviewed source-origin/transaction record, complete package closure, and matching attestation policy; do not treat this snapshot alone as distribution evidence.
