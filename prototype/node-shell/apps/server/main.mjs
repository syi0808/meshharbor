import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createApiServer } from "./api.mjs";
import { JobManager } from "./jobs.mjs";
import { Store } from "./store.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../../..");
const workerPath = path.join(projectRoot, "native/mesh-refiner/glb_refine");

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

const host = option("--host", "127.0.0.1");
const port = Number(option("--port", process.env.MESHHARBOR_PORT || "8787"));
if (host !== "127.0.0.1" && host !== "localhost") {
  console.error("External interfaces are disabled until TLS and explicit remote-access controls are implemented.");
  process.exit(2);
}
if (!Number.isInteger(port) || port < 0 || port > 65535) {
  console.error("--port must be an integer between 0 and 65535.");
  process.exit(2);
}

if (!existsSync(workerPath)) {
  const build = spawnSync("make", ["-C", path.dirname(workerPath), "glb_refine"], { stdio: "inherit" });
  if (build.error || build.status !== 0) console.warn("Native simplifier is not built; the REST/API shell will start with simplification disabled.");
}

const dataDirectory = path.resolve(process.env.MESHHARBOR_HOME || path.join(homedir(), ".meshharbor"));
const token = process.env.MESHHARBOR_TOKEN || randomBytes(32).toString("hex");
const store = new Store(dataDirectory);
const jobs = new JobManager({ store, projectRoot, workerPath });
const server = createApiServer({ store, jobs, token, projectRoot, workerPath });
server.listen(port, "127.0.0.1", () => {
  const address = server.address();
  console.log(`MeshHarbor Studio: http://127.0.0.1:${address.port}`);
  console.log(`Local API token: ${token}`);
  console.log(`Data directory: ${dataDirectory}`);
  console.log("The local API accepts loopback connections only.");
});

const shutdown = () => server.close(() => {
  store.close();
  process.exit(0);
});
process.once("SIGINT", shutdown);
process.once("SIGTERM", shutdown);

