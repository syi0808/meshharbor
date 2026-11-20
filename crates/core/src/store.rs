use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::inspect_asset;

const MAX_STORED_ARTIFACT_BYTES: u64 = crate::MAX_ASSET_BYTES as u64;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetPage {
    pub items: Vec<Asset>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub id: String,
    pub name: String,
    pub generated: bool,
    pub current_asset_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVersion {
    pub asset_id: String,
    pub number: u64,
    pub parent_number: Option<u64>,
    pub operation: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVersionPage {
    pub items: Vec<ModelVersion>,
    pub current_asset_id: String,
    pub next_before: Option<u64>,
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
    #[serde(default)]
    pub input_asset_name: Option<String>,
    pub parameters: Value,
    pub progress: u8,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub stage: Option<String>,
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
    pub(crate) fn data_root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: impl AsRef<Path>) -> CoreResult<Self> {
        let root = root.as_ref().to_path_buf();
        let artifact_root = root.join("artifacts");
        let staging_root = root.join("staging");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(&staging_root)?;
        let mut connection = Connection::open(root.join("meshharbor.sqlite3"))?;
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
                updated_at TEXT NOT NULL,
                started_at TEXT,
                stage TEXT
            );
            CREATE INDEX IF NOT EXISTS jobs_created_at_idx ON jobs(created_at DESC);
            CREATE INDEX IF NOT EXISTS jobs_updated_at_idx ON jobs(updated_at DESC, id DESC);
            CREATE INDEX IF NOT EXISTS jobs_active_idx ON jobs(updated_at DESC, id DESC)
                WHERE state NOT IN ('succeeded','failed','cancelled');",
        )?;
        let migration = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let columns = {
            let mut statement = migration.prepare("PRAGMA table_info(jobs)")?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
        };
        if !columns.iter().any(|column| column == "started_at") {
            migration.execute("ALTER TABLE jobs ADD COLUMN started_at TEXT", [])?;
        }
        if !columns.iter().any(|column| column == "stage") {
            migration.execute("ALTER TABLE jobs ADD COLUMN stage TEXT", [])?;
        }
        migrate_model_versions(&migration)?;
        migration.commit()?;
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

    pub(crate) fn import_generated_asset(
        &self,
        name: &str,
        bytes: &[u8],
        source_asset_id: Option<&str>,
        mut metadata: Value,
    ) -> CoreResult<Asset> {
        metadata["origin"] = Value::String("generated".into());
        self.import_with_metadata(name, bytes, source_asset_id, "derived", metadata)
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
        let asset_digest = digest(bytes);
        let artifact_path = self.artifact_path_for_hash(&asset_digest);
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
        let mut conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let model_id = if descriptor.kind == AssetKind::Mesh {
            let source_model_id = match source_asset_id {
                Some(source_id) => transaction
                    .query_row(
                        "SELECT model_id FROM assets WHERE id=?1",
                        [source_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten(),
                None => None,
            };
            Some(source_model_id.unwrap_or_else(|| asset_id.clone()))
        } else {
            None
        };
        let model_version = match model_id.as_deref() {
            Some(id) if id == asset_id => Some(1),
            Some(id) => Some(transaction.query_row(
                "SELECT COALESCE(MAX(model_version),0)+1 FROM assets WHERE model_id=?1",
                [id],
                |row| row.get::<_, i64>(0),
            )?),
            None => None,
        };
        transaction.execute(
            "INSERT INTO assets (id,name,kind,mime_type,sha256,byte_size,source_asset_id,metadata_json,created_at,model_id,model_version)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                asset_id,
                safe_name,
                descriptor.kind.as_str(),
                descriptor.mime_type,
                asset_digest,
                bytes.len() as i64,
                source_asset_id,
                metadata_json,
                created_at,
                model_id,
                model_version,
            ],
        )?;
        transaction.execute(
            "INSERT INTO artifacts (id,asset_id,kind,sha256,byte_size,mime_type,storage_path,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                artifact_id,
                asset_id,
                artifact_kind,
                asset_digest,
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
        if let Some(model_id) = model_id {
            if model_id == asset_id {
                transaction.execute(
                    "INSERT INTO model_heads (model_id,current_asset_id) VALUES (?1,?1)",
                    [&asset_id],
                )?;
            } else {
                transaction.execute(
                    "UPDATE model_heads SET current_asset_id=?1 WHERE model_id=?2 AND current_asset_id=?3",
                    params![asset_id, model_id, source_asset_id],
                )?;
            }
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
        Self::read_asset(&conn, id)
    }

    fn read_asset(conn: &Connection, id: &str) -> CoreResult<Option<Asset>> {
        let row = conn
            .query_row(
                "SELECT version.id,version.name,version.kind,version.mime_type,version.sha256,version.byte_size,
                        version.source_asset_id,version.metadata_json,version.created_at,
                        root.id,root.name,
                        root.source_asset_id IS NOT NULL OR json_extract(root.metadata_json,'$.origin') IS 'generated',
                        head.current_asset_id,version.model_version
                 FROM assets AS version LEFT JOIN assets AS root ON root.id=version.model_id
                 LEFT JOIN model_heads AS head ON head.model_id=root.id WHERE version.id=?1",
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
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, bool>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<i64>>(13)?,
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
            model_id,
            model_name,
            model_generated,
            current_asset_id,
            model_version,
        )) = row
        else {
            return Ok(None);
        };
        let mut artifact_statement = conn.prepare(
            "SELECT id,kind,sha256,byte_size,mime_type FROM artifacts WHERE asset_id=?1
             ORDER BY CASE WHEN sha256=(SELECT sha256 FROM assets WHERE id=?1) THEN 0 ELSE 1 END, created_at,id",
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
            model_version: model_version.map(|number| number as u64),
            model: match (model_id, model_name, current_asset_id) {
                (Some(id), Some(name), Some(current_asset_id)) => Some(ModelIdentity {
                    id,
                    name,
                    generated: model_generated,
                    current_asset_id,
                }),
                _ => None,
            },
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

    /// Read one stable page of assets ordered newest first. Cursor pagination keeps older
    /// assets reachable without loading the entire workspace into the dashboard.
    pub fn list_assets_page(
        &self,
        limit: usize,
        cursor: Option<&str>,
        query: Option<&str>,
        kind: Option<AssetKind>,
        derived: Option<bool>,
    ) -> CoreResult<AssetPage> {
        self.read_asset_page(limit, cursor, query, kind, derived, false)
    }

    pub fn list_models_page(
        &self,
        limit: usize,
        cursor: Option<&str>,
        query: Option<&str>,
        generated: Option<bool>,
    ) -> CoreResult<AssetPage> {
        self.read_asset_page(limit, cursor, query, Some(AssetKind::Mesh), generated, true)
    }

    fn read_asset_page(
        &self,
        limit: usize,
        cursor: Option<&str>,
        query: Option<&str>,
        kind: Option<AssetKind>,
        derived: Option<bool>,
        models_only: bool,
    ) -> CoreResult<AssetPage> {
        const MAX_PAGE_SIZE: usize = 100;
        const MAX_SEARCH_BYTES: usize = 256;

        let page_size = limit.clamp(1, MAX_PAGE_SIZE);
        let query = query.map(str::trim).filter(|value| !value.is_empty());
        if query.is_some_and(|value| value.len() > MAX_SEARCH_BYTES) {
            return Err(CoreError::InvalidInput(format!(
                "Asset search query exceeds {MAX_SEARCH_BYTES} bytes."
            )));
        }
        let search_pattern = query.map(|value| {
            let mut pattern = String::with_capacity(value.len() + 2);
            pattern.push('%');
            for character in value.chars() {
                if matches!(character, '%' | '_' | '\\') {
                    pattern.push('\\');
                }
                pattern.push(character);
            }
            pattern.push('%');
            pattern
        });

        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.unchecked_transaction()?;
        let asset_ids = {
            let cursor_created_at = match cursor {
                Some(cursor) => Some(
                    transaction
                        .query_row(
                            "SELECT created_at FROM assets WHERE id=?1 AND (NOT ?2 OR id=model_id)",
                            params![cursor, models_only],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?
                        .ok_or_else(|| {
                            CoreError::InvalidInput("Asset page cursor is unknown.".into())
                        })?,
                ),
                None => None,
            };
            let mut statement = transaction.prepare(
                "SELECT id, COALESCE(model_heads.current_asset_id, id) FROM assets
                 LEFT JOIN model_heads ON ?7 AND model_heads.model_id=assets.id
                 WHERE (?1 IS NULL OR name LIKE ?1 ESCAPE '\\')
                   AND (?2 IS NULL OR kind = ?2)
                   AND (NOT ?7 OR assets.model_id=assets.id)
                   AND (?3 IS NULL OR ?3 = (
                        source_asset_id IS NOT NULL OR json_extract(metadata_json,'$.origin') IS 'generated'))
                   AND (?4 IS NULL OR created_at < ?4 OR (created_at = ?4 AND id > ?5))
                 ORDER BY created_at DESC, id ASC
                 LIMIT ?6",
            )?;
            let rows = statement.query_map(
                params![
                    search_pattern,
                    kind.map(AssetKind::as_str),
                    derived.map(|value| if value { 1_i64 } else { 0_i64 }),
                    cursor_created_at,
                    cursor,
                    (page_size + 1) as i64,
                    models_only,
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let has_more = asset_ids.len() > page_size;
        let page_ids = &asset_ids[..asset_ids.len().min(page_size)];
        let next_cursor = has_more
            .then(|| page_ids.last().map(|(root, _)| root.clone()))
            .flatten();
        let mut items = Vec::with_capacity(page_ids.len());
        for (_, id) in page_ids {
            if let Some(asset) = Self::read_asset(&transaction, id)? {
                items.push(asset);
            }
        }
        transaction.commit()?;
        Ok(AssetPage { items, next_cursor })
    }

    pub fn list_model_versions(
        &self,
        model_id: &str,
        limit: usize,
        before: Option<u64>,
    ) -> CoreResult<ModelVersionPage> {
        let page_size = limit.clamp(1, 100);
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.unchecked_transaction()?;
        let current_asset_id = transaction
            .query_row(
                "SELECT current_asset_id FROM model_heads WHERE model_id=?1",
                [model_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound("Model was not found.".into()))?;
        let mut items = {
            let mut statement = transaction.prepare(
                "SELECT version.id,version.model_version,parent.model_version,
                        json_extract(version.metadata_json,'$.operation'),version.created_at
                 FROM assets AS version LEFT JOIN assets AS parent ON parent.id=version.source_asset_id AND parent.model_id=?1
                 WHERE version.model_id=?1 AND (?2 IS NULL OR version.model_version < ?2)
                 ORDER BY version.model_version DESC LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![
                    model_id,
                    before.map(|value| value.min(i64::MAX as u64) as i64),
                    (page_size + 1) as i64
                ],
                |row| {
                    Ok(ModelVersion {
                        asset_id: row.get(0)?,
                        number: row.get::<_, i64>(1)? as u64,
                        parent_number: row.get::<_, Option<i64>>(2)?.map(|number| number as u64),
                        operation: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        transaction.commit()?;
        let has_more = items.len() > page_size;
        items.truncate(page_size);
        let next_before = has_more
            .then(|| items.last().map(|item| item.number))
            .flatten();
        Ok(ModelVersionPage {
            items,
            current_asset_id,
            next_before,
        })
    }

    pub fn restore_model_version(&self, model_id: &str, asset_id: &str) -> CoreResult<Asset> {
        let mut conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let written = transaction.execute(
            "UPDATE model_heads SET current_asset_id=?2 WHERE model_id=?1
             AND EXISTS (SELECT 1 FROM assets WHERE model_id=?1 AND id=?2)",
            params![model_id, asset_id],
        )?;
        if written == 0 {
            return Err(CoreError::NotFound(
                "That version does not belong to this model.".into(),
            ));
        }
        let asset = Self::read_asset(&transaction, asset_id)?
            .ok_or_else(|| CoreError::NotFound("Model version was not found.".into()))?;
        transaction.commit()?;
        Ok(asset)
    }

    /// List persisted job records, newest update first. This does not resume or modify jobs.
    pub fn list_jobs(&self, limit: usize) -> CoreResult<Vec<Job>> {
        self.read_jobs(limit, false, None)
    }

    pub fn list_dashboard_jobs(&self, limit: usize, before: Option<&str>) -> CoreResult<Vec<Job>> {
        self.read_jobs(limit, true, before)
    }

    fn read_jobs(
        &self,
        limit: usize,
        dashboard: bool,
        before: Option<&str>,
    ) -> CoreResult<Vec<Job>> {
        let ids = {
            let conn = self.connection.lock().expect("SQLite mutex poisoned");
            let sql = match (dashboard, before) {
                (false, _) => "SELECT id FROM jobs ORDER BY updated_at DESC, id DESC LIMIT ?1",
                (true, None) => "SELECT id FROM jobs WHERE state NOT IN ('succeeded','failed','cancelled')
                    UNION ALL SELECT id FROM (SELECT id FROM jobs WHERE state IN ('succeeded','failed','cancelled')
                    ORDER BY updated_at DESC, id DESC LIMIT ?1)",
                (true, Some(_)) => "SELECT id FROM jobs WHERE state IN ('succeeded','failed','cancelled')
                    AND (updated_at, id) < (SELECT updated_at, id FROM jobs WHERE id=?2 AND state IN ('succeeded','failed','cancelled'))
                    ORDER BY updated_at DESC, id DESC LIMIT ?1",
            };
            let mut statement = conn.prepare(sql)?;
            let limit = limit.clamp(1, 200) as i64;
            let mut rows = if let Some(before) = before {
                statement.query(params![limit, before])?
            } else {
                statement.query([limit])?
            };
            let mut ids = Vec::new();
            while let Some(row) = rows.next()? {
                ids.push(row.get::<_, String>(0)?);
            }
            ids
        };
        let mut jobs = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(job) = self.get_job(&id)? {
                jobs.push(job);
            }
        }
        if dashboard && before.is_none() {
            jobs.sort_by(|left, right| {
                left.state
                    .is_terminal()
                    .cmp(&right.state.is_terminal())
                    .then_with(|| right.updated_at.cmp(&left.updated_at))
                    .then_with(|| right.id.cmp(&left.id))
            });
        }
        Ok(jobs)
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
        let bytes = read_verified_artifact(&record.1, &record.0)?;
        Ok(ArtifactContent {
            artifact: record.0,
            bytes,
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
                "SELECT id,operation,state,input_asset_id,parameters_json,progress,result_json,error_json,created_at,updated_at,started_at,stage,
                        (SELECT name FROM assets WHERE id=jobs.input_asset_id)
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
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
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
            started_at,
            stage,
            input_asset_name,
        )) = row
        else {
            return Ok(None);
        };
        Ok(Some(Job {
            id,
            operation,
            state: JobState::parse(&state)?,
            input_asset_id,
            input_asset_name,
            parameters: serde_json::from_str(&parameters_json)?,
            progress: progress as u8,
            result: result_json.map(|v| serde_json::from_str(&v)).transpose()?,
            error: error_json.map(|v| serde_json::from_str(&v)).transpose()?,
            created_at,
            updated_at,
            started_at,
            stage,
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
        let mut conn = self.connection.lock().expect("SQLite mutex poisoned");
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = job_state_from_transaction(&transaction, id)?;
        if !valid_transition(&current, &next_state) {
            return Err(CoreError::Conflict(format!(
                "Job state cannot transition from '{}' to '{}'.",
                current.as_str(),
                next_state.as_str()
            )));
        }
        transaction.execute(
            "UPDATE jobs SET state=?2,progress=?3,result_json=COALESCE(?4,result_json),error_json=COALESCE(?5,error_json),updated_at=?6,
             started_at=CASE WHEN ?2='running' AND started_at IS NULL THEN ?6 ELSE started_at END WHERE id=?1",
            params![id, next_state.as_str(), progress, result_json, error_json, now],
        )?;
        transaction.commit()?;
        drop(conn);
        self.get_job(id)?
            .ok_or_else(|| CoreError::NotFound(format!("Job '{id}' was not found.")))
    }

    pub fn update_running_job_stage(&self, id: &str, stage: &str) -> CoreResult<()> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        conn.execute(
            "UPDATE jobs SET stage=?2,updated_at=?3 WHERE id=?1 AND state='running' AND stage IS NOT ?2",
            params![id, stage, timestamp()?],
        )?;
        Ok(())
    }

    pub(crate) fn job_cancellation_requested(&self, id: &str) -> CoreResult<bool> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        Ok(conn.query_row(
            "SELECT state IN ('cancelling','cancelled') FROM jobs WHERE id=?1",
            [id],
            |row| row.get(0),
        )?)
    }

    pub fn cancel_job(&self, id: &str) -> CoreResult<Job> {
        self.request_job_cancellation(id, false)
    }

    pub fn request_job_cancellation(&self, id: &str, queued_only: bool) -> CoreResult<Job> {
        let conn = self.connection.lock().expect("SQLite mutex poisoned");
        conn.execute(
            "UPDATE jobs SET state=CASE state WHEN 'queued' THEN 'cancelled' ELSE 'cancelling' END,updated_at=?2
             WHERE id=?1 AND (state='queued' OR (NOT ?3 AND state='running' AND
                (operation LIKE 'mesh.%' OR operation IN ('model.generate','image.generate','uv.unwrap','render.optimize'))))",
            params![id, timestamp()?, queued_only],
        )?;
        drop(conn);
        let job = self
            .get_job(id)?
            .ok_or_else(|| CoreError::NotFound(format!("Job '{id}' was not found.")))?;
        match job.state {
            JobState::Cancelled => Ok(job),
            JobState::Cancelling if !queued_only => Ok(job),
            JobState::Succeeded | JobState::Failed => Err(CoreError::Conflict(
                "A completed job cannot be cancelled.".into(),
            )),
            _ if queued_only => Err(CoreError::Conflict(
                "The job is no longer waiting in the queue.".into(),
            )),
            _ => Err(CoreError::Conflict(
                "This job cannot be stopped safely at its current stage.".into(),
            )),
        }
    }

    /// Mark persisted nonterminal jobs as failed after an unclean exit.
    ///
    /// The caller must ensure that no other MeshHarbor process is using this
    /// store. This deliberately does not resume work or delete partial assets.
    pub fn recover_interrupted_jobs(&self) -> CoreResult<Vec<Job>> {
        let unfinished_jobs = {
            let conn = self.connection.lock().expect("SQLite mutex poisoned");
            let mut statement = conn.prepare(
                "SELECT id, state FROM jobs
                 WHERE state IN ('queued', 'running', 'validating', 'cancelling')
                 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut recovered = Vec::with_capacity(unfinished_jobs.len());
        for (id, state) in unfinished_jobs {
            let previous_state = JobState::parse(&state)?;
            if previous_state.is_terminal() {
                continue;
            }
            let error = serde_json::json!({
                "code": "interrupted",
                "message": "This job was unfinished when MeshHarbor last exited. It was not resumed; inspect existing assets before retrying.",
                "previous_state": previous_state.as_str(),
            });
            recovered.push(self.update_job(&id, JobState::Failed, 100, None, Some(error))?);
        }
        Ok(recovered)
    }

    fn artifact_path_for_hash(&self, hash: &str) -> PathBuf {
        self.artifact_root.join(&hash[..2]).join(hash)
    }

    fn write_content_addressed(&self, path: &Path, bytes: &[u8]) -> CoreResult<()> {
        let expected_sha256 = digest(bytes);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected_sha256.as_str()) {
            return Err(CoreError::InvalidInput(
                "Content-addressed artifact path must match the payload SHA-256.".into(),
            ));
        }
        if bytes.len() as u64 > MAX_STORED_ARTIFACT_BYTES {
            return Err(CoreError::InvalidInput(format!(
                "Artifact exceeds the {} MiB storage limit.",
                MAX_STORED_ARTIFACT_BYTES / (1024 * 1024)
            )));
        }

        let parent = path.parent().expect("artifact has parent directory");
        fs::create_dir_all(parent)?;
        if path.exists() {
            return verify_existing_content_addressed(path, bytes, &expected_sha256);
        }

        let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4().simple()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
        if let Err(error) = write_result {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        drop(file);

        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                let _ = fs::remove_file(&temporary);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temporary);
                verify_existing_content_addressed(path, bytes, &expected_sha256)
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(error.into())
            }
        }
    }
}

fn read_bounded_artifact_path(
    path: &Path,
    expected_size: u64,
    description: &str,
) -> CoreResult<Vec<u8>> {
    if expected_size > MAX_STORED_ARTIFACT_BYTES {
        return Err(CoreError::Conflict(format!(
            "{description} has a recorded size above the {} MiB artifact limit.",
            MAX_STORED_ARTIFACT_BYTES / (1024 * 1024)
        )));
    }

    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CoreError::Conflict(format!("{description} could not be verified: {error}"))
    })?;
    if !metadata.file_type().is_file() {
        return Err(CoreError::Conflict(format!(
            "{description} is not a regular artifact file."
        )));
    }
    if metadata.len() != expected_size {
        return Err(CoreError::Conflict(format!(
            "{description} has {} bytes; the recorded size is {expected_size}.",
            metadata.len()
        )));
    }

    let read_limit = expected_size.checked_add(1).ok_or_else(|| {
        CoreError::Conflict(format!("{description} has an invalid recorded size."))
    })?;
    let file = fs::File::open(path).map_err(|error| {
        CoreError::Conflict(format!("{description} could not be verified: {error}"))
    })?;
    let mut bounded = file.take(read_limit);
    let mut bytes = Vec::new();
    bounded.read_to_end(&mut bytes).map_err(|error| {
        CoreError::Conflict(format!("{description} could not be verified: {error}"))
    })?;
    if bytes.len() as u64 != expected_size {
        return Err(CoreError::Conflict(format!(
            "{description} changed size while being read."
        )));
    }
    Ok(bytes)
}

fn read_verified_artifact(path: &Path, artifact: &Artifact) -> CoreResult<Vec<u8>> {
    let description = format!("Artifact '{}'", artifact.id);
    let bytes = read_bounded_artifact_path(path, artifact.byte_size, &description)?;
    if digest(&bytes) != artifact.sha256 {
        return Err(CoreError::Conflict(format!(
            "{description} failed SHA-256 verification; stored data may be corrupt."
        )));
    }
    Ok(bytes)
}

fn verify_existing_content_addressed(
    path: &Path,
    expected_bytes: &[u8],
    expected_sha256: &str,
) -> CoreResult<()> {
    let description = format!("Content-addressed artifact '{expected_sha256}'");
    let existing = read_bounded_artifact_path(path, expected_bytes.len() as u64, &description)?;
    if existing != expected_bytes {
        return Err(CoreError::Conflict(format!(
            "{description} does not match the expected content; existing data was left untouched."
        )));
    }
    Ok(())
}

fn migrate_model_versions(transaction: &Transaction<'_>) -> CoreResult<()> {
    let has_model_id: bool = transaction.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('assets') WHERE name='model_id')",
        [],
        |row| row.get(0),
    )?;
    if has_model_id {
        return Ok(());
    }
    transaction.execute_batch(
        "ALTER TABLE assets ADD COLUMN model_id TEXT REFERENCES assets(id) ON DELETE RESTRICT;
         ALTER TABLE assets ADD COLUMN model_version INTEGER CHECK(model_version > 0);
         CREATE INDEX assets_source_idx ON assets(source_asset_id);
         CREATE TABLE model_heads (
            model_id TEXT PRIMARY KEY REFERENCES assets(id) ON DELETE RESTRICT,
            current_asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE RESTRICT
         );
         WITH RECURSIVE lineage(asset_id, root_id) AS (
            SELECT mesh.id,mesh.id FROM assets AS mesh
            LEFT JOIN assets AS parent ON parent.id=mesh.source_asset_id
            WHERE mesh.kind='mesh' AND (parent.kind IS NULL OR parent.kind<>'mesh')
            UNION ALL
            SELECT child.id,lineage.root_id FROM lineage
            JOIN assets AS child ON child.source_asset_id=lineage.asset_id AND child.kind='mesh'
         ), numbered AS (
            SELECT lineage.asset_id,lineage.root_id,row_number() OVER (PARTITION BY lineage.root_id ORDER BY mesh.rowid) AS number
            FROM lineage JOIN assets AS mesh ON mesh.id=lineage.asset_id
         )
         UPDATE assets SET model_id=numbered.root_id,model_version=numbered.number FROM numbered WHERE assets.id=numbered.asset_id;
         CREATE UNIQUE INDEX assets_model_idx ON assets(model_id,model_version);
         INSERT INTO model_heads(model_id,current_asset_id)
         SELECT root.id,(SELECT id FROM assets WHERE model_id=root.id ORDER BY model_version DESC LIMIT 1)
         FROM assets AS root WHERE root.model_id=root.id;",
    )?;
    Ok(())
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
        (
            JobState::Queued,
            JobState::Running | JobState::Cancelled | JobState::Failed
        ) | (
            JobState::Running,
            JobState::Validating | JobState::Failed | JobState::Cancelling | JobState::Cancelled
        ) | (
            JobState::Validating,
            JobState::Succeeded | JobState::Failed | JobState::Cancelling | JobState::Cancelled
        ) | (JobState::Cancelling, JobState::Cancelled | JobState::Failed)
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
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let imported = store.import_asset("../draft.png", &bytes).unwrap();
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
    fn generated_models_without_source_assets_have_their_own_lineage_and_filter() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let document = br#"{"asset":{"version":"2.0"}} "#;
        let mut bytes = b"glTF".to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&((20 + document.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&(document.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"JSON");
        bytes.extend_from_slice(document);

        let imported = store.import_asset("imported.glb", &bytes).unwrap();
        let generated = store
            .import_generated_asset("generated.glb", &bytes, None, json!({"input_mode": "text"}))
            .unwrap();
        assert!(generated.source_asset_id.is_none());
        assert!(generated.model.as_ref().unwrap().generated);
        assert_eq!(generated.model_version, Some(1));
        assert_eq!(generated.artifacts[0].kind, "derived");
        let edited = store
            .import_derived_asset("edited.glb", &bytes, &generated.id, json!({}))
            .unwrap();
        assert_eq!(edited.model.as_ref().unwrap().id, generated.id);
        assert!(edited.model.as_ref().unwrap().generated);
        assert_eq!(edited.model_version, Some(2));
        for (generated_only, expected) in [(true, &edited.id), (false, &imported.id)] {
            let page = store
                .list_models_page(10, None, None, Some(generated_only))
                .unwrap();
            assert_eq!(page.items.len(), 1);
            assert_eq!(&page.items[0].id, expected);
        }
        let job = store
            .create_job("model.generate", None, &json!({"input_mode":"text"}))
            .unwrap();
        store
            .update_job(&job.id, JobState::Running, 5, None, None)
            .unwrap();
        assert_eq!(
            store.cancel_job(&job.id).unwrap().state,
            JobState::Cancelling
        );
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_legacy_model_lineage_without_merging_image_based_generations() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        {
            let conn = store.connection.lock().unwrap();
            conn.execute_batch(
                "DROP TABLE model_heads;
                 DROP INDEX assets_model_idx;
                 DROP INDEX assets_source_idx;
                 ALTER TABLE assets DROP COLUMN model_version;
                 ALTER TABLE assets DROP COLUMN model_id;",
            )
            .unwrap();
            for (id, kind, parent) in [
                ("reference", "image", None),
                ("generated", "mesh", Some("reference")),
                ("cleanup", "mesh", Some("generated")),
                ("retopology", "mesh", Some("cleanup")),
                ("separate-model", "mesh", Some("reference")),
            ] {
                conn.execute(
                    "INSERT INTO assets (id,name,kind,mime_type,sha256,byte_size,source_asset_id,metadata_json,created_at)
                     VALUES (?1,?1,?2,'model/gltf-binary','fixture',0,?3,'{}','1000')",
                    params![id, kind, parent],
                ).unwrap();
            }
        }
        drop(store);
        let store = Store::open(&dir).unwrap();
        let page = store.list_models_page(10, None, None, Some(true)).unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            ["retopology", "separate-model"]
        );
        assert_eq!(page.items[0].model.as_ref().unwrap().id, "generated");
        let history = store.list_model_versions("generated", 2, None).unwrap();
        assert_eq!(history.current_asset_id, "retopology");
        assert_eq!(history.items[0].number, 3);
        assert_eq!(history.items[0].parent_number, Some(2));
        assert_eq!(history.next_before, Some(2));
        let older = store
            .list_model_versions("generated", 2, history.next_before)
            .unwrap();
        assert_eq!(older.items[0].asset_id, "generated");
        assert_eq!(older.items[0].number, 1);
        assert!(older.next_before.is_none());
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asset_pages_use_a_stable_cursor_without_duplicates_or_gaps() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        for name in [
            "first.png",
            "second.png",
            "third.png",
            "fourth.png",
            "fifth.png",
        ] {
            store.import_asset(name, &bytes).unwrap();
        }

        let expected: Vec<_> = store
            .list_assets(200)
            .unwrap()
            .into_iter()
            .map(|asset| asset.id)
            .collect();
        let mut actual = Vec::new();
        let mut cursor = None;
        loop {
            let page = store
                .list_assets_page(2, cursor.as_deref(), None, None, None)
                .unwrap();
            assert!(page.items.len() <= 2);
            actual.extend(page.items.into_iter().map(|asset| asset.id));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(actual, expected);
        assert!(
            store
                .list_assets_page(2, Some("missing-asset-cursor"), None, None, None)
                .is_err()
        );
        let empty = store
            .list_assets_page(2, None, Some("no-such-asset"), None, None)
            .unwrap();
        assert!(empty.items.is_empty());
        assert_eq!(empty.next_cursor, None);

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asset_pages_filter_names_literally_by_kind_and_origin() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let source = store.import_asset("Oak%Panel Source.png", &bytes).unwrap();
        let exact_derived = store
            .import_derived_asset("Oak%Panel Derived.png", &bytes, &source.id, json!({}))
            .unwrap();
        store
            .import_derived_asset("OakZZPanel Derived.png", &bytes, &source.id, json!({}))
            .unwrap();

        let exact = store
            .list_assets_page(
                20,
                None,
                Some("oak%panel"),
                Some(AssetKind::Image),
                Some(true),
            )
            .unwrap();
        assert_eq!(
            exact
                .items
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            vec![exact_derived.id.as_str()]
        );

        let originals = store
            .list_assets_page(
                20,
                None,
                Some("oak%panel"),
                Some(AssetKind::Image),
                Some(false),
            )
            .unwrap();
        assert_eq!(
            originals
                .items
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            vec![source.id.as_str()]
        );
        let wrong_kind = store
            .list_assets_page(20, None, Some("oak%panel"), Some(AssetKind::Mesh), None)
            .unwrap();
        assert!(wrong_kind.items.is_empty());

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn duplicate_import_reuses_valid_content_addressed_artifact() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let first = store.import_asset("first.png", &bytes).unwrap();
        let second = store.import_asset("second.png", &bytes).unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(
            store.artifact_path(&first.artifacts[0].id).unwrap(),
            store.artifact_path(&second.artifacts[0].id).unwrap()
        );
        assert_eq!(
            store.read_artifact(&second.artifacts[0].id).unwrap().bytes,
            bytes
        );

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_artifact_rejects_same_length_sha256_mismatch_without_repairing() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let asset = store.import_asset("source.png", &bytes).unwrap();
        let artifact = &asset.artifacts[0];
        let path = store.artifact_path(&artifact.id).unwrap();
        let mut corrupted = bytes.clone();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 1;
        fs::write(&path, &corrupted).unwrap();

        let error = store.read_artifact(&artifact.id).unwrap_err();
        assert!(error.to_string().contains("SHA-256 verification"));
        assert_eq!(fs::read(&path).unwrap(), corrupted);

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_artifact_rejects_truncated_and_oversized_files() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let asset = store.import_asset("source.png", &bytes).unwrap();
        let artifact = &asset.artifacts[0];
        let path = store.artifact_path(&artifact.id).unwrap();

        fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(
            store
                .read_artifact(&artifact.id)
                .unwrap_err()
                .to_string()
                .contains("recorded size")
        );

        let mut oversized = bytes.clone();
        oversized.push(0);
        fs::write(&path, &oversized).unwrap();
        assert!(
            store
                .read_artifact(&artifact.id)
                .unwrap_err()
                .to_string()
                .contains("recorded size")
        );

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_artifact_rejects_untrusted_oversized_database_metadata() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let asset = store.import_asset("source.png", &bytes).unwrap();
        let artifact = &asset.artifacts[0];
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE artifacts SET byte_size=?1 WHERE id=?2",
                    rusqlite::params![MAX_STORED_ARTIFACT_BYTES as i64 + 1, &artifact.id],
                )
                .unwrap();
        }

        let error = store.read_artifact(&artifact.id).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("above the 512 MiB artifact limit")
        );

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reimport_fails_closed_when_content_addressed_file_is_corrupt() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let bytes = crate::glb::test_png_with_dimensions(1, 1);
        let original = store.import_asset("source.png", &bytes).unwrap();
        let path = store.artifact_path(&original.artifacts[0].id).unwrap();
        let mut corrupted = bytes.clone();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 1;
        fs::write(&path, &corrupted).unwrap();

        let error = store.import_asset("same-content.png", &bytes).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("existing data was left untouched")
        );
        assert_eq!(fs::read(&path).unwrap(), corrupted);
        assert_eq!(store.list_assets(10).unwrap().len(), 1);

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imports_derived_asset_and_secondary_artifact_atomically() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let source_bytes = crate::glb::test_png_with_dimensions(1, 1);
        let source = store.import_asset("source.png", &source_bytes).unwrap();
        let preview = crate::glb::test_png_with_dimensions(1, 1);
        let quad_obj = b"v 0 0 0\nf 1 2 3 4\n";
        let result = store
            .import_derived_asset_with_artifacts(
                "retopology.png",
                &preview,
                &source.id,
                serde_json::Value::Null,
                &[("quad_topology", "model/obj", quad_obj)],
            )
            .unwrap();
        assert_eq!(result.artifacts.len(), 2);
        assert_eq!(result.artifacts[0].sha256, result.sha256);
        assert_eq!(result.artifacts[0].kind, "derived");
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
    fn rejects_malformed_png_before_writing_artifacts_or_asset_rows() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        let error = store
            .import_asset("truncated.png", b"\x89PNG\r\n\x1a\n")
            .unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
        assert!(store.list_assets(10).unwrap().is_empty());
        assert!(fs::read_dir(&store.artifact_root).unwrap().next().is_none());

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dashboard_keeps_active_jobs_outside_the_paginated_history_limit() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let active = store
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        let mut completed = Vec::new();
        for _ in 0..3 {
            let job = store.create_job("mesh.cleanup", None, &json!({})).unwrap();
            store.cancel_job(&job.id).unwrap();
            completed.push(job.id);
        }
        completed.sort_by(|left, right| right.cmp(left));
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE jobs SET updated_at = '2000' WHERE state = 'cancelled'",
                [],
            )
            .unwrap();

        let first = store.list_dashboard_jobs(2, None).unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(first[0].id, active.id);
        assert_eq!(first[1].id, completed[0]);
        assert_eq!(first[2].id, completed[1]);
        let older = store.list_dashboard_jobs(2, Some(&first[2].id)).unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].id, completed[2]);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_older_jobs_and_preserves_start_time_and_last_running_stage() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let queued = store
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "ALTER TABLE jobs DROP COLUMN started_at; ALTER TABLE jobs DROP COLUMN stage;",
            )
            .unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        let migrated = store.get_job(&queued.id).unwrap().unwrap();
        assert_eq!(migrated.created_at, queued.created_at);
        assert!(migrated.started_at.is_none() && migrated.stage.is_none());
        let running = store
            .update_job(&queued.id, JobState::Running, 5, None, None)
            .unwrap();
        assert_eq!(running.started_at.as_ref(), Some(&running.updated_at));
        store
            .update_running_job_stage(&queued.id, "texture_flow")
            .unwrap();
        let failed = store
            .update_job(&queued.id, JobState::Failed, 100, None, None)
            .unwrap();
        assert_eq!(failed.stage.as_deref(), Some("texture_flow"));
        assert_eq!(failed.started_at, running.started_at);

        store
            .update_running_job_stage(&queued.id, "save_model")
            .unwrap();
        let persisted = store.get_job(&queued.id).unwrap().unwrap();
        assert_eq!(persisted.stage, failed.stage);
        assert_eq!(persisted.updated_at, failed.updated_at);
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lists_persisted_jobs_by_latest_update_with_a_bounded_limit() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();
        let older = store.create_job("mesh.cleanup", None, &json!({})).unwrap();
        let active = store.create_job("mesh.simplify", None, &json!({})).unwrap();
        store
            .update_job(&active.id, JobState::Running, 25, None, None)
            .unwrap();
        let middle = store.create_job("mesh.compress", None, &json!({})).unwrap();

        {
            let connection = store.connection.lock().unwrap();
            for (id, updated_at) in [
                (&older.id, "000000000000001"),
                (&middle.id, "000000000000002"),
                (&active.id, "000000000000003"),
            ] {
                connection
                    .execute(
                        "UPDATE jobs SET updated_at=?1 WHERE id=?2",
                        rusqlite::params![updated_at, id],
                    )
                    .unwrap();
            }
        }

        let limited = store.list_jobs(2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].id, active.id);
        assert_eq!(limited[0].state, JobState::Running);
        assert_eq!(limited[1].id, middle.id);
        assert_eq!(store.list_jobs(0).unwrap().len(), 1);
        assert_eq!(store.list_jobs(500).unwrap().len(), 3);
        drop(store);

        let reopened = Store::open(&dir).unwrap();
        let persisted = reopened.list_jobs(10).unwrap();
        assert_eq!(
            persisted
                .iter()
                .map(|job| job.id.as_str())
                .collect::<Vec<_>>(),
            vec![active.id.as_str(), middle.id.as_str(), older.id.as_str()]
        );
        assert_eq!(
            reopened.get_job(&active.id).unwrap().unwrap().state,
            JobState::Running
        );
        drop(reopened);
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
    fn recovery_marks_only_interrupted_jobs_failed_and_is_idempotent() {
        let dir = temp_dir();
        let store = Store::open(&dir).unwrap();

        let queued = store.create_job("mesh.simplify", None, &json!({})).unwrap();
        let running = store.create_job("mesh.cleanup", None, &json!({})).unwrap();
        store
            .update_job(&running.id, JobState::Running, 25, None, None)
            .unwrap();
        let validating = store.create_job("uv.unwrap", None, &json!({})).unwrap();
        store
            .update_job(&validating.id, JobState::Running, 75, None, None)
            .unwrap();
        store
            .update_job(&validating.id, JobState::Validating, 90, None, None)
            .unwrap();

        let cancelled = store
            .create_job("mesh.separate_parts", None, &json!({}))
            .unwrap();
        store.cancel_job(&cancelled.id).unwrap();
        let succeeded = store.create_job("mesh.compress", None, &json!({})).unwrap();
        store
            .update_job(&succeeded.id, JobState::Running, 80, None, None)
            .unwrap();
        store
            .update_job(&succeeded.id, JobState::Validating, 90, None, None)
            .unwrap();
        store
            .update_job(
                &succeeded.id,
                JobState::Succeeded,
                100,
                Some(json!({"asset_id":"asset-existing"})),
                None,
            )
            .unwrap();
        let failed = store
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        store
            .update_job(&failed.id, JobState::Running, 30, None, None)
            .unwrap();
        store
            .update_job(
                &failed.id,
                JobState::Failed,
                100,
                None,
                Some(json!({"message":"known failure"})),
            )
            .unwrap();
        drop(store);

        // Recovery is an explicit operator action after the prior process exits.
        let store = Store::open(&dir).unwrap();
        let recovered = store.recover_interrupted_jobs().unwrap();
        assert_eq!(recovered.len(), 3);

        for (id, previous_state) in [
            (&queued.id, "queued"),
            (&running.id, "running"),
            (&validating.id, "validating"),
        ] {
            let job = store.get_job(id).unwrap().unwrap();
            assert_eq!(job.state, JobState::Failed);
            assert_eq!(job.progress, 100);
            let error = job.error.unwrap();
            assert_eq!(error["code"], "interrupted");
            assert_eq!(error["previous_state"], previous_state);
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("inspect existing assets")
            );
        }

        assert_eq!(
            store.get_job(&cancelled.id).unwrap().unwrap().state,
            JobState::Cancelled
        );
        let succeeded = store.get_job(&succeeded.id).unwrap().unwrap();
        assert_eq!(succeeded.state, JobState::Succeeded);
        assert_eq!(succeeded.result, Some(json!({"asset_id":"asset-existing"})));
        let failed = store.get_job(&failed.id).unwrap().unwrap();
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.error, Some(json!({"message":"known failure"})));
        assert!(store.recover_interrupted_jobs().unwrap().is_empty());
        assert!(store.list_assets(10).unwrap().is_empty());

        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_is_terminal_and_cannot_cross_the_asset_commit_boundary() {
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
        assert!(store.request_job_cancellation(&active.id, true).is_err());
        assert_eq!(
            store.get_job(&active.id).unwrap().unwrap().state,
            JobState::Running
        );
        assert_eq!(
            store.cancel_job(&active.id).unwrap().state,
            JobState::Cancelling
        );
        assert_eq!(
            store.cancel_job(&active.id).unwrap().state,
            JobState::Cancelling
        );
        assert!(store.job_cancellation_requested(&active.id).unwrap());
        assert!(
            store
                .update_job(&active.id, JobState::Validating, 90, None, None)
                .is_err()
        );
        store
            .update_job(&active.id, JobState::Cancelled, 100, None, None)
            .unwrap();
        assert!(
            store
                .update_job(&active.id, JobState::Running, 5, None, None)
                .is_err()
        );

        let saving = store
            .create_job("image.generate", None, &json!({}))
            .unwrap();
        store
            .update_job(&saving.id, JobState::Running, 5, None, None)
            .unwrap();
        store
            .update_job(&saving.id, JobState::Validating, 90, None, None)
            .unwrap();
        assert!(store.cancel_job(&saving.id).is_err());
        assert_eq!(
            store.get_job(&saving.id).unwrap().unwrap().state,
            JobState::Validating
        );
        let runtime = store
            .create_job("runtime.pixal3d.install_models", None, &json!({}))
            .unwrap();
        store
            .update_job(&runtime.id, JobState::Running, 5, None, None)
            .unwrap();
        assert!(store.cancel_job(&runtime.id).is_err());
        assert_eq!(
            store.get_job(&runtime.id).unwrap().unwrap().state,
            JobState::Running
        );
        drop(store);
        fs::remove_dir_all(dir).unwrap();
    }
}
