use std::{net::SocketAddr, path::Path, process::Stdio, time::Duration};

use meshharbor_core::OperationCore;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{sleep, timeout},
};
use uuid::Uuid;

const TOKEN: &str = "fixture-blender-token";
const WAIT: Duration = Duration::from_secs(10);

fn meshharbor(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_meshharbor"));
    command.arg("--data-dir").arg(root).kill_on_drop(true);
    command
}

async fn server(root: &Path) -> (Child, SocketAddr) {
    let mut child = meshharbor(root)
        .args(["serve", "--listen", "127.0.0.1:0"])
        .env("MESHHARBOR_API_TOKEN", TOKEN)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut logs = BufReader::new(child.stderr.take().unwrap()).lines();
    let address = timeout(WAIT, async {
        while let Some(line) = logs.next_line().await.unwrap() {
            if let Some((address, _)) = line
                .strip_prefix("MeshHarbor REST API listening at http://")
                .and_then(|line| line.split_once(';'))
            {
                return address.parse().unwrap();
            }
        }
        panic!("MeshHarbor exited before starting");
    })
    .await
    .unwrap();
    tokio::spawn(async move { while let Ok(Some(_)) = logs.next_line().await {} });
    (child, address)
}

struct Mcp {
    _child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Mcp {
    async fn start(root: &Path, address: SocketAddr) -> Self {
        let mut child = meshharbor(root)
            .arg("mcp")
            .env("MESHHARBOR_API_TOKEN", TOKEN)
            .env("MESHHARBOR_SERVER", address.to_string())
            .env("MESHHARBOR_EXPORT_DIR", root.join("exports"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut mcp = Self {
            _child: child,
            input,
            output,
            next_id: 0,
        };
        mcp.call(
            "initialize",
            json!({
                "protocolVersion":"2024-11-05", "capabilities":{},
                "clientInfo":{"name":"blender-test","version":"1"},
            }),
        )
        .await;
        mcp.write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        mcp
    }

    async fn write(&mut self, data: Value) {
        self.input
            .write_all(format!("{data}\n").as_bytes())
            .await
            .unwrap();
        self.input.flush().await.unwrap();
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"jsonrpc":"2.0","method":method,"params":params,"id":id}))
            .await;
        timeout(WAIT, async {
            while let Some(line) = self.output.next_line().await.unwrap() {
                let response: Value = serde_json::from_str(&line).unwrap();
                if response["id"] == id {
                    assert!(response.get("error").is_none(), "{response}");
                    return response["result"].clone();
                }
            }
            panic!("MCP stdout closed");
        })
        .await
        .unwrap()
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self
            .call("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        let text = result["content"][0]["text"]
            .as_str()
            .expect("tool response");
        serde_json::from_str(text).unwrap()
    }
}

async fn receive_model(
    client: &Client,
    origin: &str,
    client_id: Uuid,
    expected: &[u8],
    error: Option<&str>,
) -> String {
    timeout(WAIT, async {
        loop {
            let response = client
                .post(format!("{origin}/api/v1/blender/client/poll"))
                .header("x-meshharbor-blender", "local-addon-v1")
                .json(&json!({"client_id":client_id}))
                .send()
                .await
                .unwrap();
            if response.status() == StatusCode::NO_CONTENT {
                sleep(Duration::from_millis(30)).await;
                continue;
            }
            assert_eq!(response.status(), StatusCode::OK);
            let id = response.headers()["x-meshharbor-transfer"]
                .to_str()
                .unwrap()
                .to_owned();
            assert_eq!(response.bytes().await.unwrap().as_ref(), expected);
            let ack = client
                .post(format!("{origin}/api/v1/blender/client/result/{id}"))
                .header("x-meshharbor-blender", "local-addon-v1")
                .json(&json!({"client_id":client_id,"error":error}))
                .send()
                .await
                .unwrap();
            assert_eq!(ack.status(), StatusCode::NO_CONTENT);
            return id;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn cli_and_mcp_send_to_the_same_open_scene_without_studio() {
    let root = tempfile::tempdir().unwrap();
    let glb = triangle_glb();
    let asset = OperationCore::open(root.path())
        .unwrap()
        .import_asset("triangle.glb", &glb)
        .unwrap();
    let (_server, address) = server(root.path()).await;
    let origin = format!("http://{address}");
    let http = Client::builder().no_proxy().timeout(WAIT).build().unwrap();
    let status = meshharbor(root.path())
        .args([
            "engine",
            "blender",
            "status",
            "--server",
            &address.to_string(),
        ])
        .env("MESHHARBOR_API_TOKEN", TOKEN)
        .output()
        .await
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap(),
        json!({"connected":false})
    );
    let mut cli = meshharbor(root.path());
    cli.args([
        "engine",
        "--server",
        &address.to_string(),
        "blender",
        "send",
        &asset.id,
    ])
    .env("MESHHARBOR_API_TOKEN", TOKEN);
    let not_connected = cli.output().await.unwrap();
    assert!(!not_connected.status.success());
    assert!(String::from_utf8_lossy(&not_connected.stderr).contains("Blender is not connected"));

    let client_id = Uuid::new_v4();
    let ready = http
        .post(format!("{origin}/api/v1/blender/client/poll"))
        .header("x-meshharbor-blender", "local-addon-v1")
        .json(&json!({"client_id":client_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::NO_CONTENT);
    cli.env("MESHHARBOR_API_TOKEN", "wrong-token");
    let unauthorized = cli.output().await.unwrap();
    assert!(!unauthorized.status.success());
    assert!(String::from_utf8_lossy(&unauthorized.stderr).contains("MESHHARBOR_API_TOKEN"));

    cli.env("MESHHARBOR_API_TOKEN", TOKEN);
    let (sent, cli_id) = tokio::join!(
        timeout(WAIT, cli.output()),
        receive_model(&http, &origin, client_id, &glb, None)
    );
    let sent = sent.unwrap().unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let completed: Value = serde_json::from_slice(&sent.stdout).unwrap();
    assert_eq!(completed["id"], cli_id);
    assert_eq!(completed["stage"], "waiting");

    let mut mcp = Mcp::start(root.path(), address).await;
    let addon = root.path().join("exports/meshharbor_bridge.py");
    let exported = mcp
        .tool(
            "engine_blender",
            json!({"request":{"action":"download_addon","output_path":addon}}),
        )
        .await;
    let addon_source = std::fs::read_to_string(&addon).unwrap();
    assert_eq!(exported["byte_size"], addon_source.len());
    assert!(addon_source.contains(&origin));
    let refused = mcp
        .tool("install_blender_addon", json!({"confirmed":false}))
        .await;
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("explicit owner approval")
    );
    let sent = mcp
        .tool(
            "engine_blender",
            json!({"request":{"action":"send","asset_id":asset.id}}),
        )
        .await;
    assert_eq!(sent["stage"], "waiting", "{sent}");
    let mcp_id = receive_model(
        &http,
        &origin,
        client_id,
        &glb,
        Some("Cannot import in this scene."),
    )
    .await;
    assert_eq!(sent["id"], mcp_id);
    let failed = mcp
        .tool(
            "engine_blender",
            json!({"request":{"action":"transfer","transfer_id":mcp_id}}),
        )
        .await;
    assert_eq!(failed["stage"], "failed");
    assert_eq!(failed["error"], "Cannot import in this scene.");
    let old = mcp
        .tool(
            "engine_blender",
            json!({"request":{"action":"transfer","transfer_id":cli_id}}),
        )
        .await;
    assert_eq!(old["stage"], "succeeded");

    let checked = meshharbor(root.path())
        .args(["engine", "blender", "transfer", &mcp_id])
        .env("MESHHARBOR_SERVER", address.to_string())
        .env("MESHHARBOR_API_TOKEN", TOKEN)
        .output()
        .await
        .unwrap();
    assert!(checked.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&checked.stdout).unwrap(),
        failed
    );
}

fn triangle_glb() -> Vec<u8> {
    let mut json = serde_json::to_vec(&json!({
        "asset":{"version":"2.0"},
        "buffers":[{"byteLength":36}],
        "bufferViews":[{"buffer":0,"byteLength":36}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
        "scene":0, "scenes":[{"nodes":[0]}], "nodes":[{"mesh":0}]
    })).unwrap();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut glb = b"glTF".to_vec();
    glb.extend(2u32.to_le_bytes());
    glb.extend((12 + 8 + json.len() as u32 + 8 + 36).to_le_bytes());
    glb.extend((json.len() as u32).to_le_bytes());
    glb.extend(b"JSON");
    glb.extend(json);
    glb.extend(36u32.to_le_bytes());
    glb.extend(b"BIN\0");
    for coordinate in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        glb.extend(coordinate.to_le_bytes());
    }
    glb
}
