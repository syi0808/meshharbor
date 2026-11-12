# Node contract prototype

This is a temporary Node.js implementation for exercising the MeshHarbor operation, REST, CLI, MCP, storage, job, and UI contracts while Rust/Cargo is unavailable in the current Cloud environment. It does not replace the Rust application architecture in `../../docs/architecture.md`.

## Requirements and commands

Node.js 22+, a C++17 compiler, GNU Make, and Python 3 with NumPy and Pillow for the native suite.

```sh
npm test
npm run test:native
npm start
```

The server binds only to `127.0.0.1` and prints a local bearer token. Use the printed URL and token in the Studio. Example CLI operations:

```sh
node apps/cli.mjs models list --json
node apps/cli.mjs runtimes doctor --json
node apps/cli.mjs simplify source.glb --triangles 4000 -o simplified.glb
node apps/cli.mjs mcp
```

This prototype is not a release build. It does not download models or enable image generation. Consult the root licensing and architecture notes before distribution or integration into the product.
