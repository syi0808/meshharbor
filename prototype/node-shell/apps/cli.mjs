import { randomUUID } from "node:crypto";
import { accessSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { detectAsset } from "./server/glb.mjs";
import { JobManager } from "./server/jobs.mjs";
import { listModels, runtimeDoctor } from "./server/operations.mjs";
import { Store } from "./server/store.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const workerPath = path.join(projectRoot, "native/mesh-refiner/glb_refine");
const dataDirectory = path.resolve(process.env.MESHHARBOR_HOME || path.join(homedir(), ".meshharbor"));
const args = process.argv.slice(2);
const command = args[0] || "serve";
const jsonOutput = args.includes("--json");
const readOption = (name, fallback = undefined) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
};

function print(value) {
  process.stdout.write(`${jsonOutput ? JSON.stringify(value, null, 2) : String(value)}\n`);
}

function help() {
  process.stdout.write(`MeshHarbor local Node contract prototype\n\nCommands:\n  node apps/cli.mjs serve [--port 8787]\n  node apps/cli.mjs models list [--json]\n  node apps/cli.mjs runtimes doctor [--json]\n  node apps/cli.mjs import <image.png|asset.jpg|model.glb> [--json]\n  node apps/cli.mjs simplify <model.glb> --triangles <count> -o <output.glb> [--json]\n  node apps/cli.mjs generate <image.png> --model pixal3d -o <model.glb>\n  node apps/cli.mjs mcp\n`);
}

function ensureNativeWorker() {
  try { accessSync(workerPath); return true; } catch { /* Build below. */ }
  const result = spawnSync("make", ["-C", path.join(projectRoot, "native/mesh-refiner"), "glb_refine"], { stdio: "inherit" });
  return !result.error && result.status === 0;
}

async function run() {
  if (["help", "--help", "-h"].includes(command)) return help();
  if (command === "serve") return import("./server/main.mjs");
  if (command === "mcp") return import("./mcp.mjs");
  if (command === "models" && args[1] === "list") {
    const models = listModels();
    return print(jsonOutput ? { models } : models.map(model => `${model.name}: ${model.status}${model.installable ? " (installable)" : " (not installable)"}`).join("\n"));
  }
  if (command === "runtimes" && args[1] === "doctor") {
    return print(runtimeDoctor({ workerAvailable: ensureNativeWorker() }));
  }
  if (command === "generate") {
    const message = "Image generation is disabled until a complete model runtime, weight, and license path is approved.";
    print(jsonOutput ? { error: { code: "MODEL_PENDING", message } } : message);
    process.exitCode = 4;
    return;
  }
  if (command === "import") {
    const filePath = args[1];
    if (!filePath || filePath.startsWith("-")) throw new Error("Usage: meshharbor import <file.png|file.jpg|file.glb> [--json]");
    const absolute = path.resolve(filePath);
    const buffer = await readFile(absolute);
    const detected = detectAsset(buffer, path.basename(absolute));
    const store = new Store(dataDirectory);
    try {
      const result = await store.importBuffer({ buffer, name: detected.name, kind: detected.kind, mimeType: detected.mimeType, metadata: detected.metadata });
      return print(jsonOutput ? result : `Imported ${result.asset.name} as ${result.asset.id}`);
    } finally { store.close(); }
  }
  if (command === "simplify") {
    const inputPath = args[1];
    const outputPathArg = readOption("-o") || readOption("--output");
    const target = Number(readOption("--triangles", "4000"));
    if (!inputPath || !outputPathArg || !Number.isInteger(target) || target < 4 || target > 10_000_000) {
      throw new Error("Usage: meshharbor simplify <model.glb> --triangles <count> -o <output.glb>");
    }
    const inputPathAbsolute = path.resolve(inputPath);
    const outputPath = path.resolve(outputPathArg);
    if (inputPathAbsolute === outputPath) throw new Error("The output path must differ from the source so the original remains intact.");
    if (!ensureNativeWorker()) throw new Error("The native simplifier could not be built. Install a C++17 compiler and Make, then retry.");
    const bytes = await readFile(inputPathAbsolute);
    const detected = detectAsset(bytes, path.basename(inputPathAbsolute));
    if (detected.kind !== "mesh") throw new Error("Simplification requires a GLB input.");
    const store = new Store(dataDirectory);
    const manager = new JobManager({ store, projectRoot, workerPath });
    try {
      const imported = await store.importBuffer({ buffer: bytes, name: detected.name, kind: detected.kind, mimeType: detected.mimeType, metadata: detected.metadata });
      const job = manager.create({ operation: "mesh.simplify", input: { asset_id: imported.asset.id }, parameters: { target_triangles: target } });
      let latest = job;
      if (!["succeeded", "failed", "cancelled"].includes(job.state)) {
        await new Promise((resolve, reject) => {
          const timer = setTimeout(() => { manager.cancel(job.id).catch(() => {}); reject(new Error("Mesh simplification timed out.")); }, 15 * 60 * 1000);
          timer.unref?.();
          const listener = event => {
            latest = store.getJob(job.id);
            if (["succeeded", "failed", "cancelled"].includes(latest?.state)) {
              clearTimeout(timer);
              manager.events.off(job.id, listener);
              resolve();
            }
          };
          manager.events.on(job.id, listener);
          setTimeout(() => { latest = store.getJob(job.id); if (["succeeded", "failed", "cancelled"].includes(latest?.state)) { clearTimeout(timer); manager.events.off(job.id, listener); resolve(); } }, 0);
        });
      }
      latest = store.getJob(job.id);
      if (latest.state !== "succeeded") throw Object.assign(new Error(latest.error?.message || "The operation did not complete."), { code: latest.error?.code || "JOB_FAILED" });
      const artifact = store.getArtifact(latest.result.output_artifact_id);
      const outputBytes = await readFile(artifact.file_path);
      await mkdir(path.dirname(outputPath), { recursive: true });
      await writeFile(outputPath, outputBytes, { flag: "wx", mode: 0o600 });
      const result = { ...latest.result, output_path: outputPath };
      return print(jsonOutput ? result : `Wrote ${outputPath}: ${result.input_triangles} → ${result.actual_triangles} triangles (${result.output_asset_id})`);
    } finally { store.close(); }
  }
  return help();
}

try {
  await run();
} catch (error) {
  process.stderr.write(`${error.code ? `${error.code}: ` : ""}${error.message || "MeshHarbor command failed."}\n`);
  process.exitCode = 1;
}

