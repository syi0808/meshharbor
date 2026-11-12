import { createServer } from "node:http";
import { createReadStream, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { InputError, detectAsset, safeFilename } from "./glb.mjs";
import { JobError } from "./jobs.mjs";
import { listModels, listOperations, listRuntimes, runtimeDoctor } from "./operations.mjs";

const MAX_UPLOAD_BYTES = 200 * 1024 * 1024;
const JSON_LIMIT_BYTES = 1024 * 1024;
const STATIC_FILES = new Map([
  ["/", ["index.html", "text/html; charset=utf-8"]],
  ["/studio.css", ["studio.css", "text/css; charset=utf-8"]],
  ["/studio.js", ["studio.js", "text/javascript; charset=utf-8"]]
]);

function sendJson(res, status, value) {
  const body = Buffer.from(JSON.stringify(value));
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": body.length,
    "cache-control": "no-store",
    "x-content-type-options": "nosniff"
  });
  res.end(body);
}

function sendError(res, error) {
  const status = Number.isInteger(error?.status) ? error.status : 500;
  sendJson(res, status, { error: { code: error?.code || "INTERNAL_ERROR", message: status >= 500 ? "The request could not be completed." : error.message } });
}

async function readBytes(req, maxBytes) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > maxBytes) throw new InputError("UPLOAD_TOO_LARGE", `The upload exceeds the ${Math.floor(maxBytes / 1024 / 1024)} MB limit.`, 413);
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, size);
}

async function readJson(req) {
  const contentType = String(req.headers["content-type"] || "").split(";")[0].trim().toLowerCase();
  if (contentType !== "application/json") throw new InputError("INVALID_CONTENT_TYPE", "Requests that use JSON must set Content-Type: application/json.", 415);
  const bytes = await readBytes(req, JSON_LIMIT_BYTES);
  try { return JSON.parse(bytes.toString("utf8")); }
  catch { throw new InputError("INVALID_JSON", "The request body is not valid JSON."); }
}

function isLoopbackAddress(address = "") {
  return address === "127.0.0.1" || address === "::1" || address === "::ffff:127.0.0.1";
}

function streamEvent(res, entry) {
  res.write(`id: ${entry.sequence}\nevent: ${entry.event}\ndata: ${JSON.stringify(entry.data)}\n\n`);
}

function jobEvents(req, res, job, store, jobs, after) {
  res.writeHead(200, {
    "content-type": "text/event-stream; charset=utf-8",
    "cache-control": "no-store",
    "connection": "keep-alive",
    "x-accel-buffering": "no"
  });
  res.write("retry: 1500\n\n");
  for (const event of store.listJobEvents(job.id, after)) streamEvent(res, event);
  const listener = event => streamEvent(res, event);
  jobs.events.on(job.id, listener);
  const heartbeat = setInterval(() => res.write(": keep-alive\n\n"), 15_000);
  heartbeat.unref?.();
  req.once("close", () => {
    clearInterval(heartbeat);
    jobs.events.off(job.id, listener);
  });
}

export function createApiServer({ store, jobs, token, projectRoot, workerPath }) {
  if (!token || typeof token !== "string") throw new Error("A local API token is required.");
  const staticPaths = new Map();
  for (const [urlPath, [filename, mimeType]] of STATIC_FILES) {
    const filePath = path.join(projectRoot, "prototype", "node-shell", "apps", "web", filename);
    staticPaths.set(urlPath, { filePath, mimeType });
  }

  return createServer(async (req, res) => {
    try {
      const host = String(req.headers.host || "");
      if (!/^(localhost|127\.0\.0\.1)(:\d+)?$/i.test(host) || !isLoopbackAddress(req.socket.remoteAddress)) {
        return sendJson(res, 403, { error: { code: "LOCAL_ONLY", message: "MeshHarbor accepts loopback requests only." } });
      }
      const requestUrl = new URL(req.url || "/", `http://${host}`);
      const origin = req.headers.origin;
      if (origin) {
        const expected = new Set([`http://${host}`, `http://localhost${host.includes(":") ? host.slice(host.indexOf(":")) : ""}`, `http://127.0.0.1${host.includes(":") ? host.slice(host.indexOf(":")) : ""}`]);
        if (!expected.has(origin)) return sendJson(res, 403, { error: { code: "ORIGIN_REJECTED", message: "The request origin is not allowed." } });
      }

      const staticFile = staticPaths.get(requestUrl.pathname);
      if (staticFile && req.method === "GET") {
        const body = readFileSync(staticFile.filePath);
        res.writeHead(200, {
          "content-type": staticFile.mimeType,
          "content-length": body.length,
          "cache-control": "no-store",
          "x-content-type-options": "nosniff",
          "content-security-policy": "default-src 'self'; img-src 'self' blob: data:; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
          "referrer-policy": "no-referrer"
        });
        return res.end(body);
      }

      if (req.method === "GET" && requestUrl.pathname === "/v1/health") return sendJson(res, 200, { status: "ok", product: "MeshHarbor", version: "0.1.0" });
      if (req.headers.authorization !== `Bearer ${token}`) return sendJson(res, 401, { error: { code: "AUTH_REQUIRED", message: "Enter the local MeshHarbor token to continue." } });
      if (req.method === "OPTIONS") return sendJson(res, 405, { error: { code: "METHOD_NOT_ALLOWED", message: "Cross-origin requests are not enabled." } });

      if (req.method === "GET" && requestUrl.pathname === "/v1/operations") return sendJson(res, 200, { operations: listOperations() });
      if (req.method === "GET" && requestUrl.pathname === "/v1/models") return sendJson(res, 200, { models: listModels() });
      if (req.method === "GET" && requestUrl.pathname === "/v1/runtimes") return sendJson(res, 200, { runtimes: listRuntimes() });
      if (req.method === "GET" && requestUrl.pathname === "/v1/runtimes/doctor") {
        const { existsSync } = await import("node:fs");
        return sendJson(res, 200, runtimeDoctor({ workerAvailable: existsSync(workerPath || jobs.workerPath) }));
      }

      const installMatch = requestUrl.pathname.match(/^\/v1\/models\/([a-z0-9_-]+)\/install$/);
      if (req.method === "POST" && installMatch) {
        const model = listModels().find(item => item.id === installMatch[1]);
        if (!model) return sendJson(res, 404, { error: { code: "MODEL_NOT_FOUND", message: "The model was not found." } });
        return sendJson(res, 409, { error: { code: "MODEL_PENDING", message: "This model is not installable until its full runtime, dependency, and weight license review is approved." }, model_id: model.id, status: model.status });
      }

      if (req.method === "POST" && requestUrl.pathname === "/v1/assets/import") {
        const bytes = await readBytes(req, MAX_UPLOAD_BYTES);
        const filename = requestUrl.searchParams.get("name") || req.headers["x-file-name"] || "asset";
        const detected = detectAsset(bytes, filename);
        const { asset, artifact } = await store.importBuffer({
          buffer: bytes,
          name: detected.name,
          kind: detected.kind,
          mimeType: detected.mimeType,
          metadata: detected.metadata
        });
        return sendJson(res, 201, { asset, artifact });
      }

      if (req.method === "GET" && requestUrl.pathname === "/v1/assets") return sendJson(res, 200, { assets: store.listAssets(requestUrl.searchParams.get("limit")) });
      const assetArtifactsMatch = requestUrl.pathname.match(/^\/v1\/assets\/(asset_[a-f0-9]+)\/artifacts$/);
      if (req.method === "GET" && assetArtifactsMatch) {
        const asset = store.getAsset(assetArtifactsMatch[1]);
        if (!asset) return sendJson(res, 404, { error: { code: "ASSET_NOT_FOUND", message: "The asset was not found." } });
        return sendJson(res, 200, { asset_id: asset.id, artifacts: asset.artifacts });
      }
      const assetMatch = requestUrl.pathname.match(/^\/v1\/assets\/(asset_[a-f0-9]+)$/);
      if (req.method === "GET" && assetMatch) {
        const asset = store.getAsset(assetMatch[1]);
        return asset ? sendJson(res, 200, asset) : sendJson(res, 404, { error: { code: "ASSET_NOT_FOUND", message: "The asset was not found." } });
      }

      if (req.method === "POST" && requestUrl.pathname === "/v1/jobs") {
        const body = await readJson(req);
        const job = jobs.create({ operation: body.operation, input: body.input, parameters: body.parameters });
        return sendJson(res, 202, { job, status_url: `/v1/jobs/${job.id}`, events_url: `/v1/jobs/${job.id}/events` });
      }
      if (req.method === "GET" && requestUrl.pathname === "/v1/jobs") return sendJson(res, 200, { jobs: store.listJobs(requestUrl.searchParams.get("limit")) });
      const cancelMatch = requestUrl.pathname.match(/^\/v1\/jobs\/(job_[a-f0-9]+)\/cancel$/);
      if (req.method === "POST" && cancelMatch) return sendJson(res, 200, { job: await jobs.cancel(cancelMatch[1]) });
      const eventsMatch = requestUrl.pathname.match(/^\/v1\/jobs\/(job_[a-f0-9]+)\/events$/);
      if (req.method === "GET" && eventsMatch) {
        const job = store.getJob(eventsMatch[1]);
        if (!job) return sendJson(res, 404, { error: { code: "JOB_NOT_FOUND", message: "The job was not found." } });
        const after = Number(req.headers["last-event-id"] || requestUrl.searchParams.get("after") || 0);
        return jobEvents(req, res, job, store, jobs, Number.isFinite(after) ? after : 0);
      }
      const jobMatch = requestUrl.pathname.match(/^\/v1\/jobs\/(job_[a-f0-9]+)$/);
      if (req.method === "GET" && jobMatch) {
        const job = store.getJob(jobMatch[1]);
        return job ? sendJson(res, 200, { job }) : sendJson(res, 404, { error: { code: "JOB_NOT_FOUND", message: "The job was not found." } });
      }

      const artifactMatch = requestUrl.pathname.match(/^\/v1\/artifacts\/(artifact_[a-f0-9]+)\/content$/);
      if (req.method === "GET" && artifactMatch) {
        const artifact = store.getArtifact(artifactMatch[1]);
        if (!artifact || !statSync(artifact.file_path, { throwIfNoEntry: false })) return sendJson(res, 404, { error: { code: "ARTIFACT_NOT_FOUND", message: "The artifact was not found." } });
        const safeName = safeFilename(artifact.name).replaceAll('"', "_");
        res.writeHead(200, {
          "content-type": artifact.mime_type,
          "content-length": artifact.size_bytes,
          "content-disposition": `attachment; filename="${safeName}"`,
          "cache-control": "no-store",
          "x-content-type-options": "nosniff"
        });
        return createReadStream(artifact.file_path).pipe(res);
      }

      return sendJson(res, 404, { error: { code: "NOT_FOUND", message: "The requested endpoint was not found." } });
    } catch (error) {
      if (error instanceof InputError || error instanceof JobError) return sendError(res, error);
      if (!res.headersSent) return sendError(res, error);
      res.destroy(error);
    }
  });
}

