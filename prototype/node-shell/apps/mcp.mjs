import { createInterface } from "node:readline";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { JobManager } from "./server/jobs.mjs";
import { Store } from "./server/store.mjs";
import { listModels, listOperations, listRuntimes, runtimeDoctor } from "./server/operations.mjs";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const workerPath = path.join(projectRoot, "native/mesh-refiner/glb_refine");
const store = new Store(path.resolve(process.env.MESHHARBOR_HOME || path.join(homedir(), ".meshharbor")));
const jobs = new JobManager({ store, projectRoot, workerPath });

const jsonText = value => JSON.stringify(value, null, 2);
const tools = [
  { name: "meshharbor.list_operations", description: "List operations and their current support status.", inputSchema: { type: "object", properties: {}, additionalProperties: false } },
  { name: "meshharbor.list_models", description: "List model candidates, license status, and runtime blockers.", inputSchema: { type: "object", properties: {}, additionalProperties: false } },
  { name: "meshharbor.runtime_doctor", description: "Report the local MeshHarbor worker and runtime status.", inputSchema: { type: "object", properties: {}, additionalProperties: false } },
  { name: "meshharbor.list_assets", description: "List imported assets and derived results.", inputSchema: { type: "object", properties: { limit: { type: "integer", minimum: 1, maximum: 200 } }, additionalProperties: false } },
  { name: "meshharbor.get_asset", description: "Get asset metadata and artifact IDs without exposing local paths.", inputSchema: { type: "object", properties: { asset_id: { type: "string" } }, required: ["asset_id"], additionalProperties: false } },
  { name: "meshharbor.simplify", description: "Start a background GLB simplification job for an imported asset.", inputSchema: { type: "object", properties: { asset_id: { type: "string" }, target_triangles: { type: "integer", minimum: 4, maximum: 10000000 } }, required: ["asset_id"], additionalProperties: false } },
  { name: "meshharbor.get_job", description: "Read the current state, result, warnings, or failure for a job.", inputSchema: { type: "object", properties: { job_id: { type: "string" } }, required: ["job_id"], additionalProperties: false } },
  { name: "meshharbor.cancel_job", description: "Cancel a queued or running MeshHarbor job.", inputSchema: { type: "object", properties: { job_id: { type: "string" } }, required: ["job_id"], additionalProperties: false } }
];

function response(id, result) {
  return { jsonrpc: "2.0", id, result };
}

function errorResponse(id, code, message) {
  return { jsonrpc: "2.0", id, error: { code, message } };
}

async function callTool(name, args) {
  switch (name) {
    case "meshharbor.list_operations": return { content: [{ type: "text", text: jsonText({ operations: listOperations() }) }] };
    case "meshharbor.list_models": return { content: [{ type: "text", text: jsonText({ models: listModels() }) }] };
    case "meshharbor.runtime_doctor": return { content: [{ type: "text", text: jsonText(runtimeDoctor({ workerAvailable: existsSync(workerPath) })) }] };
    case "meshharbor.list_assets": return { content: [{ type: "text", text: jsonText({ assets: store.listAssets(args.limit) }) }] };
    case "meshharbor.get_asset": {
      const asset = store.getAsset(args.asset_id);
      if (!asset) throw Object.assign(new Error("Asset not found."), { code: "ASSET_NOT_FOUND" });
      return { content: [{ type: "text", text: jsonText(asset) }] };
    }
    case "meshharbor.simplify": {
      const job = jobs.create({ operation: "mesh.simplify", input: { asset_id: args.asset_id }, parameters: { target_triangles: args.target_triangles ?? 4000 } });
      return { content: [{ type: "text", text: jsonText({ job_id: job.id, state: job.state, status_tool: "meshharbor.get_job" }) }] };
    }
    case "meshharbor.get_job": {
      const job = store.getJob(args.job_id);
      if (!job) throw Object.assign(new Error("Job not found."), { code: "JOB_NOT_FOUND" });
      return { content: [{ type: "text", text: jsonText(job) }] };
    }
    case "meshharbor.cancel_job": {
      const job = await jobs.cancel(args.job_id);
      return { content: [{ type: "text", text: jsonText(job) }] };
    }
    default: throw Object.assign(new Error(`Unknown tool: ${name}`), { code: "UNKNOWN_TOOL" });
  }
}

async function handle(message) {
  const id = message?.id;
  if (message?.jsonrpc !== "2.0" || typeof message.method !== "string") {
    if (id !== undefined) return errorResponse(id, -32600, "Invalid JSON-RPC request.");
    return null;
  }
  if (message.method === "notifications/initialized") return null;
  if (message.method === "ping") return response(id, {});
  if (message.method === "initialize") {
    return response(id, {
      protocolVersion: message.params?.protocolVersion || "2024-11-05",
      capabilities: { tools: { listChanged: false } },
      serverInfo: { name: "meshharbor", version: "0.1.0" }
    });
  }
  if (message.method === "tools/list") return response(id, { tools });
  if (message.method === "tools/call") {
    try {
      const result = await callTool(message.params?.name, message.params?.arguments || {});
      return response(id, result);
    } catch (error) {
      return response(id, { isError: true, content: [{ type: "text", text: `${error.code ? `${error.code}: ` : ""}${error.message || "Tool failed."}` }] });
    }
  }
  if (id === undefined) return null;
  return errorResponse(id, -32601, "Method not found.");
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity, terminal: false });
for await (const line of lines) {
  if (!line.trim()) continue;
  try {
    const message = JSON.parse(line);
    const result = await handle(message);
    if (result !== null) process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    process.stderr.write(`MeshHarbor MCP message error: ${error.message}\n`);
  }
}
store.close();
