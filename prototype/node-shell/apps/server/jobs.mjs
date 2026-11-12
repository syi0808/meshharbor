import { EventEmitter } from "node:events";
import { spawn } from "node:child_process";
import { copyFile, mkdtemp, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { getOperation } from "./operations.mjs";
import { InputError, inspectGlb } from "./glb.mjs";

const terminal = new Set(["succeeded", "failed", "cancelled"]);
const transitions = {
  queued: new Set(["preparing", "cancelling", "cancelled", "failed"]),
  preparing: new Set(["running", "cancelling", "cancelled", "failed"]),
  running: new Set(["validating", "cancelling", "cancelled", "failed"]),
  validating: new Set(["succeeded", "cancelling", "cancelled", "failed"]),
  cancelling: new Set(["cancelled", "failed"])
};

export class JobError extends Error {
  constructor(code, message, status = 400) {
    super(message);
    this.name = "JobError";
    this.code = code;
    this.status = status;
  }
}

export class JobManager {
  constructor({ store, projectRoot, workerPath, timeoutMs = 15 * 60 * 1000 }) {
    this.store = store;
    this.projectRoot = path.resolve(projectRoot);
    this.workerPath = workerPath || path.join(this.projectRoot, "native/mesh-refiner/glb_refine");
    this.timeoutMs = timeoutMs;
    this.events = new EventEmitter();
    this.events.setMaxListeners(0);
    this.active = new Map();
  }

  create({ operation, input = {}, parameters = {} }) {
    const definition = getOperation(operation);
    if (!definition) throw new JobError("UNKNOWN_OPERATION", `Unknown operation: ${operation}.`, 404);
    if (definition.state === "pending") throw new JobError("OPERATION_PENDING", definition.blocked_reason || "This operation is not enabled.", 409);
    if (operation !== "mesh.simplify") throw new JobError("OPERATION_UNAVAILABLE", "Only mesh.simplify can execute in this prototype.", 409);
    if (typeof input.asset_id !== "string" || !input.asset_id.startsWith("asset_")) throw new JobError("INVALID_INPUT", "input.asset_id must identify an imported asset.");
    const target = Number(parameters.target_triangles ?? 4000);
    if (!Number.isInteger(target) || target < 4 || target > 10_000_000) throw new JobError("INVALID_PARAMETERS", "target_triangles must be an integer between 4 and 10,000,000.");
    if (!this.store.getAsset(input.asset_id)) throw new JobError("ASSET_NOT_FOUND", "The input asset was not found.", 404);
    const job = this.store.createJob({ operation, input: { asset_id: input.asset_id }, parameters: { target_triangles: target } });
    setImmediate(() => this.execute(job.id));
    return job;
  }

  emit(jobId, event, data) {
    const stored = this.store.addJobEvent(jobId, event, data);
    this.events.emit(jobId, stored);
    return stored;
  }

  setState(jobId, state, data = {}) {
    const job = this.store.getJob(jobId);
    if (!job) throw new JobError("JOB_NOT_FOUND", "The job was not found.", 404);
    if (job.state !== state && !transitions[job.state]?.has(state)) throw new Error(`Invalid job transition: ${job.state} -> ${state}`);
    const updated = this.store.updateJob(jobId, { state });
    this.emit(jobId, "state", { state, ...data });
    return updated;
  }

  isCancelled(jobId) {
    return Boolean(this.store.getJob(jobId)?.cancel_requested);
  }

  async cancel(jobId) {
    const job = this.store.getJob(jobId);
    if (!job) throw new JobError("JOB_NOT_FOUND", "The job was not found.", 404);
    if (terminal.has(job.state)) return job;
    this.store.requestJobCancel(jobId);
    if (job.state === "queued") {
      return this.setState(jobId, "cancelled", { reason: "cancelled_before_start" });
    }
    if (job.state !== "cancelling") this.setState(jobId, "cancelling", { reason: "user_requested" });
    const child = this.active.get(jobId);
    if (child && !child.killed) {
      child.kill("SIGTERM");
      const forceTimer = setTimeout(() => child.kill("SIGKILL"), 5000);
      forceTimer.unref?.();
    }
    return this.store.getJob(jobId);
  }

  async execute(jobId) {
    const startedAt = Date.now();
    let workDir = null;
    try {
      if (this.isCancelled(jobId)) return this.setState(jobId, "cancelled", { reason: "cancelled_before_start" });
      this.setState(jobId, "preparing");
      const job = this.store.getJob(jobId);
      const source = this.store.getAsset(job.input.asset_id);
      if (!source || source.kind !== "mesh") throw new JobError("INVALID_ASSET_KIND", "mesh.simplify requires a GLB mesh asset.", 415);
      const inputArtifact = source.artifacts.find(item => item.kind === "source" || item.kind === "result");
      if (!inputArtifact) throw new JobError("MISSING_ARTIFACT", "The source asset has no readable GLB artifact.", 422);
      const inputRecord = this.store.getArtifact(inputArtifact.id);
      const inputBytes = await readFile(inputRecord.file_path);
      const inputInfo = inspectGlb(inputBytes);
      if (!inputInfo.support.supported) throw new JobError("UNSUPPORTED_INPUT", inputInfo.support.reason, 415);
      if (this.isCancelled(jobId)) return this.setState(jobId, "cancelled", { reason: "cancelled_before_worker" });
      workDir = await mkdtemp(path.join(this.store.tempRoot, `${jobId}-`));
      const inputPath = path.join(workDir, "input.glb");
      const outputPath = path.join(workDir, "output.glb");
      const reportPath = path.join(workDir, "components.tsv");
      const provenancePath = `${outputPath}.provenance.csv`;
      await copyFile(inputRecord.file_path, inputPath);
      if (this.isCancelled(jobId)) return this.setState(jobId, "cancelled", { reason: "cancelled_before_worker" });
      this.setState(jobId, "running", { progress: 0.05, step: "native_worker" });
      const target = job.parameters.target_triangles;
      const output = await this.runWorker(jobId, [inputPath, outputPath, String(target), reportPath, "0.004", "1.25", "0.8660254"]);
      if (this.isCancelled(jobId)) return this.setState(jobId, "cancelled", { reason: "user_requested" });
      this.setState(jobId, "validating", { progress: 0.9, step: "output_validation" });
      const outputBytes = await readFile(outputPath);
      const outputInfo = inspectGlb(outputBytes);
      if (outputInfo.triangles < 1) throw new JobError("INVALID_OUTPUT", "The worker produced a GLB with no triangles.", 500);
      const reportBytes = await readFile(reportPath).catch(() => Buffer.from("No component report was produced.\n"));
      const provenanceBytes = await readFile(provenancePath).catch(() => Buffer.from("output_face,source_face\n"));
      const warnings = [];
      if (outputInfo.triangles > target) warnings.push(`The requested target was ${target}, but preservation constraints retained ${outputInfo.triangles} triangles.`);
      const durationMs = Date.now() - startedAt;
      const derived = await this.store.createDerivedAsset({
        sourceAssetId: source.id,
        name: `simplified-${source.name}`,
        kind: "mesh",
        metadata: {
          operation: "mesh.simplify",
          input_asset_id: source.id,
          input_revision: source.revision,
          requested_parameters: job.parameters,
          applied_parameters: { target_triangles: target, envelope_limit: 0.004, uv_stretch_limit: 1.25, source_normal_cos: 0.8660254 },
          input_geometry: { triangles: inputInfo.triangles, vertices: inputInfo.vertices },
          output_geometry: { triangles: outputInfo.triangles, vertices: outputInfo.vertices },
          worker_stdout: output.stdout.trim(),
          duration_ms: durationMs,
          preserved: ["source asset", "embedded GLB JSON", "supported material records", "face-corner UV coordinates", "normals"],
          warnings
        },
        artifacts: [
          { kind: "result", name: `simplified${path.extname(source.name).toLowerCase() === ".glb" ? ".glb" : ".glb"}`, mimeType: "model/gltf-binary", buffer: outputBytes },
          { kind: "report", name: "components.tsv", mimeType: "text/tab-separated-values", buffer: reportBytes },
          { kind: "provenance", name: "provenance.csv", mimeType: "text/csv", buffer: provenanceBytes }
        ]
      });
      const result = {
        input_asset_id: source.id,
        output_asset_id: derived.asset.id,
        output_artifact_id: derived.artifacts.find(item => item.kind === "result")?.id,
        report_artifact_ids: derived.artifacts.filter(item => item.kind !== "result").map(item => item.id),
        requested_triangles: target,
        actual_triangles: outputInfo.triangles,
        input_triangles: inputInfo.triangles,
        duration_ms: durationMs
      };
      this.store.updateJob(jobId, { result, warnings, error: {} });
      this.setState(jobId, "succeeded", { progress: 1, result });
    } catch (error) {
      const current = this.store.getJob(jobId);
      if (!current || terminal.has(current.state)) return;
      if (this.isCancelled(jobId) || current.state === "cancelling") {
        this.setState(jobId, "cancelled", { reason: "user_requested" });
        return;
      }
      const message = String(error?.message || "The operation failed.").replaceAll(workDir || "\0", "<job-temp>");
      const errorData = { code: error.code || "JOB_FAILED", message };
      this.store.updateJob(jobId, { error: errorData });
      this.setState(jobId, "failed", { error: errorData });
    } finally {
      this.active.delete(jobId);
      if (workDir) await rm(workDir, { recursive: true, force: true }).catch(() => {});
    }
  }

  runWorker(jobId, args) {
    return new Promise((resolve, reject) => {
      const child = spawn(this.workerPath, args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
      this.active.set(jobId, child);
      let stdout = "";
      let stderr = "";
      let timedOut = false;
      child.stdout.on("data", chunk => { if (stdout.length < 32_000) stdout += chunk.toString("utf8").slice(0, 32_000 - stdout.length); });
      child.stderr.on("data", chunk => { if (stderr.length < 32_000) stderr += chunk.toString("utf8").slice(0, 32_000 - stderr.length); });
      const timer = setTimeout(() => {
        timedOut = true;
        child.kill("SIGTERM");
        const forceTimer = setTimeout(() => child.kill("SIGKILL"), 5000);
        forceTimer.unref?.();
      }, this.timeoutMs);
      timer.unref?.();
      child.once("error", error => {
        clearTimeout(timer);
        this.active.delete(jobId);
        reject(new JobError("WORKER_START_FAILED", error.code === "ENOENT" ? "The native simplifier is not built. Run the project build step first." : `The native simplifier could not start: ${error.message}`, 503));
      });
      child.once("close", (code, signal) => {
        clearTimeout(timer);
        this.active.delete(jobId);
        if (timedOut) return reject(new JobError("WORKER_TIMEOUT", "Mesh simplification exceeded the configured time limit.", 504));
        if (this.isCancelled(jobId)) return reject(new JobError("CANCELLED", "The operation was cancelled."));
        if (code !== 0) {
          const safeMessage = (stderr.trim() || `Native worker exited with ${signal || code}.`).slice(0, 2000);
          return reject(new JobError("WORKER_FAILED", safeMessage, 422));
        }
        resolve({ stdout, stderr });
      });
    });
  }
}

