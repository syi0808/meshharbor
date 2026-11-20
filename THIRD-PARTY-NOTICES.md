# Third-party notices

The root [MIT license](LICENSE) covers MeshHarbor's own code and documentation. It does not relicense vendored code, copied assets, fonts, dependencies, or model materials. Preserve their applicable copyright, attribution, and license notices when redistributing them.

| Component | Notices and provenance |
| --- | --- |
| Pixal3D CUDA and MLX source snapshots | [Pixal3D notices](runtimes/pixal3d/THIRD-PARTY-NOTICES.md), [upstream license texts](runtimes/pixal3d/THIRD-PARTY-LICENSES/), and [pinned source inventory](docs/pixal3d-source-inventory.json). Port MIT notices do not override the separate terms for DINOv3 or other derived components. |
| Vendored xatlas and stb headers | [Native source notices](crates/xatlas-sys/THIRD_PARTY_NOTICES.md) and the [xatlas license](crates/xatlas-sys/vendor/xatlas/LICENSE). |
| Studio UI and bundled assets | [Studio notices](apps/studio/THIRD-PARTY-NOTICES.md) for the checked-in client. See also the [pnpm lockfile](apps/studio/pnpm-lock.yaml). |
| Rust and native build dependencies | [Cargo lockfile](Cargo.lock) and [licensing status](docs/licensing-status.md). |
| External or separately downloaded components | [TRELLIS Text CUDA terms](docs/trellis-text-cuda.md#upstream-and-terms), [TRELLIS Text MLX provenance](docs/trellis-text-mlx.md#provenance-and-verification), [TRELLIS.2 and TripoSG terms](docs/external-generation-runtimes.md#choose-a-backend), [QuadriFlow candidate notes](docs/retopology-candidate.md#upstream-and-licensing), and [model/runtime licensing status](docs/licensing-status.md). No model weights or QuadriFlow executable are bundled. |

This is an index for the source checkout, not a complete notice bundle or software bill of materials for a release. See the [release checklist](docs/releasing.md) before distributing a package or executable.
