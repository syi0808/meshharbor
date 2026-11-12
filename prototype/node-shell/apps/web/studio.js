const tokenKey = "meshharbor.local-token";
const state = { token: sessionStorage.getItem(tokenKey), assets: [], jobs: [], selectedAssetId: null, model: null, rotationX: -0.3, rotationY: 0.45, zoom: 1, dragging: false, wireframe: false, lastPointer: null, toastTimer: null };
const $ = id => document.getElementById(id);
const authScreen = $("auth-screen");
const appScreen = $("app");
const canvas = $("mesh-canvas");
const ctx = canvas.getContext("2d");

function toast(message, error = false) {
  const element = $("toast");
  element.textContent = message;
  element.classList.toggle("error", error);
  element.classList.remove("hidden");
  clearTimeout(state.toastTimer);
  state.toastTimer = setTimeout(() => element.classList.add("hidden"), 3600);
}

function setStatus(message) {
  $("status-message").textContent = message;
}

async function request(path, options = {}) {
  const headers = new Headers(options.headers || {});
  headers.set("Authorization", `Bearer ${state.token || ""}`);
  if (options.body && !headers.has("Content-Type") && typeof options.body === "string") headers.set("Content-Type", "application/json");
  const response = await fetch(path, { ...options, headers, cache: "no-store" });
  const contentType = response.headers.get("content-type") || "";
  const body = contentType.includes("application/json") ? await response.json() : await response.arrayBuffer();
  if (!response.ok) throw new Error(body?.error?.message || `Request failed (${response.status}).`);
  return body;
}

function authenticate(token) {
  state.token = token;
  return request("/v1/operations");
}

function displayApp() {
  authScreen.classList.add("hidden");
  appScreen.classList.remove("hidden");
  refreshData().catch(error => toast(error.message, true));
}

async function refreshData() {
  if (!state.token) return;
  const [assetsResponse, jobsResponse] = await Promise.all([request("/v1/assets"), request("/v1/jobs")]);
  state.assets = assetsResponse.assets || [];
  state.jobs = jobsResponse.jobs || [];
  if (state.selectedAssetId && !state.assets.some(asset => asset.id === state.selectedAssetId)) state.selectedAssetId = null;
  renderAssets();
  renderJobs();
  updateSelectedAsset();
  $("asset-count").textContent = String(state.assets.length);
  $("job-count").textContent = String(state.jobs.length);
  $("jobs-count-side").textContent = String(state.jobs.length);
}

function prettyBytes(bytes = 0) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function renderAssets() {
  const list = $("asset-list");
  if (!state.assets.length) {
    list.innerHTML = '<div class="empty-list">Your imported and generated assets will appear here.</div>';
    return;
  }
  list.innerHTML = "";
  for (const asset of state.assets) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = `asset-card${state.selectedAssetId === asset.id ? " selected" : ""}`;
    const icon = document.createElement("span");
    icon.className = `asset-icon ${asset.kind === "image" ? "image" : ""}`;
    icon.textContent = asset.kind === "image" ? "▧" : "◇";
    const copy = document.createElement("span");
    copy.className = "asset-copy";
    const name = document.createElement("strong");
    name.textContent = asset.name;
    const detail = document.createElement("small");
    const metadata = asset.metadata || {};
    const geometry = metadata.triangles ? `${metadata.triangles.toLocaleString()} triangles` : asset.kind.toUpperCase();
    detail.textContent = `${geometry} · ${new Date(asset.created_at).toLocaleString()}`;
    copy.append(name, detail);
    const type = document.createElement("span");
    type.className = "asset-type";
    type.textContent = asset.source_asset_id ? "DERIVED" : "SOURCE";
    card.append(icon, copy, type);
    card.addEventListener("click", () => selectAsset(asset.id));
    list.append(card);
  }
}

function renderJobs() {
  const list = $("job-list");
  if (!state.jobs.length) {
    list.innerHTML = '<div class="empty-list">No jobs yet.</div>';
    return;
  }
  list.innerHTML = "";
  for (const job of state.jobs) {
    const card = document.createElement("div");
    card.className = "job-card";
    const dot = document.createElement("span");
    dot.className = `job-status ${job.state}`;
    const copy = document.createElement("div");
    copy.className = "job-copy";
    const title = document.createElement("strong");
    title.textContent = job.operation.replaceAll(".", " · ");
    const subtitle = document.createElement("small");
    subtitle.textContent = job.error?.message || (job.state === "succeeded" ? `${job.result?.actual_triangles?.toLocaleString() ?? ""} triangles · ${job.id.slice(0, 13)}` : `${job.state} · ${job.id.slice(0, 13)}`);
    copy.append(title, subtitle);
    card.append(dot, copy);
    list.append(card);
  }
}

async function selectAsset(assetId) {
  state.selectedAssetId = assetId;
  renderAssets();
  await updateSelectedAsset();
}

async function updateSelectedAsset() {
  const asset = state.assets.find(item => item.id === state.selectedAssetId) || null;
  $("simplify-button").disabled = !asset || asset.kind !== "mesh";
  $("export-button").disabled = !asset || !asset.artifacts?.length;
  $("viewer-title").textContent = asset?.name || "No asset selected";
  const stats = $("viewer-stats");
  if (!asset) {
    $("viewer-empty").classList.remove("hidden");
    stats.textContent = "—";
    $("inspector-empty").classList.remove("hidden");
    $("inspector-body").classList.add("hidden");
    state.model = null;
    drawViewer();
    return;
  }
  $("viewer-empty").classList.add("hidden");
  const metadata = asset.metadata || {};
  stats.textContent = asset.kind === "mesh" ? `${(metadata.triangles || 0).toLocaleString()} triangles · ${(metadata.vertices || 0).toLocaleString()} vertices` : `IMAGE · ${prettyBytes(metadata.bytes || 0)}`;
  renderInspector(asset);
  const previewArtifact = asset.artifacts?.find(item => item.name.endsWith(".glb") && ["source", "result"].includes(item.kind));
  if (!previewArtifact) {
    state.model = null;
    drawViewer();
    return;
  }
  try {
    const buffer = await request(`/v1/artifacts/${previewArtifact.id}/content`);
    state.model = parseGlbForPreview(buffer);
    drawViewer();
    setStatus(`Selected ${asset.name}`);
  } catch (error) {
    state.model = null;
    drawViewer();
    toast(`Preview unavailable: ${error.message}`, true);
  }
}

function renderInspector(asset) {
  $("inspector-empty").classList.add("hidden");
  const body = $("inspector-body");
  body.classList.remove("hidden");
  const metadata = asset.metadata || {};
  const cells = [
    ["TRIANGLES", metadata.triangles == null ? "—" : metadata.triangles.toLocaleString()],
    ["VERTICES", metadata.vertices == null ? "—" : metadata.vertices.toLocaleString()],
    ["MATERIALS", metadata.materials == null ? "—" : String(metadata.materials)],
    ["REVISION", asset.revision?.slice(0, 13) || "—"]
  ];
  body.innerHTML = "";
  const name = document.createElement("div");
  name.className = "inspector-name";
  name.textContent = asset.name;
  const grid = document.createElement("div");
  grid.className = "metadata-grid";
  for (const [label, value] of cells) {
    const cell = document.createElement("div");
    cell.className = "metadata-cell";
    const caption = document.createElement("small");
    caption.textContent = label;
    const content = document.createElement("strong");
    content.textContent = value;
    cell.append(caption, content);
    grid.append(cell);
  }
  body.append(name, grid);
  for (const artifact of asset.artifacts || []) {
    const row = document.createElement("div");
    row.className = "artifact-row";
    const label = document.createElement("span");
    label.textContent = `${artifact.kind} · ${prettyBytes(artifact.size_bytes)}`;
    const link = document.createElement("button");
    link.className = "artifact-link";
    link.textContent = artifact.name;
    link.addEventListener("click", () => downloadArtifact(artifact));
    row.append(label, link);
    body.append(row);
  }
  if (asset.source_asset_id) {
    const row = document.createElement("div");
    row.className = "artifact-row";
    const label = document.createElement("span");
    label.textContent = "Derived from";
    const link = document.createElement("button");
    link.className = "artifact-link";
    link.textContent = asset.source_asset_id.slice(0, 18);
    link.addEventListener("click", () => selectAsset(asset.source_asset_id));
    row.append(label, link);
    body.append(row);
  }
}

function parseGlbForPreview(buffer) {
  const view = new DataView(buffer);
  if (view.byteLength < 20 || view.getUint32(0, true) !== 0x46546c67 || view.getUint32(4, true) !== 2) throw new Error("Not a glTF 2 binary file.");
  let offset = 12;
  let document = null;
  let binaryOffset = 0;
  let binaryLength = 0;
  while (offset + 8 <= view.byteLength) {
    const length = view.getUint32(offset, true);
    const type = view.getUint32(offset + 4, true);
    offset += 8;
    if (offset + length > view.byteLength) throw new Error("Invalid GLB chunk length.");
    if (type === 0x4e4f534a) document = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, offset, length)).replace(/[\u0000\u0020]+$/g, ""));
    if (type === 0x004e4942) { binaryOffset = offset; binaryLength = length; }
    offset += length;
  }
  const primitive = document?.meshes?.[0]?.primitives?.[0];
  if (!primitive) throw new Error("This asset has no previewable mesh primitive.");
  const readAccessor = (accessorId, components, type) => {
    const accessor = document.accessors?.[accessorId];
    const bufferView = document.bufferViews?.[accessor?.bufferView];
    if (!accessor || !bufferView || accessor.componentType !== type || accessor.sparse) throw new Error("The preview cannot decode this accessor.");
    const stride = bufferView.byteStride || components * 4;
    const start = binaryOffset + (bufferView.byteOffset || 0) + (accessor.byteOffset || 0);
    if (start < binaryOffset || start + Math.max(0, accessor.count - 1) * stride + components * 4 > binaryOffset + binaryLength) throw new Error("The preview accessor is outside the GLB data chunk.");
    const output = new Array(accessor.count);
    for (let i = 0; i < accessor.count; i++) {
      const row = [];
      for (let c = 0; c < components; c++) row.push(view.getFloat32(start + i * stride + c * 4, true));
      output[i] = row;
    }
    return output;
  };
  const positions = readAccessor(primitive.attributes.POSITION, 3, 5126);
  let indices;
  if (Number.isInteger(primitive.indices)) {
    const accessor = document.accessors[primitive.indices];
    const bufferView = document.bufferViews[accessor.bufferView];
    const bytes = accessor.componentType === 5121 ? 1 : accessor.componentType === 5123 ? 2 : accessor.componentType === 5125 ? 4 : 0;
    if (!bytes || accessor.type !== "SCALAR" || accessor.sparse) throw new Error("The preview cannot decode mesh indices.");
    const start = binaryOffset + (bufferView.byteOffset || 0) + (accessor.byteOffset || 0);
    if (start < binaryOffset || start + accessor.count * bytes > binaryOffset + binaryLength) throw new Error("The preview index accessor is outside the GLB data chunk.");
    indices = new Array(accessor.count);
    for (let i = 0; i < accessor.count; i++) indices[i] = bytes === 1 ? view.getUint8(start + i) : bytes === 2 ? view.getUint16(start + i * bytes, true) : view.getUint32(start + i * bytes, true);
  } else {
    indices = positions.map((_, index) => index);
  }
  const materialId = primitive.material;
  const factor = document.materials?.[materialId]?.pbrMetallicRoughness?.baseColorFactor || [0.7, 0.56, 0.45, 1];
  const xs = positions.map(point => point[0]);
  const ys = positions.map(point => point[1]);
  const zs = positions.map(point => point[2]);
  const min = [Math.min(...xs), Math.min(...ys), Math.min(...zs)];
  const max = [Math.max(...xs), Math.max(...ys), Math.max(...zs)];
  const center = min.map((value, index) => (value + max[index]) / 2);
  const size = Math.max(...max.map((value, index) => value - min[index]), 1e-8);
  const normalized = positions.map(point => point.map((value, index) => (value - center[index]) / size));
  return { positions: normalized, indices, color: factor.slice(0, 3).map(value => Math.max(0, Math.min(1, Number(value) || 0))) };
}

function drawViewer() {
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const width = Math.max(1, Math.round(rect.width * dpr));
  const height = Math.max(1, Math.round(rect.height * dpr));
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, rect.width, rect.height);
  if (!state.model) return;
  const { positions, indices, color } = state.model;
  const cosX = Math.cos(state.rotationX), sinX = Math.sin(state.rotationX);
  const cosY = Math.cos(state.rotationY), sinY = Math.sin(state.rotationY);
  const scale = Math.min(rect.width, rect.height) * 0.74 * state.zoom;
  const centerX = rect.width / 2, centerY = rect.height / 2;
  const transformed = positions.map(([x, y, z]) => {
    const rx = x * cosY + z * sinY;
    const rz = -x * sinY + z * cosY;
    const ry = y * cosX - rz * sinX;
    const depth = y * sinX + rz * cosX;
    return [centerX + rx * scale, centerY - ry * scale, depth];
  });
  const faces = [];
  for (let i = 0; i + 2 < indices.length; i += 3) {
    const a = transformed[indices[i]], b = transformed[indices[i + 1]], c = transformed[indices[i + 2]];
    if (!a || !b || !c) continue;
    const ux = b[0] - a[0], uy = b[1] - a[1], uz = b[2] - a[2];
    const vx = c[0] - a[0], vy = c[1] - a[1], vz = c[2] - a[2];
    const nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx;
    const norm = Math.hypot(nx, ny, nz) || 1;
    const light = Math.max(0, (nx * 0.28 + ny * -0.35 + nz * -0.9) / norm);
    faces.push({ points: [a, b, c], depth: (a[2] + b[2] + c[2]) / 3, shade: 0.34 + light * 0.66 });
  }
  faces.sort((a, b) => a.depth - b.depth);
  for (const face of faces) {
    const rgb = color.map(channel => Math.round(channel * face.shade * 255));
    ctx.beginPath();
    ctx.moveTo(face.points[0][0], face.points[0][1]);
    ctx.lineTo(face.points[1][0], face.points[1][1]);
    ctx.lineTo(face.points[2][0], face.points[2][1]);
    ctx.closePath();
    ctx.fillStyle = `rgb(${rgb.join(",")})`;
    ctx.fill();
    if (state.wireframe) { ctx.strokeStyle = "rgba(224,232,241,.32)"; ctx.lineWidth = 0.55; ctx.stroke(); }
  }
  $("view-mode").textContent = state.wireframe ? "WIREFRAME" : "SOLID";
}

async function importFile(file) {
  if (!file) return;
  setStatus(`Importing ${file.name}…`);
  try {
    const result = await request(`/v1/assets/import?name=${encodeURIComponent(file.name)}`, { method: "POST", headers: { "Content-Type": "application/octet-stream" }, body: file });
    await refreshData();
    await selectAsset(result.asset.id);
    toast(`Imported ${file.name}`);
    setStatus(`Imported ${file.name}`);
  } catch (error) {
    toast(error.message, true);
    setStatus("Import failed.");
  }
}

async function createSimplification(event) {
  event.preventDefault();
  const asset = state.assets.find(item => item.id === state.selectedAssetId);
  if (!asset || asset.kind !== "mesh") return toast("Select a GLB mesh first.", true);
  const target = Number($("triangle-target").value);
  if (!Number.isInteger(target) || target < 4 || target > 10_000_000) return toast("Enter a valid target between 4 and 10,000,000.", true);
  $("simplify-button").disabled = true;
  setStatus("Starting simplification job…");
  try {
    const { job } = await request("/v1/jobs", { method: "POST", body: JSON.stringify({ operation: "mesh.simplify", input: { asset_id: asset.id }, parameters: { target_triangles: target } }) });
    await refreshData();
    toast(`Job queued · ${job.id.slice(0, 15)}`);
    setStatus("Simplification queued. The source asset remains unchanged.");
  } catch (error) {
    toast(error.message, true);
    setStatus("The job could not be started.");
  } finally {
    $("simplify-button").disabled = !state.selectedAssetId;
  }
}

async function downloadArtifact(artifact) {
  try {
    const buffer = await request(`/v1/artifacts/${artifact.id}/content`);
    const url = URL.createObjectURL(new Blob([buffer], { type: artifact.mime_type }));
    const link = document.createElement("a");
    link.href = url;
    link.download = artifact.name;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 15_000);
  } catch (error) { toast(error.message, true); }
}

async function showRuntimeDoctor() {
  try {
    const report = await request("/v1/runtimes/doctor");
    const host = $("runtime-report");
    host.innerHTML = "";
    const rows = [
      ["Native mesh worker", report.native_worker],
      ["Simplification", report.mesh_simplification],
      ["Image generation", report.image_generation],
      ...report.runtimes.map(runtime => [runtime.id, `${runtime.status} · ${runtime.license_status}`])
    ];
    for (const [title, status] of rows) {
      const row = document.createElement("div");
      row.className = "runtime-row";
      const strong = document.createElement("strong");
      strong.textContent = title;
      const small = document.createElement("small");
      small.textContent = status;
      row.append(strong, small);
      host.append(row);
    }
    for (const warning of report.warnings || []) {
      const item = document.createElement("p");
      item.className = "runtime-warning";
      item.textContent = warning;
      host.append(item);
    }
    $("modal-backdrop").classList.remove("hidden");
  } catch (error) { toast(error.message, true); }
}

$("auth-form").addEventListener("submit", async event => {
  event.preventDefault();
  const errorNode = $("auth-error");
  errorNode.textContent = "";
  try {
    await authenticate($("token-input").value.trim());
    sessionStorage.setItem(tokenKey, state.token);
    displayApp();
  } catch (error) {
    state.token = null;
    errorNode.textContent = error.message;
  }
});

if (state.token) authenticate(state.token).then(displayApp).catch(() => {
  sessionStorage.removeItem(tokenKey);
  state.token = null;
});

$("import-button").addEventListener("click", () => $("import-file").click());
$("right-import-button").addEventListener("click", () => $("import-file").click());
$("empty-import-button").addEventListener("click", () => $("import-file").click());
$("import-file").addEventListener("change", event => importFile(event.target.files?.[0]));
$("image-file").addEventListener("change", () => toast("Image generation is pending license and runtime review."));
$("simplify-form").addEventListener("submit", createSimplification);
$("refresh-button").addEventListener("click", () => refreshData().then(() => toast("Studio refreshed")).catch(error => toast(error.message, true)));
$("doctor-button").addEventListener("click", showRuntimeDoctor);
$("modal-close").addEventListener("click", () => $("modal-backdrop").classList.add("hidden"));
$("modal-backdrop").addEventListener("click", event => { if (event.target === $("modal-backdrop")) $("modal-backdrop").classList.add("hidden"); });
$("token-reset").addEventListener("click", () => { sessionStorage.removeItem(tokenKey); location.reload(); });
$("export-button").addEventListener("click", () => {
  const asset = state.assets.find(item => item.id === state.selectedAssetId);
  const artifact = asset?.artifacts?.find(item => item.kind === "result") || asset?.artifacts?.find(item => item.kind === "source");
  if (artifact) downloadArtifact(artifact);
});
$("fit-view").addEventListener("click", () => { state.rotationX = -0.3; state.rotationY = 0.45; state.zoom = 1; drawViewer(); });
$("wireframe-toggle").addEventListener("click", () => { state.wireframe = !state.wireframe; drawViewer(); });
$("clear-selection").addEventListener("click", () => { state.selectedAssetId = null; renderAssets(); updateSelectedAsset(); });

document.querySelectorAll(".tab").forEach(tab => tab.addEventListener("click", () => {
  document.querySelectorAll(".tab").forEach(item => item.classList.toggle("active", item === tab));
  $("assets-view").classList.toggle("hidden", tab.dataset.tab !== "assets");
  $("jobs-view").classList.toggle("hidden", tab.dataset.tab !== "jobs");
}));

canvas.addEventListener("pointerdown", event => { state.dragging = true; state.lastPointer = [event.clientX, event.clientY]; canvas.setPointerCapture(event.pointerId); });
canvas.addEventListener("pointermove", event => {
  if (!state.dragging || !state.lastPointer) return;
  const dx = event.clientX - state.lastPointer[0], dy = event.clientY - state.lastPointer[1];
  state.rotationY += dx * 0.008;
  state.rotationX = Math.max(-1.5, Math.min(1.5, state.rotationX + dy * 0.008));
  state.lastPointer = [event.clientX, event.clientY];
  drawViewer();
});
canvas.addEventListener("pointerup", () => { state.dragging = false; state.lastPointer = null; });
canvas.addEventListener("pointercancel", () => { state.dragging = false; state.lastPointer = null; });
canvas.addEventListener("wheel", event => { event.preventDefault(); state.zoom = Math.max(0.3, Math.min(4, state.zoom * (event.deltaY < 0 ? 1.1 : 0.9))); drawViewer(); }, { passive: false });

const viewer = $("viewer-frame");
let dropOverlay;
viewer.addEventListener("dragover", event => { event.preventDefault(); if (!dropOverlay) { dropOverlay = document.createElement("div"); dropOverlay.className = "drop-overlay"; dropOverlay.textContent = "Drop an asset to import"; viewer.append(dropOverlay); } });
viewer.addEventListener("dragleave", event => { if (!viewer.contains(event.relatedTarget)) { dropOverlay?.remove(); dropOverlay = null; } });
viewer.addEventListener("drop", event => { event.preventDefault(); dropOverlay?.remove(); dropOverlay = null; importFile(event.dataTransfer.files?.[0]); });

window.addEventListener("resize", drawViewer);
setInterval(() => { if (state.token && !appScreen.classList.contains("hidden")) refreshData().catch(() => {}); }, 3000);
