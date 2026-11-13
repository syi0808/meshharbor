//! Model-free end-to-end tests for the public generation surfaces.
//!
//! These tests run only on Unix because their mock Python executable is a tiny
//! shell shim. It copies a valid fixture GLB, so no model, network access, or
//! Python package installation is required.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const IMAGE_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xb0, 0xa9, 0x38, 0x01,
    0x00, 0x02, 0x70, 0x01, 0x7d, 0xe5, 0xc6, 0x55, 0x54, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

const PIXAL_CHECKPOINTS: &[&str] = &[
    "ss_flow_img_dit_1_3B_64_bf16.safetensors",
    "ss_dec_conv3d_16l8_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_512_bf16.safetensors",
    "shape_dec_next_dc_f16c32_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_1024_bf16.safetensors",
    "slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors",
    "tex_dec_next_dc_f16c32_fp16.safetensors",
];

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("meshharbor-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("create isolated test directory");
        Self(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct MockRuntime {
    _tree: TempTree,
    runtime_root: PathBuf,
    source_root: PathBuf,
    python: PathBuf,
    weights: PathBuf,
    generated_glb: Vec<u8>,
}

impl MockRuntime {
    fn new(label: &str) -> Self {
        let tree = TempTree::new(label);
        let runtime_root = tree.0.join("runtime");
        let source_root = tree.0.join("mock-source");
        let python = tree.0.join("fake-python");
        let weights = tree.0.join("mock-weights");
        let generated_glb = minimal_triangle_glb();

        fs::create_dir_all(source_root.join("pixalmlx")).unwrap();
        fs::create_dir_all(&weights).unwrap();
        fs::write(
            source_root.join("generate_pixal3d.py"),
            "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")\n",
        )
        .unwrap();
        fs::write(source_root.join("fixture.glb"), &generated_glb).unwrap();
        fs::write(weights.join("naf_release.safetensors"), b"mock checkpoint").unwrap();

        // The production launcher clears the Python environment and uses the
        // source root as cwd. A shell executable is sufficient because the
        // only behavior under test is the CLI contract and GLB validation.
        fs::write(
            &python,
            "#!/bin/sh\nset -eu\ninput= output= resolution= texture_size= target_faces=\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --image) shift; input=$1 ;;\n    --output) shift; output=$1 ;;\n    --resolution) shift; resolution=$1 ;;\n    --texture-size) shift; texture_size=$1 ;;\n    --target-faces) shift; target_faces=$1 ;;\n  esac\n  shift\ndone\n[ -n \"$input\" ] && [ -s \"$input\" ]\n[ -n \"$output\" ]\n[ \"$resolution\" = 512 ]\n[ \"$texture_size\" = 512 ] || [ \"$texture_size\" = 1024 ] || [ \"$texture_size\" = 2048 ]\n[ \"$target_faces\" -ge 4 ]\n[ \"$HF_HUB_OFFLINE\" = 1 ] && [ \"$TRANSFORMERS_OFFLINE\" = 1 ]\n[ -z \"${HF_TOKEN-}\" ]\ncp \"fixture.glb\" \"$output\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&python).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&python, permissions).unwrap();

        let hub = runtime_root.join("home/.cache/huggingface/hub");
        let pixal = hub.join(
            "models--TencentARC--Pixal3D/snapshots/0b31f9160aa400719af409098bff7936a932f726/ckpts",
        );
        fs::create_dir_all(&pixal).unwrap();
        for checkpoint in PIXAL_CHECKPOINTS {
            fs::write(pixal.join(checkpoint), b"test placeholder; never loaded").unwrap();
        }
        let dino = hub.join(
            "models--facebook--dinov3-vitl16-pretrain-lvd1689m/snapshots/ea8dc2863c51be0a264bab82070e3e8836b02d51",
        );
        fs::create_dir_all(&dino).unwrap();
        fs::write(
            dino.join("mock.safetensors"),
            b"test placeholder; never loaded",
        )
        .unwrap();

        Self {
            _tree: tree,
            runtime_root,
            source_root,
            python,
            weights,
            generated_glb,
        }
    }

    fn configure(&self, command: &mut Command) {
        command
            .env("MESHHARBOR_PIXAL3D_MLX_HOME", &self.runtime_root)
            .env("MESHHARBOR_PIXAL3D_MLX_ROOT", &self.source_root)
            .env("MESHHARBOR_PIXAL3D_MLX_PYTHON", &self.python)
            .env("MESHHARBOR_PIXAL3D_WEIGHTS_DIR", &self.weights)
            .env("HF_TOKEN", "test-token-must-not-reach-worker");
    }
}

fn meshharbor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_meshharbor"))
}

fn run_cli(runtime: &MockRuntime, data_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = meshharbor();
    command.arg("--data-dir").arg(data_dir).args(args);
    runtime.configure(&mut command);
    command.output().expect("launch meshharbor CLI")
}

#[test]
fn cli_import_generate_and_export_uses_mock_runtime_without_models() {
    let runtime = MockRuntime::new("cli-generation");
    let data_dir = runtime._tree.0.join("store");
    let image = runtime._tree.0.join("input.png");
    fs::write(&image, IMAGE_PNG).unwrap();

    let imported = run_cli(&runtime, &data_dir, &["import", image.to_str().unwrap()]);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let asset: Value = serde_json::from_slice(&imported.stdout).unwrap();
    let asset_id = asset["id"].as_str().expect("imported image asset id");

    let output_glb = runtime._tree.0.join("cli-result.glb");
    let generated = run_cli(
        &runtime,
        &data_dir,
        &[
            "generate",
            asset_id,
            "--backend-id",
            "pixal3d-mlx",
            "--output",
            output_glb.to_str().unwrap(),
        ],
    );
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let result: Value = serde_json::from_slice(&generated.stdout).unwrap();
    assert_eq!(result["backend_id"], "pixal3d-mlx");
    assert_eq!(fs::read(output_glb).unwrap(), runtime.generated_glb);
}

#[test]
fn rest_upload_job_poll_and_artifact_download_share_the_generation_core() {
    let runtime = MockRuntime::new("rest-generation");
    let data_dir = runtime._tree.0.join("store");
    let auth_token = "mock-e2e-token";
    let authorization = format!("Bearer {auth_token}");
    let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let address = SocketAddr::from(([127, 0, 0, 1], port));

    let mut command = meshharbor();
    let listen = format!("127.0.0.1:{port}");
    command
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["serve", "--listen"])
        .arg(&listen)
        .env("MESHHARBOR_API_TOKEN", auth_token)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    runtime.configure(&mut command);
    let mut server = ChildGuard(command.spawn().expect("launch REST server"));

    let response = wait_for_health(&address, &mut server.0);
    assert_eq!(response.0, 200, "REST API failed to become ready");

    let (status, body) = http_request(
        address,
        "POST",
        "/api/v1/assets?filename=source.png",
        &[
            ("Authorization", authorization.as_str()),
            ("Content-Type", "image/png"),
        ],
        IMAGE_PNG,
    )
    .unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let source: Value = serde_json::from_slice(&body).unwrap();
    let source_id = source["id"].as_str().unwrap();

    let job_body = json!({
        "operation": "image.generate",
        "input": { "asset_id": source_id },
        "parameters": {
            "backend_id": "pixal3d-mlx",
            "seed": 17,
            "resolution": 512,
            "texture_size": 512,
            "target_faces": 64
        }
    });
    let job_json = serde_json::to_vec(&job_body).unwrap();
    let (status, body) = http_request(
        address,
        "POST",
        "/api/v1/jobs",
        &[
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
        ],
        &job_json,
    )
    .unwrap();
    assert_eq!(status, 202, "{}", String::from_utf8_lossy(&body));
    let submitted: Value = serde_json::from_slice(&body).unwrap();
    let job_id = submitted["id"].as_str().unwrap();

    let mut completed = None;
    for _ in 0..200 {
        let (status, body) = http_request(
            address,
            "GET",
            &format!("/api/v1/jobs/{job_id}"),
            &[("Authorization", authorization.as_str())],
            &[],
        )
        .unwrap();
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let job: Value = serde_json::from_slice(&body).unwrap();
        if job["state"] == "succeeded" || job["state"] == "failed" {
            completed = Some(job);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let completed = completed.expect("generation job should reach a terminal state");
    assert_eq!(completed["state"], "succeeded", "job: {completed}");
    assert_eq!(completed["result"]["backend_id"], "pixal3d-mlx");
    let generated_asset_id = completed["result"]["asset_id"].as_str().unwrap();

    let (status, body) = http_request(
        address,
        "GET",
        &format!("/api/v1/assets/{generated_asset_id}"),
        &[("Authorization", authorization.as_str())],
        &[],
    )
    .unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let asset: Value = serde_json::from_slice(&body).unwrap();
    let derived = asset["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["kind"] == "derived")
        .expect("generated mesh has a derived GLB artifact");
    let artifact_id = derived["id"].as_str().unwrap();
    let (status, artifact_bytes) = http_request(
        address,
        "GET",
        &format!("/api/v1/artifacts/{artifact_id}"),
        &[("Authorization", authorization.as_str())],
        &[],
    )
    .unwrap();
    assert_eq!(status, 200);
    assert_eq!(artifact_bytes, runtime.generated_glb);

    server.terminate();
}

#[test]
fn mcp_import_generation_and_glb_export_use_the_same_mock_generation_core() {
    let runtime = MockRuntime::new("mcp-generation");
    let data_dir = runtime._tree.0.join("store");
    let export_dir = runtime._tree.0.join("exports");
    fs::create_dir_all(&export_dir).unwrap();

    let mut command = meshharbor();
    command
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("mcp")
        .env("MESHHARBOR_EXPORT_DIR", &export_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    runtime.configure(&mut command);
    let mut child = command.spawn().expect("launch MCP server");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (line_tx, line_rx) = mpsc::channel::<std::io::Result<String>>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut client = McpClient {
        child,
        stdin,
        lines: line_rx,
        next_id: 1,
    };

    let initialized = client.request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "meshharbor-mock-e2e", "version": "0.1.0" }
        }),
    );
    assert!(
        initialized.get("result").is_some(),
        "MCP initialize: {initialized}"
    );
    client.notify("notifications/initialized", json!({}));

    let imported = client.tool(
        "import_asset",
        json!({
            "filename": "source.png",
            "bytes_base64": encode_base64(IMAGE_PNG)
        }),
    );
    let source_id = imported["id"]
        .as_str()
        .expect("MCP import asset id")
        .to_owned();

    let submitted = client.tool(
        "start_image_generation",
        json!({
            "asset_id": source_id,
            "backend_id": "pixal3d-mlx",
            "seed": 23,
            "resolution": 512,
            "texture_size": 512,
            "target_faces": 64
        }),
    );
    let job_id = submitted["id"]
        .as_str()
        .expect("MCP generation job id")
        .to_owned();
    let mut completed = None;
    for _ in 0..200 {
        let job = client.tool("get_job", json!({"job_id": job_id}));
        if job["state"] == "succeeded" || job["state"] == "failed" {
            completed = Some(job);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let completed = completed.expect("MCP generation job should reach terminal state");
    assert_eq!(completed["state"], "succeeded", "job: {completed}");
    assert_eq!(completed["result"]["backend_id"], "pixal3d-mlx");
    let generated_asset_id = completed["result"]["asset_id"].as_str().unwrap();
    let destination = export_dir.join("generated.glb");
    let exported = client.tool(
        "export_glb",
        json!({
            "asset_id": generated_asset_id,
            "output_path": destination.to_string_lossy()
        }),
    );
    assert_eq!(exported["overwritten"], false);
    assert_eq!(fs::read(destination).unwrap(), runtime.generated_glb);

    client.shutdown();
}

struct McpClient {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<std::io::Result<String>>,
    next_id: u64,
}

impl McpClient {
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        serde_json::to_writer(
            &mut self.stdin,
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )
        .unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(remaining)
                .expect("MCP response timed out")
                .expect("read MCP response line");
            let value: Value = serde_json::from_str(&line).expect("MCP emits JSON-RPC lines");
            if value["id"] == id {
                return value;
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        serde_json::to_writer(
            &mut self.stdin,
            &json!({"jsonrpc":"2.0", "method":method, "params":params}),
        )
        .unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(
            response.get("error").is_none(),
            "MCP {name} RPC error: {response}"
        );
        assert_ne!(
            response["result"]["isError"], true,
            "MCP {name} returned error: {response}"
        );
        let text = response["result"]["content"]
            .as_array()
            .and_then(|content| content.iter().find_map(|item| item["text"].as_str()))
            .expect("MCP tool returns a text result");
        serde_json::from_str(text)
            .unwrap_or_else(|error| panic!("MCP {name} returned invalid JSON ({error}): {text}"))
    }

    fn shutdown(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    fn terminate(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn wait_for_health(address: &SocketAddr, child: &mut Child) -> (u16, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("check REST server status") {
            panic!("REST server exited before becoming ready: {status}");
        }
        if let Ok(response) = http_request(*address, "GET", "/api/v1/health", &[], &[]) {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "REST server did not start before timeout"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn http_request(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;

    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("invalid HTTP response headers"))?;
    let head = String::from_utf8_lossy(&raw[..header_end]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| std::io::Error::other("missing HTTP status"))?;
    let response_body = &raw[header_end + 4..];
    let chunked = head
        .lines()
        .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"));
    let body = if chunked {
        decode_chunked(response_body)?
    } else {
        response_body.to_vec()
    };
    Ok((status, body))
}

fn decode_chunked(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    loop {
        let line_end = bytes[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or_else(|| std::io::Error::other("invalid HTTP chunk size"))?;
        let size_text = std::str::from_utf8(&bytes[cursor..line_end])
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        cursor = line_end + 2;
        if size == 0 {
            return Ok(output);
        }
        let end = cursor
            .checked_add(size)
            .ok_or_else(|| std::io::Error::other("HTTP chunk size overflow"))?;
        let chunk = bytes
            .get(cursor..end)
            .ok_or_else(|| std::io::Error::other("truncated HTTP chunk"))?;
        output.extend_from_slice(chunk);
        if bytes
            .get(end..end + 2)
            .is_none_or(|terminator| terminator != b"\r\n")
        {
            return Err(std::io::Error::other("invalid HTTP chunk terminator"));
        }
        cursor = end + 2;
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(a >> 2) as usize] as char);
        encoded.push(ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

fn minimal_triangle_glb() -> Vec<u8> {
    let mut binary = Vec::new();
    for position in [
        [0.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
    ] {
        for component in position {
            binary.extend_from_slice(&component.to_le_bytes());
        }
    }
    let index_offset = binary.len();
    for index in [0u16, 2, 1, 1, 2, 3] {
        binary.extend_from_slice(&index.to_le_bytes());
    }
    let document = json!({
        "asset":{"version":"2.0"},
        "buffers":[{"byteLength":binary.len()}],
        "bufferViews":[
            {"buffer":0,"byteOffset":0,"byteLength":48,"byteStride":12,"target":34962},
            {"buffer":0,"byteOffset":index_offset,"byteLength":12,"target":34963}
        ],
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"},
            {"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}
        ],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}]
    });
    let mut json_bytes = serde_json::to_vec(&document).unwrap();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let total_len = 12 + 8 + json_bytes.len() + 8 + binary.len();
    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(total_len as u32).to_le_bytes());
    bytes.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4e4f_534au32.to_le_bytes());
    bytes.extend_from_slice(&json_bytes);
    bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x004e_4942u32.to_le_bytes());
    bytes.extend_from_slice(&binary);
    bytes
}
