import http.client
import json
from concurrent.futures import Future
from pathlib import Path
from queue import Empty, SimpleQueue
from tempfile import TemporaryDirectory
from threading import Event, Thread
from time import monotonic
from urllib.parse import urlsplit
from uuid import UUID, uuid4

import bpy


bl_info = {
    "name": "MeshHarbor Bridge",
    "author": "MeshHarbor",
    "version": (1, 0, 0),
    "blender": (4, 2, 0),
    "location": "3D View > Sidebar > MeshHarbor",
    "description": "Import models sent from MeshHarbor Studio into the current scene",
    "category": "Import-Export",
}

SERVER = "http://127.0.0.1:7432"
MAX_BYTES = 512 * 1024 * 1024
HEADERS = {
    "Content-Type": "application/json",
    "X-MeshHarbor-Blender": "local-addon-v1",
}
BRIDGE = None


class Bridge:
    def __init__(self, window_pointer):
        self.window_pointer = window_pointer
        self.client_id = str(uuid4())
        self.server = urlsplit(SERVER)
        self.stop = Event()
        self.imports = SimpleQueue()
        self.status = "Connecting to MeshHarbor…"
        self.displayed_status = ""
        self.worker = Thread(target=self.run, name="MeshHarbor transfer", daemon=True)

    def request(self, path, **fields):
        connection = http.client.HTTPConnection(self.server.hostname, self.server.port, timeout=120)
        try:
            connection.request(
                "POST", "/api/v1/blender/client/" + path,
                json.dumps({"client_id": self.client_id, **fields}), HEADERS,
            )
            response = connection.getresponse()
            if response.status not in (200, 204):
                payload = json.loads(response.read(4096))
                raise OSError(payload.get("error", f"MeshHarbor returned {response.status}"))
            return connection, response
        except Exception:
            connection.close()
            raise

    def run(self):
        idle_status = "Connected · Ready to receive"
        while not self.stop.is_set():
            connection = None
            try:
                connection, response = self.request("poll")
                if response.status == 200:
                    transfer_id = str(UUID(response.getheader("X-MeshHarbor-Transfer", "")))
                    self.status = "Receiving model…"
                    with TemporaryDirectory(prefix="meshharbor-blender-") as directory:
                        error = self.receive(response, Path(directory) / "MeshHarbor.glb")
                    for attempt in range(3):
                        try:
                            result_connection, _response = self.request(f"result/{transfer_id}", error=error)
                            result_connection.close()
                            break
                        except (OSError, ValueError, http.client.HTTPException):
                            if attempt == 2 or self.stop.wait(0.5):
                                raise
                    idle_status = "Model added to scene" if error is None else "Import failed: " + error
                self.status = idle_status
            except (OSError, ValueError, http.client.HTTPException) as error:
                self.status = f"Waiting for Studio: {error}"
                idle_status = "Connected · Ready to receive"
            finally:
                if connection is not None:
                    connection.close()
            self.stop.wait(0.8)
        try:
            connection, _response = self.request("disconnect")
            connection.close()
        except (OSError, ValueError, http.client.HTTPException):
            pass

    def receive(self, response, destination):
        try:
            expected = int(response.getheader("Content-Length", "0"))
            if not 20 <= expected <= MAX_BYTES:
                raise ValueError("Invalid model download size.")
            count = 0
            with destination.open("wb") as output:
                while count < expected and not self.stop.is_set():
                    chunk = response.read(min(256 * 1024, expected - count))
                    if not chunk:
                        break
                    output.write(chunk)
                    count += len(chunk)
            if self.stop.is_set():
                return "Bridge stopped."
            if count != expected:
                raise ValueError("The model download was incomplete.")
            self.status = "Importing into the current scene…"
            result = Future()
            deadline = monotonic() + 90
            self.imports.put((destination, result))
            while True:
                try:
                    return result.result(timeout=0.2)
                except TimeoutError:
                    if (self.stop.is_set() or monotonic() >= deadline) and result.cancel():
                        return "Blender could not start the import in time."
        except (OSError, ValueError, http.client.HTTPException) as error:
            return str(error)[:400]


def import_into_scene(filepath, window_pointer):
    windows = bpy.context.window_manager.windows
    window = next((window for window in windows if window.as_pointer() == window_pointer), None)
    if window is None:
        window = bpy.context.window or next(iter(windows), None)
    with bpy.context.temp_override(window=window):
        result = bpy.ops.import_scene.gltf(filepath=str(filepath), import_pack_images=True)
        if "FINISHED" not in result:
            raise RuntimeError("Blender did not import the GLB.")


def tick():
    bridge = BRIDGE
    if bridge is None:
        return None
    try:
        filepath, result = bridge.imports.get_nowait()
    except Empty:
        pass
    else:
        if result.set_running_or_notify_cancel():
            try:
                import_into_scene(filepath, bridge.window_pointer)
                result.set_result(None)
            except Exception as error:
                result.set_result(str(error)[:400])
    if bridge.displayed_status != bridge.status:
        bridge.displayed_status = bridge.status
        for window in bpy.context.window_manager.windows:
            for area in window.screen.areas:
                if area.type == "VIEW_3D":
                    area.tag_redraw()
    return 0.2


def stop_bridge():
    global BRIDGE
    if BRIDGE is not None:
        BRIDGE.stop.set()
        BRIDGE = None
    if bpy.app.timers.is_registered(tick):
        bpy.app.timers.unregister(tick)


class MESHHARBOR_OT_toggle_bridge(bpy.types.Operator):
    bl_idname = "meshharbor.toggle_bridge"
    bl_label = "Toggle MeshHarbor Bridge"
    bl_description = "Start or stop receiving models from the local MeshHarbor Studio"

    def execute(self, context):
        global BRIDGE
        if BRIDGE is not None:
            stop_bridge()
        else:
            BRIDGE = Bridge(context.window.as_pointer())
            BRIDGE.worker.start()
            bpy.app.timers.register(tick, first_interval=0.1, persistent=True)
        return {"FINISHED"}


class MESHHARBOR_PT_bridge(bpy.types.Panel):
    bl_label = "MeshHarbor Bridge"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "MeshHarbor"

    def draw(self, _context):
        layout = self.layout
        if BRIDGE is None:
            layout.operator("meshharbor.toggle_bridge", text="Start Bridge", icon="PLAY")
        else:
            layout.operator("meshharbor.toggle_bridge", text="Stop Bridge", icon="PAUSE")
            layout.label(text=BRIDGE.status)


CLASSES = (MESHHARBOR_OT_toggle_bridge, MESHHARBOR_PT_bridge)


def register():
    for item in CLASSES:
        bpy.utils.register_class(item)


def unregister():
    stop_bridge()
    for item in reversed(CLASSES):
        bpy.utils.unregister_class(item)
