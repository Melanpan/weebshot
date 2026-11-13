#!/usr/bin/env python3
"""End-to-end MCP tests. --gui also needs Xvfb, Openbox, xdotool and xprop."""
import argparse
import base64
import struct
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import sys
sys.dont_write_bytecode = True


def summary(value):
    if isinstance(value,dict):
        return {key:(f"<{len(item)} bytes of image data>" if key=="data" else summary(item)) for key,item in value.items()}
    if isinstance(value,list):
        return [summary(item) for item in value]
    return value


class Client:
    def __init__(self, binary, args, env, log):
        self.process = subprocess.Popen([binary, *args], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=log, env=env, text=True)
        self.messages = queue.Queue()
        def read():
            for line in self.process.stdout:
                try:
                    self.messages.put(json.loads(line))
                except ValueError:
                    self.messages.put({"bad_stdout": line})
            self.messages.put({"disconnected": True})
        threading.Thread(target=read, daemon=True).start()
        self.responses = {}
        self.next_id = 0
        init = self.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                     "clientInfo": {"name": "weebshot-test", "version": "1"}})
        assert init["result"]["capabilities"]["tools"] == {}, init
        assert init["result"]["serverInfo"]["name"] == "weebshot", init
        self.notify("notifications/initialized")

    def notify(self, method):
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.process.stdin.flush()

    def rpc(self, method, params):
        self.next_id += 1
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.next_id,
                                            "method": method, "params": params}) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            message = self.messages.get(timeout=max(.01, deadline-time.monotonic()))
            assert "bad_stdout" not in message and "disconnected" not in message, message
            if message.get("id") == self.next_id:
                return message
            if "id" in message:
                self.responses[message["id"]] = message
        raise AssertionError("MCP response timed out")

    def tool(self, name, arguments=None, error=False):
        response = self.rpc("tools/call", {"name": name, "arguments": arguments or {}})
        if error:
            assert "error" in response or response["result"].get("isError"), summary(response)
            return response
        assert "error" not in response, summary(response)
        result = response["result"]
        assert not result.get("isError"), result
        return result.get("structuredContent") or json.loads(result["content"][0]["text"])

    def wait_job(self, job, status="done"):
        deadline = time.monotonic()+30
        while time.monotonic()<deadline:
            result = self.tool("jobs_get", {"job_id": job["job_id"]})
            if result["status"] in ("done", "failed", "cancelled"):
                assert result["status"] == status, result
                return result
            time.sleep(.04)
        raise AssertionError("Job did not finish")

    def close(self):
        self.process.stdin.close()
        assert self.process.wait(timeout=10) == 0

    def terminate(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()


def fixture(directory):
    video = directory / "pan.mkv"
    # Deterministic artwork and the same 8fps vertical-pan shape used by Stitch tests.
    seed = 0x12345678
    artwork = bytearray()
    for y in range(320):
        for x in range(384):
            seed ^= (seed << 13) & 0xffffffff
            seed ^= seed >> 17
            seed ^= (seed << 5) & 0xffffffff
            value = min(255, (x*7+y*3+(x//17)*61+(y//13)*47) % 180 +
                        (45 if (x-130)**2+(y-110)**2 < 2500 else 0) + seed % 24)
            artwork.extend((value, min(255, value+12), max(0, value-10)))
    frames = bytearray()
    for i in range(24):
        for y in range(96):
            offset = ((10+i*3+y)*384+20)*3
            frames.extend(artwork[offset:offset+128*3])
    (directory/"frame-8.rgb").write_bytes(frames[8*128*96*3:9*128*96*3])
    subtitle=directory/"fixture.srt"
    subtitle.write_text("1\n00:00:00,000 --> 00:00:03,000\nTest subtitle\n")
    subprocess.run(["ffmpeg", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24",
                    "-s", "128x96", "-r", "8", "-i", "pipe:0", "-f", "lavfi",
                    "-i", "sine=frequency=440:duration=3", "-i", str(subtitle), "-c:s", "srt", "-c:v", "ffv1", "-c:a", "pcm_s16le",
                    "-shortest", str(video)], input=frames, check=True)
    return video


def headless(binary, directory, video, env, log):
    client = Client(binary, ["--headless", "--mcp"], env, log)
    try:
        tools = client.rpc("tools/list", {})["result"]["tools"]
        assert len(tools) == 29, len(tools)
        assert all(t["inputSchema"]["additionalProperties"] is False for t in tools)
        client.tool("gui_get_state", error=True)
        client.tool("media_info", error=True)
        client.tool("media_open", {"path": str(directory / "missing.mkv")}, error=True)
        invalid=directory/"invalid.mkv"
        invalid.write_text("not a media file")
        client.tool("media_open", {"path":str(invalid)}, error=True)
        client.tool("media_open", {"path": str(video)})
        info = client.tool("media_info")
        assert info["streams"][0]["width"] == 128, info
        tracks = client.tool("media_tracks")["tracks"]
        audio = next(t["index"] for t in tracks if t["codec_type"] == "audio")
        client.tool("media_select_track", {"kind": "audio", "track_id": audio})
        client.tool("media_select_track", {"kind": "audio", "track_id": 999}, error=True)
        assert client.tool("timeline_seek", {"frame": 8})["position_seconds"] == 1
        client.tool("timeline_seek", {"seconds": 99}, error=True)
        client.tool("timeline_seek", {"seconds": 1, "frame": 8}, error=True)
        client.tool("timeline_seek", {"seconds": "1"}, error=True)
        client.tool("timeline_set_in", {"seconds": .5})
        client.tool("timeline_set_out", {"seconds": 2.5, "replace_selection": True})
        client.tool("timeline_set_range", {"in_seconds": 2, "out_seconds": 1,
                                           "replace_selection": True}, error=True)
        client.tool("media_close", error=True)
        client.tool("playback_control", {"action": "play"}, error=True)
        assert client.tool("playback_control", {"action": "step", "frames": 1})["position_seconds"] == 1.125
        shot = directory/"frame.png"
        job = client.tool("screenshot_extract", {"output": str(shot), "frame": 8, "subtitles": False})
        # Completion must publish output while the client is idle (no jobs polling).
        deadline=time.monotonic()+10
        while not shot.exists() and time.monotonic()<deadline:
            time.sleep(.02)
        client.wait_job(job)
        raw=subprocess.check_output(["ffmpeg","-v","error","-i",str(shot),"-f","rawvideo","-pix_fmt","rgb24","pipe:1"])
        assert raw==(directory/"frame-8.rgb").read_bytes(), "frame index did not select the exact frame"
        assert shot.read_bytes().startswith(b"\x89PNG")
        result = client.rpc("tools/call", {"name": "jobs_get", "arguments": {"job_id": job["job_id"], "preview": True}})
        assert any(c["type"] == "image" for c in result["result"]["content"]), result
        image=next(c for c in result["result"]["content"] if c["type"]=="image")
        width,height=struct.unpack(">II",base64.b64decode(image["data"])[16:24])
        assert max(width,height)<=512
        client.tool("screenshot_extract", {"output": str(shot)}, error=True)
        for suffix in ("jpg", "webp"):
            client.wait_job(client.tool("screenshot_extract", {"output": str(directory/f"frame.{suffix}")}))
        subtitle=next(t["index"] for t in tracks if t["codec_type"]=="subtitle")
        client.tool("media_select_track",{"kind":"subtitle","track_id":subtitle})
        client.tool("export_set_settings",{"settings":{"shot_subs":"Both"}})
        pair=client.tool("screenshot_extract",{"output":str(directory/"both.png")})
        assert len(pair["job_ids"])==2,pair
        for job_id in pair["job_ids"]:
            client.wait_job({"job_id":job_id})
        assert (directory/"both_raw.png").exists()
        # Refusing the raw sibling must not queue a partial pair.
        (directory/"collision_raw.png").write_bytes(b"keep")
        client.tool("screenshot_extract",{"output":str(directory/"collision.png")},error=True)
        assert not (directory/"collision.png").exists()
        client.tool("screenshot_extract", {"output": str(directory/"crop.png"), "crop":
                    {"x": 2**64-1, "y": 0, "w": 2, "h": 2}}, error=True)
        cropped=directory/"valid-crop.png"
        client.wait_job(client.tool("screenshot_extract",{"output":str(cropped),"subtitles":False,"crop":{"x":2,"y":2,"w":64,"h":48}}))
        assert struct.unpack(">II",cropped.read_bytes()[16:24])==(64,48)
        client.tool("export_set_settings", {"settings": {"quality": "Small", "clip_format": "Mp4", "clip_subs": "Off"}})
        output = directory/"clip.mp4"
        client.wait_job(client.tool("export_add", {"output": str(output)}))
        assert output.stat().st_size > 0
        pair=client.tool("export_add",{"output":str(directory/"both.mp4"),"settings":{"clip_subs":"Both"}})
        assert len(pair["job_ids"])==2,pair
        for job_id in pair["job_ids"]:
            client.wait_job({"job_id":job_id})
        assert (directory/"both_raw.mp4").stat().st_size>0
        client.wait_job(client.tool("export_add", {"output": str(directory/"clip.gif"),
                        "settings": {"clip_format": "Gif", "anim_height": 96, "anim_fps": 8,
                                     "anim": {"gifsicle_optimize": False}}}))
        client.wait_job(client.tool("export_add", {"output": str(directory/"clip.mkv"),
                        "settings": {"clip_format": "Mkv", "clip_stream_copy": True}}))
        client.wait_job(client.tool("stitch_create", {"output": str(directory/"stitch.png"),
                        "in_seconds": 0, "out_seconds": 3, "method": "translation"}))
        psd=directory/"stitch.psd"
        client.wait_job(client.tool("stitch_create",{"output":str(psd),"in_seconds":0,"out_seconds":3,"method":"translation"}))
        assert psd.read_bytes().startswith(b"8BPS")
        # A long export can be queried and cancelled while the worker is active.
        job = client.tool("export_add", {"output": str(directory/"cancel.mp4"),
                          "settings": {"loop_count": 100, "encoder": {"x264_preset": "veryslow"}}})
        race=directory/"race.png"
        raced=client.tool("screenshot_extract",{"output":str(race),"subtitles":False})
        race.write_bytes(b"do not overwrite")
        assert client.tool("media_info")["streams"]
        client.tool("jobs_cancel", {"job_id": job["job_id"]})
        client.wait_job(job, "cancelled")
        assert not (directory/"cancel.mp4").exists()
        client.wait_job(raced,"failed")
        assert race.read_bytes()==b"do not overwrite"
        # Fail a real export by removing its input after it has been opened.
        gone = directory/"gone.mkv"
        shutil.copyfile(video, gone)
        client.tool("media_open", {"path": str(gone), "replace_selection": True})
        gone.unlink()
        client.wait_job(client.tool("screenshot_extract", {"output": str(directory/"failed.png")}), "failed")
        client.tool("media_close",{"replace_selection":True})
        media_dir=directory/"media-checks"
        media_dir.mkdir()
        from test_mcp_media import exercise
        exercise(client,media_dir)
        from test_mcp_reports import exercise as report
        report(client,log,rate_limit=True)
        client.close()
        # EOF and SIGTERM must cancel workers and reap subprocesses.
        for terminate in (False, True):
            c = Client(binary, ["--headless", "--mcp"], env, log)
            try:
                c.tool("media_open", {"path": str(video)})
                c.tool("export_add", {"output": str(directory/f"shutdown-{terminate}.mp4"),
                       "settings": {"clip_format": "Mp4", "loop_count": 100,
                                    "encoder": {"x264_preset": "veryslow"}}})
                children=[]
                children_file=Path(f"/proc/{c.process.pid}/task/{c.process.pid}/children")
                deadline=time.monotonic()+1
                while time.monotonic()<deadline:
                    if children_file.exists():
                        children=[int(p) for p in children_file.read_text().split()]
                    if children:
                        break
                    time.sleep(.01)
                if terminate:
                    c.process.send_signal(signal.SIGTERM)
                    assert c.process.wait(timeout=10) == 0
                else:
                    c.close()
                assert not any(Path(f"/proc/{pid}").exists() for pid in children), "orphaned media subprocess"
            finally:
                c.terminate()
        assert not list(directory.glob(".weebshot-*")), list(directory.iterdir())
    finally:
        client.terminate()


def gui(binary, directory, video, env, log):
    read_fd, write_fd = os.pipe()
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24"],
                            pass_fds=(write_fd,), stdout=log, stderr=log)
    os.close(write_fd)
    with os.fdopen(read_fd) as pipe:
        display = pipe.readline().strip()
    gui_env = dict(env, DISPLAY=":"+display, WINIT_UNIX_BACKEND="x11",
                   LIBGL_ALWAYS_SOFTWARE="1", XDG_DATA_HOME=str(directory/"data"))
    # A real window manager is necessary to exercise eframe's hidden-window
    # lifecycle. A bare Xvfb display never sends the minimized state.
    wm = subprocess.Popen(["openbox", "--sm-disable"], env=gui_env, stdout=log, stderr=log)
    settings = directory/"data"/"weebshot"/"app.ron"
    settings.parent.mkdir(parents=True)
    settings.write_text('{"app": "(check_updates:false,sounds:false,hwdec:false)"}')
    disabled=subprocess.Popen([binary],env=gui_env,stdout=log,stderr=log)
    try:
        time.sleep(.6)
        assert disabled.poll() is None,"Normal GUI failed to launch"
        assert not (directory/f"weebshot-mcp-{os.getuid()}"/"gui.sock").exists(),"MCP enabled by default"
    finally:
        disabled.terminate()
        disabled.wait(timeout=10)
    settings.write_text('{"app": "(mcp_enabled:true,check_updates:false,sounds:false,hwdec:false)"}')
    # A killed process leaves the filesystem socket behind. Enabling MCP at
    # startup must recover it without requiring manual deletion or a UI toggle.
    import socket as unix_socket
    socket_path = directory/f"weebshot-mcp-{os.getuid()}"/"gui.sock"
    socket_path.parent.mkdir(mode=0o700, exist_ok=True)
    with unix_socket.socket(unix_socket.AF_UNIX, unix_socket.SOCK_STREAM) as stale:
        stale.bind(str(socket_path))
    app = subprocess.Popen([binary], env=gui_env, stdout=log, stderr=log)
    client = None
    try:
        socket = directory/f"weebshot-mcp-{os.getuid()}"/"gui.sock"
        deadline = time.monotonic()+20
        # Socket existence alone is not readiness: the stale file already exists.
        while app.poll() is None and time.monotonic()<deadline:
            try:
                with unix_socket.socket(unix_socket.AF_UNIX, unix_socket.SOCK_STREAM) as probe:
                    probe.settimeout(.2)
                    probe.connect(str(socket))
                break
            except OSError:
                time.sleep(.1)
        else:
            raise AssertionError("GUI did not recover the stale socket; inspect test log")
        client = Client(binary, ["--mcp", "--mcp-socket", str(socket)], env, log)
        assert client.tool("gui_get_state")["gui"]
        prepared = client.tool("gui_prepare_export", {"media": str(video), "in_seconds": .5,
                 "out_seconds": 2, "seek_to_in": True, "settings": {"clip_format": "Gif", "anim_fps": 8}})
        deadline = time.monotonic()+10
        while time.monotonic()<deadline:
            state = client.tool("gui_get_state")
            if not state["preparing"] and state["duration_seconds"]>0:
                break
            time.sleep(.1)
        assert state["in_seconds"] == .5 and state["out_seconds"] == 2, state
        assert state["settings"]["clip_format"] == "Gif", state
        client.tool("gui_set_range", {"in_seconds": 1, "out_seconds": 2.5}, error=True)
        client.tool("gui_set_range", {"in_seconds": 1, "out_seconds": 2.5, "replace_selection": True})
        client.tool("gui_seek", {"seconds": 1.5})
        time.sleep(.2)
        state = client.tool("gui_get_state")
        assert state["in_seconds"] == 1 and state["out_seconds"] == 2.5, state
        assert abs(state["position_seconds"]-1.5)<.2, state
        client.tool("gui_set_export_settings", {"settings": {"quality": "High", "anim_fps": 12}})
        assert client.tool("gui_get_state")["settings"]["anim_fps"] == 12
        window = subprocess.check_output(
            ["xdotool", "search", "--onlyvisible", "--pid", str(app.pid)],
            env=gui_env, text=True).splitlines()[0]
        subprocess.run(["xdotool", "windowminimize", "--sync", window], env=gui_env, check=True)
        deadline = time.monotonic()+5
        while time.monotonic()<deadline:
            properties = subprocess.check_output(
                ["xprop", "-id", window, "_NET_WM_STATE"], env=gui_env, text=True)
            if "_NET_WM_STATE_HIDDEN" in properties:
                break
            time.sleep(.05)
        assert "_NET_WM_STATE_HIDDEN" in properties, properties
        time.sleep(.3)  # Let winit process the visibility event before sending work.
        # Cached status works even with the old bug; these calls must actually
        # reach App while ui() is suspended. Include loading and async export.
        assert client.tool("gui_get_state")["gui"]
        client.tool("media_close", {"replace_selection": True})
        client.tool("media_open", {"path": str(video)})
        deadline = time.monotonic()+10
        while client.tool("media_info")["duration_seconds"] <= 0:
            assert time.monotonic()<deadline, "Hidden GUI did not load media"
            time.sleep(.05)
        client.tool("gui_seek", {"seconds": 1.5})
        client.tool("gui_set_range", {"in_seconds": 1, "out_seconds": 2.5})
        client.tool("gui_set_export_settings", {"settings": {"anim_fps": 12}})
        time.sleep(.15)
        hidden = client.tool("gui_get_state")
        assert abs(hidden["position_seconds"]-1.5)<.2, hidden
        assert hidden["in_seconds"] == 1 and hidden["out_seconds"] == 2.5, hidden
        assert hidden["settings"]["anim_fps"] == 12, hidden
        client.wait_job(client.tool("screenshot_extract", {"output": str(directory/"hidden.png")}))
        properties = subprocess.check_output(
            ["xprop", "-id", window, "_NET_WM_STATE"], env=gui_env, text=True)
        assert "_NET_WM_STATE_HIDDEN" in properties, "MCP unexpectedly restored the window"
        subprocess.run(["xdotool", "windowmap", "--sync", window], env=gui_env, check=True)
        subprocess.run(["xdotool", "windowactivate", "--sync", window], env=gui_env, check=True)
        assert client.tool("gui_get_state")["in_seconds"] == 1
        from test_mcp_inspection import preserve
        preserve(client,gui=True)
        # Inspection during playback must not pause or seek it.
        client.tool("gui_seek",{"seconds":0})
        client.tool("playback_control",{"action":"play"})
        time.sleep(.1)
        playing=client.tool("gui_get_state")
        assert not playing["paused"],playing
        from test_mcp_inspection import call
        call(client,"video_extract_frames",{"timestamps_seconds":[.25]})
        continuing=client.tool("gui_get_state")
        assert not continuing["paused"] and continuing["position_seconds"]>=playing["position_seconds"],continuing
        assert continuing["in_seconds"]==playing["in_seconds"] and continuing["out_seconds"]==playing["out_seconds"]
        client.tool("playback_control",{"action":"pause"})
        client.tool("gui_seek",{"seconds":1.5})
        time.sleep(.15)
        client.wait_job(client.tool("screenshot_extract", {"output": str(directory/"gui.png")}))
        assert client.tool("gui_get_state")["jobs"]
        from test_mcp_media import exercise
        exercise(client,directory,gui=True)
        from test_mcp_reports import exercise as report
        report(client,log)
        app.terminate()
        app.wait(timeout=10)
        client.tool("gui_get_state", error=True)
        client.close()
    finally:
        if client:
            client.terminate()
        if app.poll() is None:
            app.terminate()
            app.wait(timeout=10)
        wm.terminate()
        wm.wait(timeout=10)
        xvfb.terminate()
        xvfb.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--gui", action="store_true")
    parser.add_argument("--inspection", action="store_true")
    parser.add_argument("--reliability", action="store_true")
    parser.add_argument("--diagnose", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="weebshot-mcp-test-") as temp:
        directory = Path(temp)
        env = dict(os.environ, TMPDIR=temp)
        env.pop("DISPLAY", None)
        env.pop("WAYLAND_DISPLAY", None)
        video = fixture(directory)
        with (directory/"test.log").open("w+") as log:
            try:
                if args.reliability:
                    from test_mcp_reliability import gui as reliability
                    reliability(os.path.abspath(args.binary), directory, video, env, log, Client, args.diagnose)
                elif args.inspection:
                    from test_mcp_inspection import headless as inspection
                    inspection(os.path.abspath(args.binary), directory, video, env, log, Client)
                else:
                    (gui if args.gui else headless)(os.path.abspath(args.binary), directory, video, env, log)
            except BaseException:
                log.seek(0)
                print(log.read()[-20000:])
                raise
    print("Visual MCP inspection passed" if args.inspection else "GUI MCP workflow passed" if args.gui else "Headless MCP workflow passed (DISPLAY and WAYLAND_DISPLAY unset)")


if __name__ == "__main__":
    main()
