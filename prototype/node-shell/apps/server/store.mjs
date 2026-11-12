import { DatabaseSync } from "node:sqlite";
import { createHash, randomUUID } from "node:crypto";
import { mkdirSync, writeFileSync, existsSync, readFileSync, unlinkSync } from "node:fs";
import path from "node:path";
import { safeFilename } from "./glb.mjs";

const now = () => new Date().toISOString();
const newId = prefix => `${prefix}_${randomUUID().replaceAll("-", "")}`;
const json = value => JSON.stringify(value ?? {});
const parse = value => {
  try { return JSON.parse(value || "{}"); } catch { return {}; }
};

export class Store {
  constructor(dataDirectory) {
    this.root = path.resolve(dataDirectory);
    this.artifactRoot = path.join(this.root, "artifacts");
    this.tempRoot = path.join(this.root, "tmp");
    mkdirSync(this.artifactRoot, { recursive: true, mode: 0o700 });
    mkdirSync(this.tempRoot, { recursive: true, mode: 0o700 });
    const databasePath = path.join(this.root, "meshharbor.sqlite");
    this.db = new DatabaseSync(databasePath);
    this.db.exec("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;");
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS assets (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        revision TEXT NOT NULL,
        source_asset_id TEXT,
        metadata_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        FOREIGN KEY(source_asset_id) REFERENCES assets(id)
      );
      CREATE INDEX IF NOT EXISTS assets_created_at_idx ON assets(created_at DESC);
      CREATE TABLE IF NOT EXISTS artifacts (
        id TEXT PRIMARY KEY,
        asset_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        mime_type TEXT NOT NULL,
        relative_path TEXT NOT NULL UNIQUE,
        sha256 TEXT NOT NULL,
        size_bytes INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        FOREIGN KEY(asset_id) REFERENCES assets(id) ON DELETE CASCADE
      );
      CREATE INDEX IF NOT EXISTS artifacts_asset_id_idx ON artifacts(asset_id, created_at);
      CREATE TABLE IF NOT EXISTS jobs (
        id TEXT PRIMARY KEY,
        operation TEXT NOT NULL,
        state TEXT NOT NULL,
        input_json TEXT NOT NULL,
        parameters_json TEXT NOT NULL,
        result_json TEXT NOT NULL,
        error_json TEXT NOT NULL,
        warnings_json TEXT NOT NULL,
        cancel_requested INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS jobs_created_at_idx ON jobs(created_at DESC);
      CREATE TABLE IF NOT EXISTS job_events (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        job_id TEXT NOT NULL,
        event TEXT NOT NULL,
        data_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        FOREIGN KEY(job_id) REFERENCES jobs(id) ON DELETE CASCADE
      );
      CREATE INDEX IF NOT EXISTS job_events_job_id_idx ON job_events(job_id, sequence);
    `);
    try { readFileSync(databasePath); } catch { /* The database was just created. */ }
  }

  close() {
    this.db.close();
  }

  async importBuffer({ buffer, name, kind, mimeType, metadata = {} }) {
    const assetId = newId("asset");
    const revision = newId("rev");
    const artifactId = newId("artifact");
    const createdAt = now();
    const safeName = safeFilename(name);
    const suffix = path.extname(safeName).slice(0, 12);
    const artifactName = `source${suffix}`;
    const relativePath = path.join("artifacts", assetId, `${artifactId}-${artifactName}`);
    const absolutePath = path.join(this.root, relativePath);
    mkdirSync(path.dirname(absolutePath), { recursive: true, mode: 0o700 });
    const bytes = Buffer.from(buffer);
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    writeFileSync(absolutePath, bytes, { flag: "wx", mode: 0o600 });
    try {
      this.db.exec("BEGIN IMMEDIATE;");
      this.db.prepare(`INSERT INTO assets(id,kind,name,revision,source_asset_id,metadata_json,created_at)
        VALUES(?,?,?,?,?,?,?)`).run(assetId, kind, safeName, revision, null, json(metadata), createdAt);
      this.db.prepare(`INSERT INTO artifacts(id,asset_id,kind,name,mime_type,relative_path,sha256,size_bytes,created_at)
        VALUES(?,?,?,?,?,?,?,?,?)`).run(artifactId, assetId, "source", artifactName, mimeType, relativePath, sha256, bytes.length, createdAt);
      this.db.exec("COMMIT;");
    } catch (error) {
      this.db.exec("ROLLBACK;");
      if (existsSync(absolutePath)) unlinkSync(absolutePath);
      throw error;
    }
    return { asset: this.getAsset(assetId), artifact: this.getArtifact(artifactId) };
  }

  async createDerivedAsset({ sourceAssetId, name, kind, metadata = {}, artifacts }) {
    if (!this.getAsset(sourceAssetId)) throw new Error("Source asset no longer exists.");
    if (!Array.isArray(artifacts) || artifacts.length === 0) throw new Error("A derived asset needs at least one artifact.");
    const assetId = newId("asset");
    const revision = newId("rev");
    const createdAt = now();
    const safeName = safeFilename(name);
    const prepared = [];
    for (const artifact of artifacts) {
      const artifactId = newId("artifact");
      const artifactName = safeFilename(artifact.name);
      const relativePath = path.join("artifacts", assetId, `${artifactId}-${artifactName}`);
      const absolutePath = path.join(this.root, relativePath);
      mkdirSync(path.dirname(absolutePath), { recursive: true, mode: 0o700 });
      const bytes = Buffer.from(artifact.buffer);
      writeFileSync(absolutePath, bytes, { flag: "wx", mode: 0o600 });
      prepared.push({
        id: artifactId,
        kind: artifact.kind || "result",
        name: artifactName,
        mimeType: artifact.mimeType || "application/octet-stream",
        relativePath,
        sha256: createHash("sha256").update(bytes).digest("hex"),
        sizeBytes: bytes.length,
        absolutePath
      });
    }
    try {
      this.db.exec("BEGIN IMMEDIATE;");
      this.db.prepare(`INSERT INTO assets(id,kind,name,revision,source_asset_id,metadata_json,created_at)
        VALUES(?,?,?,?,?,?,?)`).run(assetId, kind, safeName, revision, sourceAssetId, json(metadata), createdAt);
      const insert = this.db.prepare(`INSERT INTO artifacts(id,asset_id,kind,name,mime_type,relative_path,sha256,size_bytes,created_at)
        VALUES(?,?,?,?,?,?,?,?,?)`);
      for (const artifact of prepared) insert.run(artifact.id, assetId, artifact.kind, artifact.name, artifact.mimeType, artifact.relativePath, artifact.sha256, artifact.sizeBytes, createdAt);
      this.db.exec("COMMIT;");
    } catch (error) {
      this.db.exec("ROLLBACK;");
      for (const artifact of prepared) if (existsSync(artifact.absolutePath)) unlinkSync(artifact.absolutePath);
      throw error;
    }
    return { asset: this.getAsset(assetId), artifacts: prepared.map(item => this.getArtifact(item.id)) };
  }

  getAsset(assetId) {
    const row = this.db.prepare("SELECT * FROM assets WHERE id = ?").get(assetId);
    if (!row) return null;
    return {
      id: row.id,
      kind: row.kind,
      name: row.name,
      revision: row.revision,
      source_asset_id: row.source_asset_id,
      created_at: row.created_at,
      metadata: parse(row.metadata_json),
      artifacts: this.listArtifacts(row.id)
    };
  }

  listAssets(limit = 100) {
    const bounded = Math.max(1, Math.min(Number(limit) || 100, 200));
    return this.db.prepare("SELECT id FROM assets ORDER BY created_at DESC LIMIT ?").all(bounded).map(row => this.getAsset(row.id));
  }

  listArtifacts(assetId) {
    return this.db.prepare("SELECT id,asset_id,kind,name,mime_type,sha256,size_bytes,created_at FROM artifacts WHERE asset_id = ? ORDER BY created_at ASC")
      .all(assetId).map(row => this.publicArtifact(row));
  }

  getArtifact(artifactId) {
    const row = this.db.prepare("SELECT * FROM artifacts WHERE id = ?").get(artifactId);
    return row ? { ...this.publicArtifact(row), file_path: path.join(this.root, row.relative_path) } : null;
  }

  publicArtifact(row) {
    return {
      id: row.id,
      asset_id: row.asset_id,
      kind: row.kind,
      name: row.name,
      mime_type: row.mime_type,
      sha256: row.sha256,
      size_bytes: row.size_bytes,
      created_at: row.created_at
    };
  }

  createJob({ operation, input = {}, parameters = {} }) {
    const id = newId("job");
    const createdAt = now();
    this.db.prepare(`INSERT INTO jobs(id,operation,state,input_json,parameters_json,result_json,error_json,warnings_json,cancel_requested,created_at,updated_at)
      VALUES(?,?,?,?,?,?,?,?,?,?,?)`).run(id, operation, "queued", json(input), json(parameters), "{}", "{}", "[]", 0, createdAt, createdAt);
    this.addJobEvent(id, "state", { state: "queued" });
    return this.getJob(id);
  }

  getJob(jobId) {
    const row = this.db.prepare("SELECT * FROM jobs WHERE id = ?").get(jobId);
    if (!row) return null;
    return {
      id: row.id,
      operation: row.operation,
      state: row.state,
      input: parse(row.input_json),
      parameters: parse(row.parameters_json),
      result: parse(row.result_json),
      error: parse(row.error_json),
      warnings: parse(row.warnings_json),
      cancel_requested: Boolean(row.cancel_requested),
      created_at: row.created_at,
      updated_at: row.updated_at
    };
  }

  listJobs(limit = 50) {
    const bounded = Math.max(1, Math.min(Number(limit) || 50, 200));
    return this.db.prepare("SELECT id FROM jobs ORDER BY created_at DESC LIMIT ?").all(bounded).map(row => this.getJob(row.id));
  }

  updateJob(jobId, patch) {
    const current = this.getJob(jobId);
    if (!current) return null;
    const next = { ...current, ...patch, updated_at: now() };
    this.db.prepare(`UPDATE jobs SET state=?,result_json=?,error_json=?,warnings_json=?,cancel_requested=?,updated_at=? WHERE id=?`)
      .run(next.state, json(next.result), json(next.error), json(next.warnings), next.cancel_requested ? 1 : 0, next.updated_at, jobId);
    return this.getJob(jobId);
  }

  requestJobCancel(jobId) {
    this.db.prepare("UPDATE jobs SET cancel_requested=1,updated_at=? WHERE id=?").run(now(), jobId);
    return this.getJob(jobId);
  }

  addJobEvent(jobId, event, data = {}) {
    const createdAt = now();
    const result = this.db.prepare("INSERT INTO job_events(job_id,event,data_json,created_at) VALUES(?,?,?,?)").run(jobId, event, json(data), createdAt);
    return { sequence: Number(result.lastInsertRowid), job_id: jobId, event, data, created_at: createdAt };
  }

  listJobEvents(jobId, after = 0, limit = 100) {
    const safeAfter = Math.max(0, Number(after) || 0);
    const safeLimit = Math.max(1, Math.min(Number(limit) || 100, 500));
    return this.db.prepare("SELECT sequence,job_id,event,data_json,created_at FROM job_events WHERE job_id=? AND sequence>? ORDER BY sequence LIMIT ?")
      .all(jobId, safeAfter, safeLimit).map(row => ({ sequence: row.sequence, job_id: row.job_id, event: row.event, data: parse(row.data_json), created_at: row.created_at }));
  }
}

