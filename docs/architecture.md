# Architecture status

## Intended product shape

The approved design is a Rust application core with Axum/Tokio, Clap, the official `rmcp` SDK, a shared operation registry, SQLite metadata, an artifact file store, and a React/TypeScript/Three.js Studio. Python is managed behind isolated CUDA or MLX workers. REST, CLI, MCP, and the UI must call the same operations and preserve the same input/output contracts.

## Prototype implementation in this checkout

The available Cloud computer has Node.js, Python 3, and g++, but no Rust/Cargo or native UI package dependencies. This prototype uses Node's built-in HTTP, SQLite, test, and child-process modules to provide a runnable operation/API/UI shell without downloading packages. The simplifier itself is the C++17 worker from the user's `glb-mesh-refiner` repository. The static canvas viewer is a deliberately small fallback, not a replacement for the planned Three.js viewer.

The Node contract prototype keeps the architectural boundaries that matter for the first flow, but its files are not the final product implementation:

- `prototype/node-shell/apps/server/operations.mjs` is the single operation and model-capability registry.
- `prototype/node-shell/apps/server/store.mjs` owns SQLite metadata and content-addressed local artifacts.
- `prototype/node-shell/apps/server/jobs.mjs` owns the job state machine and isolated native-worker lifecycle.
- `prototype/node-shell/apps/server/api.mjs` exposes the REST contract and SSE job events.
- `prototype/node-shell/apps/cli.mjs` and `prototype/node-shell/apps/mcp.mjs` exercise shared core operations through the prototype's CLI and MCP surface.
- `prototype/node-shell/apps/web/` is the prototype Studio UI, not the planned React/TypeScript/Three.js client.

## Implemented flow

`GLB import → input validation → mesh.simplify job → isolated C++ worker → output validation → new derived asset and report artifacts`

The original artifact is immutable. Results record requested and actual triangle counts, worker version, and report artifacts. Textures and supported material JSON are retained by the refiner; unsupported inputs fail rather than being silently rewritten.

## Not yet implemented

The prototype does not meet the first public release bar: no generation runtime is approved, the Rust application and official MCP SDK are not yet implemented, and the three-OS packaging and full PBR viewer are still pending. Rust/Cargo installation was attempted in the available Cloud environment but blocked because the environment lacks root package-manager access; this did not change the intended architecture.
