use std::sync::{Arc, LazyLock};

use axum::{
    Router,
    body::Bytes,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use meshharbor_core::{
    Asset, AssetPage, HuggingFaceAccess, HuggingFaceConnection, Job, JobStep, ModelVersionPage,
    OperationDescriptor, QuadRetopologyOptions, QuadRetopologyPreflight, QuadriFlowBackendStatus,
    QuadriFlowInstallResult, RuntimeDoctor,
};
use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Value, json};
use utoipa_swagger_ui::Config;

use crate::{
    ApiErrorBody, AppState, AssetPageQuery, CancelJobQuery, EnvironmentSetupConfirmation,
    GenerationQueueSwap, HuggingFaceConnectionRequest, JobHistoryQuery, ModelPageQuery,
    ModelVersionsQuery, RestoreModelVersion, RuntimeRemovalConfirmation, UploadQuery, actions,
    blender_bridge, browser_login,
};

const SPEC_PATH: &str = "/openapi.json";
static SPEC: LazyLock<Bytes> = LazyLock::new(|| Bytes::from(document().to_string()));
static UI_CONFIG: LazyLock<Arc<Config<'static>>> = LazyLock::new(|| {
    Arc::new(
        Config::new([SPEC_PATH])
            .use_base_layout()
            .filter(true)
            .query_config_enabled(false)
            .validator_url("none")
            .persist_authorization(false)
            .supported_submit_methods(["get"]),
    )
});

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/docs", get(|| async { Redirect::permanent("/docs/") }))
        .route("/docs/", get(|| async { ui_file("index.html") }))
        .route("/docs/{*asset}", get(ui_asset))
        .route(SPEC_PATH, get(spec))
}

async fn spec() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        SPEC.clone(),
    )
}

async fn ui_asset(Path(asset): Path<String>) -> Response {
    match asset.as_str() {
        "index.css"
        | "favicon-16x16.png"
        | "favicon-32x32.png"
        | "swagger-ui.css"
        | "swagger-ui-bundle.js"
        | "swagger-ui-standalone-preset.js"
        | "swagger-initializer.js" => ui_file(&asset),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn ui_file(path: &str) -> Response {
    match utoipa_swagger_ui::serve(path, Arc::clone(&UI_CONFIG)) {
        Ok(Some(file)) => (
            [
                (header::CONTENT_TYPE, file.content_type.as_str()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::X_FRAME_OPTIONS, "DENY"),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
                ),
            ],
            Bytes::from_owner(file.bytes),
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(%error, "Could not serve embedded Swagger UI");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn document() -> Value {
    let mut api = ApiDocument::new();

    api.add(
        "get",
        "/health",
        "Check server health",
        200,
        json!({"application/json": {"schema": {
            "type": "object",
            "required": ["status", "service"],
            "properties": {"status": {"const": "ok"}, "service": {"const": "meshharbor"}}
        }}}),
    )
    .operation["security"] = json!([]);
    api.get::<Vec<OperationDescriptor>>("/operations", "List available operations");
    api.get::<RuntimeDoctor>("/runtime", "Get runtime and model readiness");
    api.post::<Job>("/runtime/environment/setup", "Queue shared tool setup", 202)
        .body::<EnvironmentSetupConfirmation>();
    api.post::<browser_login::LoginView>(
        "/connections/huggingface/{scope}/sessions",
        "Start Hugging Face device sign-in",
        202,
    );
    api.get::<browser_login::LoginView>(
        "/connections/huggingface/{scope}/sessions/{session}",
        "Check the sign-in session",
    );
    api.json::<browser_login::LoginView>(
        "delete",
        "/connections/huggingface/{scope}/sessions/{session}",
        "Cancel the sign-in session",
        200,
    );
    api.post::<HuggingFaceConnection>(
        "/connections/huggingface/{scope}",
        "Check or save a Hugging Face token (Studio only)",
        200,
    )
    .body::<HuggingFaceConnectionRequest>()
    .studio_only();
    api.json::<RuntimeDoctor>(
        "delete",
        "/connections/huggingface/{scope}",
        "Remove saved local Hugging Face credentials",
        200,
    )
    .body::<RuntimeRemovalConfirmation>();
    api.post::<HuggingFaceConnection>(
        "/connections/huggingface/{scope}/check",
        "Check saved Hugging Face credentials",
        200,
    );
    api.post::<HuggingFaceAccess>(
        "/connections/huggingface/{scope}/model-access",
        "Check gated model access",
        200,
    );
    api.json::<RuntimeDoctor>(
        "delete",
        "/runtime/{backend}/model-files",
        "Remove managed model files",
        200,
    )
    .body::<RuntimeRemovalConfirmation>();
    api.post::<Job>(
        "/runtime/{backend}/install",
        "Queue model installation",
        202,
    )
    .body::<actions::InstallBackend>();
    api.get::<Vec<Job>>("/runtime/{backend}/jobs", "List recent runtime jobs");
    api.get::<QuadriFlowBackendStatus>("/runtime/quadriflow", "Check QuadriFlow availability");
    api.post::<QuadriFlowInstallResult>(
        "/runtime/quadriflow/install",
        "Install the verified QuadriFlow sidecar",
        200,
    );
    api.get::<AssetPage>("/assets", "Page through imported assets and mesh versions")
        .query::<AssetPageQuery>();
    api.post::<Asset>("/assets", "Import raw PNG, JPEG, or GLB bytes", 200)
        .query::<UploadQuery>()
        .request_content(binary_content(&[
            "application/octet-stream",
            "image/png",
            "image/jpeg",
            "model/gltf-binary",
        ]));
    api.get::<AssetPage>(
        "/models",
        "Page through the current version of each 3D model",
    )
    .query::<ModelPageQuery>();
    api.get::<Asset>("/assets/{asset_id}", "Get an asset");
    api.post::<Job>(
        "/assets/{asset_id}/simplify",
        "Queue mesh simplification",
        202,
    )
    .body::<actions::SimplifyMesh>();
    api.post::<Job>("/assets/{asset_id}/cleanup", "Queue mesh cleanup", 202)
        .body::<actions::CleanupMesh>();
    api.post::<Job>(
        "/assets/{asset_id}/voxel-remesh",
        "Queue voxel remeshing",
        202,
    )
    .body::<actions::VoxelRemesh>();
    api.post::<Job>("/assets/{asset_id}/unwrap-uv", "Queue UV atlas rebake", 202)
        .body::<actions::UnwrapUv>();
    api.post::<Job>(
        "/assets/{asset_id}/optimize-rendering",
        "Queue render-order optimization",
        202,
    )
    .body::<actions::OptimizeRendering>();
    api.post::<Job>("/assets/{asset_id}/compress", "Queue mesh compression", 202)
        .body::<actions::CompressMesh>();
    api.post::<Job>(
        "/assets/{asset_id}/retopology",
        "Queue QuadriFlow retopology",
        202,
    )
    .body::<QuadRetopologyOptions>();
    api.post::<Job>(
        "/assets/{asset_id}/retopology-lato2",
        "Queue LATO.2 retopology",
        202,
    )
    .body::<actions::RetopologizeLato2>();
    api.post::<QuadRetopologyPreflight>(
        "/assets/{asset_id}/retopology-preflight",
        "Check mesh eligibility for QuadriFlow",
        200,
    )
    .body::<QuadRetopologyOptions>();
    api.get::<ModelVersionPage>("/models/{model_id}/versions", "Page through model versions")
        .query::<ModelVersionsQuery>();
    api.json::<Asset>(
        "put",
        "/models/{model_id}/current",
        "Restore a model version",
        200,
    )
    .body::<RestoreModelVersion>();
    api.add(
        "get",
        "/artifacts/{artifact_id}",
        "Download stored artifact bytes (content type varies)",
        200,
        binary_content(&[
            "application/octet-stream",
            "model/gltf-binary",
            "image/png",
            "image/jpeg",
            "model/obj",
        ]),
    );

    api.get::<Vec<Job>>("/jobs", "List persisted jobs")
        .query::<JobHistoryQuery>();
    api.post::<Job>("/generation", "Generate a new 3D model", 202)
        .body::<actions::GenerateModel>()
        .operation["requestBody"]["content"]["application/json"]["example"] = json!({
        "input": {"kind": "image", "asset_id": "asset_example"},
        "parameters": {"backend_id": "auto"}
    });
    api.get::<Vec<String>>("/generation/queue", "List IDs in the generation queue");
    api.post::<Vec<String>>(
        "/generation/queue",
        "Swap adjacent queued generation jobs",
        200,
    )
    .body::<GenerationQueueSwap>();
    api.get::<Job>("/jobs/{job_id}", "Get a job");
    api.get::<Vec<JobStep>>(
        "/jobs/{job_id}/steps",
        "Get execution steps and command logs",
    );
    api.post::<Job>("/jobs/{job_id}/cancel", "Request job cancellation", 200)
        .query::<CancelJobQuery>();
    api.post::<Job>(
        "/jobs/{job_id}/retry",
        "Retry the same saved job, with explicit confirmation for downloads",
        202,
    )
    .body::<actions::RetryJob>();

    api.add(
        "get",
        "/blender",
        "Check if Blender is connected",
        200,
        json!({"application/json": {"schema": {
            "type": "object",
            "required": ["connected"],
            "properties": {"connected": {"type": "boolean"}}
        }}}),
    );
    api.add(
        "get",
        "/blender/addon",
        "Download the Blender add-on",
        200,
        binary_content(&["application/octet-stream"]),
    );
    api.post::<blender_bridge::InstallResult>(
        "/blender/install",
        "Install and enable the Blender add-on (Studio only)",
        200,
    )
    .studio_only();
    api.post::<blender_bridge::SendStatus>("/blender/send", "Send a model version to Blender", 202)
        .body::<blender_bridge::SendRequest>();
    api.get::<blender_bridge::SendStatus>("/blender/send/{id}", "Check a Blender transfer");

    api.finish()
}

struct ApiDocument {
    paths: Value,
    requests: SchemaGenerator,
    responses: SchemaGenerator,
}

impl ApiDocument {
    fn new() -> Self {
        let mut settings = SchemaSettings::draft2020_12();
        settings.definitions_path = "/components/schemas".into();
        Self {
            paths: json!({}),
            requests: settings.clone().into_generator(),
            responses: settings.for_serialize().into_generator(),
        }
    }

    fn get<Body: JsonSchema>(&mut self, path: &str, summary: &str) -> Operation<'_> {
        self.json::<Body>("get", path, summary, 200)
    }

    fn post<Body: JsonSchema>(&mut self, path: &str, summary: &str, status: u16) -> Operation<'_> {
        self.json::<Body>("post", path, summary, status)
    }

    fn json<Body: JsonSchema>(
        &mut self,
        method: &str,
        path: &str,
        summary: &str,
        status: u16,
    ) -> Operation<'_> {
        let schema = self.responses.subschema_for::<Body>();
        self.add(
            method,
            path,
            summary,
            status,
            json!({"application/json": {"schema": schema}}),
        )
    }

    fn add(
        &mut self,
        method: &str,
        path: &str,
        summary: &str,
        status: u16,
        content: Value,
    ) -> Operation<'_> {
        let parameters: Vec<_> = path
            .split('/')
            .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
            .map(|name| {
                let schema = if matches!(name, "id" | "session") {
                    json!({"type": "string", "format": "uuid"})
                } else if name == "scope" {
                    json!({"type": "string", "enum": ["mlx", "cuda"]})
                } else {
                    json!({"type": "string"})
                };
                json!({"name": name, "in": "path", "required": true, "schema": schema})
            })
            .collect();
        let tag = match path.split('/').nth(1).unwrap_or_default() {
            "assets" if path.split('/').count() == 4 => "Mesh editing",
            "runtime" => "Runtime",
            "connections" => "Connections",
            "generation" => "Generation",
            "assets" | "artifacts" | "models" => "Assets and models",
            "jobs" => "Jobs",
            "blender" => "Blender",
            _ => "Server",
        };
        let status = StatusCode::from_u16(status).expect("valid OpenAPI response status");
        let operation = &mut self.paths[format!("/api/v1{path}")][method];
        *operation = json!({
            "summary": summary,
            "tags": [tag],
            "parameters": parameters,
            "responses": {
                (status.as_str()): {
                    "description": status.canonical_reason().unwrap_or("Success"),
                    "content": content
                },
                "default": {"$ref": "#/components/responses/ApiError"}
            }
        });
        Operation {
            operation,
            schemas: &mut self.requests,
        }
    }

    fn finish(mut self) -> Value {
        let error = self.responses.subschema_for::<ApiErrorBody>();
        let mut schemas = self.responses.take_definitions(true);
        schemas.extend(self.requests.take_definitions(true));
        json!({
            "openapi": "3.1.0",
            "info": {
                "title": "MeshHarbor REST API",
                "version": env!("CARGO_PKG_VERSION"),
                "description": "Single-user loopback API. Use the server's API token for bearer authentication. Studio-only operations use the dashboard marker and browser same-origin checks. The native Blender companion protocol is internal."
            },
            "servers": [{"url": "/"}],
            "security": [{"bearerAuth": []}, {"studioDashboard": []}],
            "paths": self.paths,
            "components": {
                "securitySchemes": {
                    "bearerAuth": {
                        "type": "http", "scheme": "bearer",
                        "description": "Use MESHHARBOR_API_TOKEN or the temporary token printed by the running server."
                    },
                    "studioDashboard": {
                        "type": "apiKey", "in": "header", "name": "X-MeshHarbor-Dashboard",
                        "description": "The built-in Studio sends this marker with browser-enforced same-origin provenance."
                    }
                },
                "responses": {
                    "ApiError": {
                        "description": "Request denied, invalid, unavailable, or failed.",
                        "content": {
                            "application/json": {"schema": error},
                            "text/plain": {"schema": {"type": "string"}}
                        }
                    }
                },
                "schemas": schemas
            }
        })
    }
}

struct Operation<'a> {
    operation: &'a mut Value,
    schemas: &'a mut SchemaGenerator,
}

impl Operation<'_> {
    fn query<Query: JsonSchema>(self) -> Self {
        let schema = Query::json_schema(self.schemas).to_value();
        if let Some(properties) = schema["properties"].as_object() {
            let parameters = self.operation["parameters"].as_array_mut().unwrap();
            for (name, property) in properties {
                let required = schema["required"]
                    .as_array()
                    .is_some_and(|fields| fields.iter().any(|field| field == name.as_str()));
                parameters.push(json!({
                    "name": name,
                    "in": "query",
                    "required": required,
                    "schema": property
                }));
            }
        }
        self
    }

    fn body<Body: JsonSchema>(self) -> Self {
        let schema = self.schemas.subschema_for::<Body>();
        self.request_content(json!({"application/json": {"schema": schema}}))
    }

    fn request_content(self, content: Value) -> Self {
        self.operation["requestBody"] = json!({"required": true, "content": content});
        self
    }

    fn studio_only(self) {
        self.operation["security"] = json!([{"studioDashboard": []}]);
    }
}

fn binary_content(media_types: &[&str]) -> Value {
    Value::Object(
        media_types
            .iter()
            .map(|media_type| ((*media_type).into(), json!({})))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn local_request(path: &str) -> axum::http::request::Builder {
        Request::builder()
            .uri(path)
            .header(header::HOST, "localhost:7432")
    }

    fn app(root: &std::path::Path) -> Router {
        crate::api_router(
            AppState {
                core: meshharbor_core::OperationCore::open(root).unwrap(),
                scheduler: Arc::default(),
            },
            "docs-test-token".into(),
        )
    }

    #[tokio::test]
    async fn docs_and_ui_assets_are_available_only_on_loopback() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path());
        let redirect = app
            .clone()
            .oneshot(local_request("/docs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(redirect.headers()[header::LOCATION], "/docs/");

        for (path, marker) in [
            ("/docs/", "swagger-ui-bundle.js"),
            ("/docs/swagger-initializer.js", SPEC_PATH),
            ("/docs/swagger-ui-bundle.js", "SwaggerUIBundle"),
            ("/docs/swagger-ui.css", ".swagger-ui"),
        ] {
            let response = app
                .clone()
                .oneshot(local_request(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert!(
                response.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("connect-src 'self'")
            );
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body = std::str::from_utf8(&body).unwrap();
            assert!(body.contains(marker), "Missing {marker} in {path}");
            if path.ends_with("swagger-initializer.js") {
                let compact: String = body
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect();
                assert!(compact.contains(r#""validatorUrl":"none""#));
                assert!(compact.contains(r#""supportedSubmitMethods":["get"]"#));
            }
        }

        for path in ["/docs/", "/docs/swagger-ui-bundle.js", SPEC_PATH] {
            for (host, origin) in [
                ("not-local.example", None),
                ("localhost:7432", Some("http://localhost:7000")),
            ] {
                let mut request = Request::builder().uri(path).header(header::HOST, host);
                if let Some(origin) = origin {
                    request = request.header(header::ORIGIN, origin);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            }
        }

        for (path, expected) in [
            ("/docs/unknown.js", StatusCode::NOT_FOUND),
            ("/api/v1/jobs", StatusCode::UNAUTHORIZED),
        ] {
            let response = app
                .clone()
                .oneshot(local_request(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
        }
    }

    #[tokio::test]
    async fn openapi_document_matches_served_routes() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path());
        let response = app
            .clone()
            .oneshot(local_request(SPEC_PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let spec: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(spec["openapi"], "3.1.0");
        assert_eq!(
            spec["paths"]["/api/v1/health"]["get"]["security"],
            json!([])
        );
        assert_eq!(
            spec["paths"]["/api/v1/blender/install"]["post"]["security"],
            json!([{"studioDashboard": []}])
        );
        assert_eq!(
            spec["components"]["securitySchemes"]["bearerAuth"]["scheme"],
            "bearer"
        );
        assert!(spec["components"]["schemas"]["GenerateModel"]["properties"]["input"].is_object());
        assert!(spec["paths"]["/api/v1/generation"]["post"]["responses"]["202"].is_object());
        assert!(spec["paths"]["/api/v1/jobs"]["post"].is_null());
        assert!(
            !std::str::from_utf8(&bytes)
                .unwrap()
                .contains("docs-test-token")
        );
        assert_references_resolve(&spec, &spec);

        for (path, operations) in spec["paths"].as_object().unwrap() {
            let concrete_path = path
                .split('/')
                .map(|segment| {
                    if segment.starts_with('{') {
                        "test-id"
                    } else {
                        segment
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            let response = app
                .clone()
                .oneshot(
                    local_request(&concrete_path)
                        .method(Method::OPTIONS)
                        .header(header::AUTHORIZATION, "Bearer docs-test-token")
                        .header(header::ORIGIN, "http://localhost:7432")
                        .header("x-meshharbor-dashboard", "local-ui-v1")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
            let allowed = response.headers()[header::ALLOW].to_str().unwrap();
            for method in operations.as_object().unwrap().keys() {
                assert!(
                    allowed
                        .split(',')
                        .any(|entry| entry.trim().eq_ignore_ascii_case(method)),
                    "{method} {path} is not routed"
                );
            }
        }
    }

    fn assert_references_resolve(node: &Value, spec: &Value) {
        match node {
            Value::Object(fields) => {
                if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
                    let pointer = reference.strip_prefix('#').expect("local reference");
                    assert!(spec.pointer(pointer).is_some(), "Missing {reference}");
                }
                for child in fields.values() {
                    assert_references_resolve(child, spec);
                }
            }
            Value::Array(items) => {
                for child in items {
                    assert_references_resolve(child, spec);
                }
            }
            _ => {}
        }
    }
}
