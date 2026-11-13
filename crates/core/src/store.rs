use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::inspect_asset;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid request: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("operation unavailable: {0}")]
    OperationUnavailable(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("job cancelled")]
    Cancelled,
    #[error("local storage error: {0}")]
    Io(#[from] std::io::Error),
    #[error("metadata database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("JSON encoding error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("system clock error")]
    Clock,
}

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Mesh,
}

impl AssetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Mesh => "mesh",
        }
    }

    fn from_str(value: &str) -> CoreResult<Self> {
        match value {
            "image" => Ok(Self::Image),
            "mesh" => Ok(Self::Mesh),
            _ => Err(CoreError::InvalidInput(format!(
                "Unknown asset kind '{value}'."
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub sha256: String,
    pub byte_size: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub kind: AssetKind,
    pub mime_type: String,
    pub sha256: String,
    pub byte_size: u64,
    pub source_asset_id: Option<String>,
    pub metadata: Value,
    pub created_at: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Validating,
    Succeeded,
    Failed,
    Cancelling,
    Cancelled,
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Validating => "validating",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "validating" => Ok(Self::Validating),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelling" => Ok(Self::Cancelling),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(CoreError::InvalidInput(format!(
                "Unknown job state '{value}'."
            ))),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub operation: String,
    pub state: JobState,
    pub input_asset_id: Option<String>,
    pub parameters: Value,
    pub progress: u8,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactContent {
    pub artifact: Artifact,
    pub bytes: Vec<u8>,
}

pub struct Store {
    root: PathBuf,
    artifact_root: PathBuf,
    staging_root: PathBuf,
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> CoreResult<Self> {
        let root = root.as_ref().to_path_buf();
        let artifact_root = root.join("artifacts");
        let staging_root = root.join("staging");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(&staging_root)?;
        let connection = Connection::open(root.join("meshharbor.sqlite3"))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS assets (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK(kind IN ('image', 'mesh')),
                mime_type TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                byte_size INTEGER NOT NULL CHECK(byte_size >= 0),
                source_asset_id TEXT REFERENCES assets(id) ON DELETE RESTRICT,
                metadata_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS assets_created_at_idx ON assets(created_at DESC);
            CREATE TABLE IF NOT EXISTS artifacts (
                id TEXT PRIMARY KEY,
                asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE RESTRICT,
                kind TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                byte_size INTEGER NOT NULL CHECK(byte_size >= 0),
                mime_type TEXT NOT NULL,
                storage_path TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS artifacts_asset_idx ON artifacts(asset_id, created_at);
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                operation TEXT NOT NULL,
                state TEXT NOT NULL,
                input_asset_id TEXT REFERENCES assets(id) ON DELETE RESTRICT,
                parameters_json TEXT NOT NULL,
                progress INTEGER NOT NULL DEFAULT 0 CHECK(progress BETWEEN 0 AND 100),
                result_json TEXT,
                error_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS jobs_created_at_idx ON jobs(created_at DESC);",
        )?;
        Ok(Self {
            root,
            artifact_root,
            staging_root,
            connection: Mutex::new(connection),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    pub fn import_asset(&self, name: &str, bytes: &[u8]) -> CoreResult<Asset> {
        self.import_with_metadata(name, bytes, None, "source", Value::Null)
    }

    pub fn import_derived_asset(
        &self,
        name: &str,
        bytes: &[u8],
        source_asset_id: &str,
        metadata: Value,
    ) -> CoreResult<Asset> {
        self.import_with_metadata(name, bytes, Some(source_asset_id), "derived", metadata)
    }

    /// Import a derived asset and its secondary artifacts in the same database transaction.
    /// This prevents an output preview from being visible when its primary editable artifact
    /// (for example, the original QuadriFlow OBJ) could not be committed.
    pub fn import_derived_asset_with_artifacts(
        &self,
        name: &str,
        bytes: &[u8],
        source_asset_id: &str,
        metadata: Value,
        additional_artifacts: &[(&str, &str, &[u8])],
    ) -> CoreResult<Asset> {
        self.import_with_metadata_and_artifacts(
            name,
            bytes,
            Some(source_asset_id),
            "derived",
            metadata,
            additional_artifacts,
        )
    }

    fn import_with_metadata(
        &self,
        name: &str,
        bytes: &[u8],
        source_asset_id: Option<&str>,
        artifact_kind: &str,
        extra_metadata: Value,
    ) -> CoreResult<Asset> {
        self.import_with_metadata_and_artifacts(
            name,
            bytes,
            source_asset_id,
            artifact_kind,
            extra_metadata,
            &[],
        )
    }

    fn import_with_metadata_and_artifacts(
        &self,
        name: &str,
        bytes: &[u8],
        source_asset_id: Option<&str>,
        artifact_kind: &str,
        extra_metadata: Value,
        additional_artifacts: &[(&str, &str, &[u8])],
    ) -> CoreResult<Asset> {
        if bytes.is_empty() {
            return Err(CoreError::InvalidInput("Asset file is empty.".into()));
        }
        let safe_name = sanitize_filename(name)?;
        let descriptor = inspect_asset(&safe_name, bytes)?;
        let mut metadata = descriptor.metadata;
        if let (Some(base), Some(extra)) = (metadata.as_object_mut(), extra_metadata.as_object()) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        let digest = digest(bytes);
        let artifact_path = self.artifact_path_for_hash(&digest);
        self.write_content_addressed(&artifact_path, bytes)?;

        let mut additional_rows = Vec::with_capacity(additional_artifacts.len());
        for (kind, mime_type, extra_bytes) in additional_artifacts {
            if kind.trim().is_empty() || mime_type.trim().is_empty() || extra_bytes.is_empty() {
                return Err(CoreError::InvalidInput(
                    "Secondary artifacts require nonempty kind, MIME type, and bytes.".into(),
                ));
            }
            let extra_digest = digest(extra_bytes);
            let extra_path = self.artifact_path_for_hash(&extra_digest);
            self.write_content_addressed(&extra_path, extra_bytes)?;
            additional_rows.push((
                format!("artifact_{}", Uuid::new_v4().simple()),
                (*kind).to_string(),
                (*mime_type).to_string(),
                extra_digest,
                extra_bytes.len() as i64,
                extra_path.to_string_lossy().into_owned(),
            ));
        }

        let asset_id = format!("asset_{}", Uuid::new_v4().simple());
        let artifact_id = format!("artifact_{}", Uuid::new_v4().simple());
        let created_at = timestamp()?;
        let metadata_json = serde_json::to_string(&metadata)?;
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO assets (id,name,kind,mime_type,sha256,byte_size,source_asset_id,metadata_json,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                asset_id,
                safe_name,
                descriptor.kind.as_str(),
                descriptor.mime_type,
                digest,
                bytes.len() as i64,
                source_asset_id,
                metadata_json,
                created_at
            ],
        )?;
        transaction.execute(
            "INSERT INTO artifacts (id,asset_id,kind,sha256,byte_size,mime_type,storage_path,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                artifact_id,
                asset_id,
                artifact_kind,
                digest,
                bytes.len() as i64,
                descriptor.mime_type,
                artifact_path.to_string_lossy(),
                created_at
            ],
        )?;
        for (id, kind, mime_type, extra_digest, byte_size, storage_path) in additional_rows {
            transaction.execute(
                "INSERT INTO artifacts (id,asset_id,kind,sha256,byte_size,mime_type,storage_path,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    asset_id,
                    kind,
                    extra_digest,
                    byte_size,
                    mime_type,
                    storage_path,
                    created_at
                ],
            )?;
        }
        transaction.commit()?;
        drop(conn);
        self.get_asset(&asset_id)?.ok_or_else(|| {
            CoreError::Conflict("Imported asset was not visible after commit.".into())
        })
    }

    pub fn add_artifact(
        &self,
        asset_id: &str,
        kind: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> CoreResult<Artifact> {
        if self.get_asset(asset_id)?.is_none() {
            return Err(CoreError::NotFound(format!(
                "Asset '{asset_id}' was not found."
            )));
        }
        let digest = digest(bytes);
        let path = self.artifact_path_for_hash(&digest);
        self.write_content_addressed(&path, bytes)?;
        let id = format!("artifact_{}", Uuid::new_v4().simple());
        let created_at = timestamp()?;
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        conn.execute(
            "INSERT INTO artifacts (id,asset_id,kind,sha256,byte_size,mime_type,storage_path,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, asset_id, kind, digest, bytes.len() as i64, mime_type, path.to_string_lossy(), created_at],
        )?;
        Ok(Artifact {
            id,
            kind: kind.to_string(),
            sha256: digest,
            byte_size: bytes.len() as u64,
            mime_type: mime_type.to_string(),
        })
    }

    pub fn get_asset(&self, id: &str) -> CoreResult<Option<Asset>> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id,name,kind,mime_type,sha256,byte_size,source_asset_id,metadata_json,created_at
                 FROM assets WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            id,
            name,
            kind,
            mime_type,
            sha256,
            byte_size,
            source_asset_id,
            metadata_json,
            created_at,
        )) = row
        else {
            return Ok(None);
        };
        let mut artifact_statement = conn.prepare(
            "SELECT id,kind,sha256,byte_size,mime_type FROM artifacts WHERE asset_id=?1 ORDER BY created_at,id",
        )?;
        let artifact_rows = artifact_statement.query_map([&id], |row| {
            Ok(Artifact {
                id: row.get(0)?,
                kind: row.get(1)?,
                sha256: row.get(2)?,
                byte_size: row.get::<_, i64>(3)? as u64,
                mime_type: row.get(4)?,
            })
        })?;
        let artifacts = artifact_rows.collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Asset {
            id,
            name,
            kind: AssetKind::from_str(&kind)?,
            mime_type,
            sha256,
            byte_size: byte_size as u64,
            source_asset_id,
            metadata: serde_json::from_str(&metadata_json)?,
            created_at,
            artifacts,
        }))
    }

    pub fn list_assets(&self, limit: usize) -> CoreResult<Vec<Asset>> {
        let ids = {
            let conn = self.connection.lock().expect("SQLite mutex poisoned");
            let mut statement =
                conn.prepare("SELECT id FROM assets ORDER BY created_at DESC,id LIMIT ?1")?;
            let rows =
                statement.query_map([limit.clamp(1, 200) as i64], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        ids.iter()
            .filter_map(|id| match self.get_asset(id) {
                Ok(Some(asset)) => Some(Ok(asset)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub fn read_artifact(&self, artifact_id: &str) -> CoreResult<ArtifactContent> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let record = conn
            .query_row(
                "SELECT id,kind,sha256,byte_size,mime_type,storage_path FROM artifacts WHERE id=?1",
                [artifact_id],
                |row| {
                    Ok((
                        Artifact {
                            id: row.get(0)?,
                            kind: row.get(1)?,
                            sha256: row.get(2)?,
                            byte_size: row.get::<_, i64>(3)? as u64,
                            mime_type: row.get(4)?,
                        },
                        PathBuf::from(row.get::<_, String>(5)?),
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                CoreError::NotFound(format!("Artifact '{artifact_id}' was not found."))
            })?;
        drop(conn);
        Ok(ArtifactContent {
            artifact: record.0,
            bytes: fs::read(record.1)?,
        })
    }

    pub fn artifact_path(&self, artifact_id: &str) -> CoreResult<PathBuf> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        conn.query_row(
            "SELECT storage_path FROM artifacts WHERE id=?1",
            [artifact_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(PathBuf::from)
        .ok_or_else(|| CoreError::NotFound(format!("Artifact '{artifact_id}' was not found.")))
    }

    pub fn create_job(
        &self,
        operation: &str,
        input_asset_id: Option<&str>,
        parameters: &Value,
    ) -> CoreResult<Job> {
        let id = format!("job_{}", Uuid::new_v4().simple());
        let now = timestamp()?;
        let parameters_json = serde_json::to_string(parameters)?;
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        conn.execute(
            "INSERT INTO jobs (id,operation,state,input_asset_id,parameters_json,progress,created_at,updated_at)
             VALUES (?1,?2,'queued',?3,?4,0,?5,?5)",
            params![id, operation, input_asset_id, parameters_json, now],
        )?;
        drop(conn);
        self.get_job(&id)?
            .ok_or_else(|| CoreError::Conflict("Created job was not visible after commit.".into()))
    }

    pub fn get_job(&self, id: &str) -> CoreResult<Option<Job>> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id,operation,state,input_asset_id,parameters_json,progress,result_json,error_json,created_at,updated_at
                 FROM jobs WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            id,
            operation,
            state,
            input_asset_id,
            parameters_json,
            progress,
            result_json,
            error_json,
            created_at,
            updated_at,
        )) = row
        else {
            return Ok(None);
        };
        Ok(Some(Job {
            id,
            operation,
            state: JobState::parse(&state)?,
            input_asset_id,
            parameters: serde_json::from_str(&parameters_json)?,
            progress: progress as u8,
            result: result_json.map(|v| serde_json::from_str(&v)).transpose()?,
            error: error_json.map(|v| serde_json::from_str(&v)).transpose()?,
            created_at,
            updated_at,
        }))
    }

    pub fn update_job(
        &self,
        id: &str,
        next_state: JobState,
        progress: u8,
        result: Option<Value>,
        error: Option<Value>,
    ) -> CoreResult<Job> {
        if progress > 100 {
            return Err(CoreError::InvalidInput(
                "Job progress must be in the range 0 to 100.".into(),
            ));
        }
        let now = timestamp()?;
        let result_json = result.as_ref().map(serde_json::to_string).transpose()?;
        let error_json = error.as_ref().map(serde_json::to_string).transpose()?;
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.unchecked_transaction()?;
        let current = job_state_from_transaction(&transaction, id)?;
        if !valid_transition(&current, &next_state) {
            return Err(CoreError::Conflict(format!(
                "Job state cannot transition from '{}' to '{}'.",
                current.as_str(),
                next_state.as_str()
            )));
        }
        transaction.execute(
            "UPDATE jobs SET state=?2,progress=?3,result_json=COALESCE(?4,result_json),error_json=COALESCE(?5,error_json),updated_at=?6 WHERE id=?1",
            params![id, next_state.as_str(), progress, result_json, error_json, now],
        )?;
        transaction.commit()?;
        drop(conn);
        self.get_job(id)?
            .ok_or_else(|| CoreError::NotFound(format!("Job '{id}' was not found.")))
    }

    pub fn cancel_job(&self, id: &str) -> CoreResult<Job> {
        let job = self
            .get_job(id)?
            .ok_or_else(|| CoreError::NotFound(format!("Job '{id}' was not found.")))?;
        match job.state {
            JobState::Queued => self.update_job(id, JobState::Cancelled, job.progress, None, None),
            JobState::Running | JobState::Validating => Err(CoreError::Conflict(
                "This native operation cannot be interrupted safely once it has started.".into(),
            )),
            JobState::Cancelling | JobState::Cancelled => Ok(job),
            JobState::Succeeded | JobState::Failed => Err(CoreError::Conflict(
                "A completed job cannot be cancelled.".into(),
            )),
        }
    }

    fn artifact_path_for_hash(&self, hash: &str) -> PathBuf {
        self.artifact_root.join(&hash[..2]).join(hash)
    }

    fn write_content_addressed(&self, path: &Path, bytes: &[u8]) -> CoreResult<()> {
        let parent = path.parent().expect("artifact has parent directory");
        fs::create_dir_all(parent)?;
        if path.exists() {
            return Ok(());
        }
        let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4().simple()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(_error) if path.exists() => {
                let _ = fs::remove_file(&temporary);
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(error.into())
            }
        }
    }
}

fn job_state_from_transaction(transaction: &Transaction<'_>, id: &str) -> CoreResult<JobState> {
    let state = transaction
        .query_row("SELECT state FROM jobs WHERE id=?1", [id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("Job '{id}' was not found.")))?;
    JobState::parse(&state)
}

fn valid_transition(current: &JobState, next: &JobState) -> bool {
    matches!(
        (current, next),
        (JobState::Queued, JobState::Running | JobState::Cancelled)
            | (
                JobState::Running,
                JobState::Validating
                    | JobState::Failed
                    | JobState::Cancelling
                    | JobState::Cancelled
            )
            | (
                JobState::Validating,
                JobState::Succeeded | JobState::Failed | JobState::Cancelling | JobState::Cancelled
            )
            | (JobState::Cancelling, JobState::Cancelled | JobState::Failed)
    )
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn timestamp() -> CoreResult<String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CoreError::Clock)?
        .as_millis()
        .to_string())
}

fn sanitize_filename(name: &str) -> CoreResult<String> {
    let base = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let cleaned = base
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." || cleaned.len() > 240 {
        return Err(CoreError::InvalidInput(
            "Asset filename is empty or unsafe.".into(),
        ));
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use serde_json::json;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("meshharbor-core-{}-{n}", std::process::id()))
    }

    #[test]
    fn imports_immutable_content_addressed_asset_and_lists_it() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nimage bytes";
        let imported = store.import_asset("../draft.png", bytes).unwrap();
        assert_eq!(imported.name, "draft.png");
        assert_eq!(imported.kind, AssetKind::Image);
        assert_eq!(imported.artifacts.len(), 1);
        assert_eq!(
            store
                .read_artifact(&imported.artifacts[0].id)
                .unwrap()
                .bytes,
            bytes
        );
        assert_eq!(store.list_assets(10).unwrap().len(), 1);
        assert_eq!(
            store.get_asset(&imported.id).unwrap().unwrap().sha256,
            imported.sha256
        );
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imports_derived_asset_and_secondary_artifact_atomically() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let source = store
            .import_asset("source.png", b"\x89PNG\r\n\x1a\nimage bytes")
            .unwrap();
        let preview = b"\x89PNG\r\n\x1a\npreview bytes";
        let quad_obj = b"v 0 0 0\nf 1 2 3 4\n";
        let result = store
            .import_derived_asset_with_artifacts(
                "retopology.png",
                preview,
                &source.id,
                serde_json::Value::Null,
                &[("quad_topology", "model/obj", quad_obj)],
            )
            .unwrap();
        assert_eq!(result.artifacts.len(), 2);
        let obj = result
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == "quad_topology")
            .unwrap();
        assert_eq!(store.read_artifact(&obj.id).unwrap().bytes, quad_obj);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_invalid_files_and_pathless_names() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        assert!(store.import_asset("", b"not a supported asset").is_err());
        assert!(store.import_asset("../", b"not a supported asset").is_err());
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persists_job_state_and_enforces_transitions() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let job = store
            .create_job("mesh.simplify", None, &json!({ "target_triangles": 100 }))
            .unwrap();
        assert_eq!(job.state, JobState::Queued);
        let running = store
            .update_job(&job.id, JobState::Running, 15, None, None)
            .unwrap();
        assert_eq!(running.progress, 15);
        assert!(
            store
                .update_job(&job.id, JobState::Succeeded, 100, None, None)
                .is_err()
        );
        let validating = store
            .update_job(&job.id, JobState::Validating, 90, None, None)
            .unwrap();
        assert_eq!(validating.state, JobState::Validating);
        let complete = store
            .update_job(
                &job.id,
                JobState::Succeeded,
                100,
                Some(json!({"ok":true})),
                None,
            )
            .unwrap();
        assert_eq!(complete.result, Some(json!({ "ok": true })));
        assert!(
            store
                .update_job(&job.id, JobState::Failed, 100, None, None)
                .is_err()
        );
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn queued_job_can_cancel_but_running_native_work_is_not_misreported_as_cancelled() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let queued = store.create_job("mesh.simplify", None, &json!({})).unwrap();
        assert_eq!(
            store.cancel_job(&queued.id).unwrap().state,
            JobState::Cancelled
        );

        let active = store.create_job("mesh.simplify", None, &json!({})).unwrap();
        store
            .update_job(&active.id, JobState::Running, 10, None, None)
            .unwrap();
        let error = store.cancel_job(&active.id).unwrap_err();
        assert!(error.to_string().contains("cannot be interrupted safely"));
        assert_eq!(
            store.get_job(&active.id).unwrap().unwrap().state,
            JobState::Running
        );
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }
}
