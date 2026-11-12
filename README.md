# MeshHarbor

MeshHarbor is an early local-first 3D asset studio. This repository contains the first native C++ mesh-refinement component and an explicitly separate Node.js contract prototype. The intended product architecture remains Rust/C++ as described in [docs/architecture.md](docs/architecture.md).

## What is implemented

- `native/mesh-refiner/`: the user's GLB refiner source at a pinned revision, built as a C++17 worker. It simplifies supported static GLB assets and writes a new result while retaining the original.
- `prototype/node-shell/`: a temporary local shell for exercising the planned REST, CLI, MCP, storage, job, and UI contracts while Rust/Cargo are unavailable in the current Cloud environment. It is not the product's permanent application architecture.
- `registry/models/pixal3d.json`: model candidates remain pending and non-installable until their complete runtime, dependency, and weight-license paths are cleared.

No model downloads or image generation are enabled. This repository is public at the owner's request, but no root project license or third-party redistribution grant has been added. Do not treat the code as commercially cleared or open-source licensed.

## Run the isolated prototype

Requirements: Node.js 22+, a C++17 compiler, GNU Make, and Python 3 with NumPy and Pillow for the native test suite.

```sh
cd prototype/node-shell
npm test
npm run test:native
npm start
```

The prototype server binds to `127.0.0.1` by default and prints a local bearer token. It never binds to a public interface by default. See the prototype directory for the current commands and limitations.

## Scope and limitations

The native refiner supports one static indexed triangle primitive with float POSITION, NORMAL, and TEXCOORD_0 attributes. Unsupported attributes, animation, skinning, morph targets, sparse accessors, extensions, extra meshes, and extra primitives fail explicitly. The requested triangle target is aspirational; its UV correspondence threshold is a sampled heuristic, and it does not apply node transforms. Image generation, retopology, semantic separation, texture baking, compression, and model install/repair remain out of scope until their dependencies and preservation contracts are validated.

## Licensing

No root `LICENSE` is included because the product license has not been selected. See [docs/licensing-status.md](docs/licensing-status.md) before further distribution. The copied refiner source came from the user's repository at a pinned revision under the user's explicit instruction; the source repo had no root license. Do not infer a public license for MeshHarbor from the MIT notices on the Pixal3D porting repositories.
