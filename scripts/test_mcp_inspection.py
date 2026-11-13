"""Visual MCP integration checks; invoked by test-mcp.py using its existing fixture/client."""
import base64
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import struct
import subprocess
import time


def images(response):
    assert "error" not in response, response
    result=response["result"]
    assert not result.get("isError"),result
    metadata=result["structuredContent"]
    visual=[c for c in result["content"] if c["type"]=="image"]
    # Base64 belongs only in image blocks, never in structured/text metadata.
    for content in result["content"]:
        if content["type"]=="text":
            assert "base64" not in content["text"] and '"data"' not in content["text"]
    for frame in metadata["frames"]:
        if "error" not in frame:
            assert result["content"][frame["content_index"]]["type"]=="image"
            assert visual[frame["image_index"]] is result["content"][frame["content_index"]]
            assert frame["actual_timestamp_seconds"]>=frame["timestamp_seconds"]-1e-9
            assert abs(frame["actual_timestamp_seconds"]-frame["presentation_timestamp"]*
                       frame["time_base"]["numerator"]/frame["time_base"]["denominator"])<1e-9
    return metadata,visual


def call(client,name,args):
    return images(client.rpc("tools/call",{"name":name,"arguments":args}))


def preserve(client,gui=False):
    state_tool="gui_get_state" if gui else "timeline_get_state"
    before=client.tool(state_tool)
    queue=client.tool("jobs_list")
    metadata,visual=call(client,"video_contact_sheet",{"sampling":"uniform","max_frames":4,"frame_width":128,"columns":2})
    assert len(visual)==1 and visual[0]["mimeType"]=="image/jpeg"
    assert metadata["width"]==256 and metadata["height"]==248,metadata
    assert [f["timestamp_seconds"] for f in metadata["frames"]]==[.375,1.125,1.875,2.625]
    metadata,visual=call(client,"video_extract_frames",{"timestamps_seconds":[1,.1,1,2.7],"format":"png"})
    assert len(visual)==4
    assert [f["actual_timestamp_seconds"] for f in metadata["frames"]]==[1,.125,1,2.75],metadata
    assert visual[0]["data"]==visual[2]["data"]
    for image in visual:
        data=base64.b64decode(image["data"])
        assert image["mimeType"]=="image/png" and data.startswith(b"\x89PNG")
        assert struct.unpack(">II",data[16:24])==(128,96)
    scenes=client.tool("video_timeline_scenes",{"max_scenes":8})
    assert scenes["analysis"]["approximate"]
    after=client.tool(state_tool)
    for key in ("media","position_seconds","paused","in_seconds","out_seconds"):
        assert before[key]==after[key],(key,before,after)
    if gui:
        assert before["tracks"]==after["tracks"]
        assert before["settings"]==after["settings"]
        assert before["jobs"]==after["jobs"]
    assert queue==client.tool("jobs_list")


def transcode(video,output,*args):
    subprocess.run(["ffmpeg","-v","error","-i",str(video),"-map","0:v:0",*args,str(output)],check=True)


def send(client,name,args):
    client.next_id+=1
    request_id=client.next_id
    client.process.stdin.write(json.dumps({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":name,"arguments":args}})+"\n")
    client.process.stdin.flush()
    return request_id


def wait_response(client,request_id):
    if request_id in client.responses:
        return client.responses.pop(request_id)
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        response=client.messages.get(timeout=10)
        if response.get("id")==request_id:
            return response
        if "id" in response:
            client.responses[response["id"]]=response
    raise AssertionError("missing asynchronous response")


def wait_decoder(marker):
    deadline=time.monotonic()+5
    while not marker.exists() and time.monotonic()<deadline:
        time.sleep(.01)
    assert marker.exists(),"decoder did not start"
    return int(marker.read_text())


def reaped(pid):
    deadline=time.monotonic()+5
    while Path(f"/proc/{pid}").exists() and time.monotonic()<deadline:
        time.sleep(.02)
    assert not Path(f"/proc/{pid}").exists(),"orphaned inspection decoder"


def interruption(binary,directory,video,env,log,Client):
    # Slow input makes cancellation deterministic while still using the real decoder.
    wrapper=directory/"slow-tools"
    wrapper.mkdir()
    marker=directory/"decoder.pid"
    ffmpeg=wrapper/"ffmpeg"
    ffmpeg.write_text("#!/bin/sh\nprintf '%s' \"$$\" > "+shlex.quote(str(marker))+"\nexec "+shlex.quote(shutil.which("ffmpeg"))+" -readrate 0.1 \"$@\"\n")
    ffmpeg.chmod(0o700)
    slow_env=dict(env,PATH=str(wrapper)+os.pathsep+env["PATH"])
    for mode in ("cancel","media_change","file_change","eof","terminate"):
        marker.unlink(missing_ok=True)
        source=directory/f"interrupt-{mode}.mkv"
        shutil.copyfile(video,source)
        client=Client(binary,["--headless","--mcp"],slow_env,log)
        try:
            client.tool("media_open",{"path":str(source)})
            request_id=send(client,"video_extract_frames",{"timestamps_seconds":[0,2.75]})
            pid=wait_decoder(marker)
            # Unrelated requests must continue while inspection owns its decoder.
            assert client.tool("timeline_get_state")["media"]==str(source)
            if mode=="cancel":
                client.process.stdin.write(json.dumps({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":request_id,"reason":"test"}})+"\n")
                client.process.stdin.flush()
                reaped(pid)
                assert client.tool("media_info")["streams"]
                client.close()
            elif mode in ("media_change","file_change"):
                if mode=="media_change":
                    client.tool("media_close")
                else:
                    source.touch()
                response=wait_response(client,request_id)
                assert response["result"].get("isError"),response
                assert any(word in str(response).lower() for word in ("changed","media","cancelled")),response
                reaped(pid)
                client.close()
            else:
                if mode=="eof":
                    client.close()
                else:
                    client.process.send_signal(signal.SIGTERM)
                    assert client.process.wait(timeout=10)==0
                reaped(pid)
        finally:
            client.terminate()


def headless(binary,directory,video,env,log,Client):
    client=Client(binary,["--headless","--mcp"],env,log)
    try:
        client.tool("video_contact_sheet",error=True)
        client.tool("video_extract_frames",{"timestamps_seconds":[0]},error=True)
        client.tool("media_open",{"path":str(video)})
        client.tool("timeline_seek",{"seconds":1.5})
        client.tool("timeline_set_range",{"in_seconds":.5,"out_seconds":2.5})
        before_files=set(directory.iterdir())
        preserve(client)
        assert set(directory.iterdir())==before_files,"inspection created visible files"
        metadata,_=call(client,"video_contact_sheet",{"interval_seconds":.5,"max_frames":3,"columns":2,"frame_width":128})
        assert metadata["truncated"] and metadata["candidate_count"]==6,metadata
        assert [f["timestamp_seconds"] for f in metadata["frames"]]==[0,.5,1]
        metadata,_=call(client,"video_contact_sheet",{"sampling":"uniform","max_frames":4,"frame_width":128,"columns":2,"include_timestamps":False})
        assert metadata["height"]==192
        for name,args in [
            ("video_contact_sheet",{"start_seconds":2,"end_seconds":1}),
            ("video_contact_sheet",{"end_seconds":4}),
            ("video_contact_sheet",{"max_frames":65}),
            ("video_contact_sheet",{"sampling":"bad"}),
            ("video_contact_sheet",{"interval_seconds":0}),
            ("video_contact_sheet",{"columns":7}),
            ("video_contact_sheet",{"sampling":"uniform","max_frames":64,"columns":1,"frame_width":1920}),
            ("video_extract_frames",{"timestamps_seconds":[]}),
            ("video_extract_frames",{"timestamps_seconds":[0]*17}),
            ("video_extract_frames",{"timestamps_seconds":[-1]}),
            ("video_extract_frames",{"timestamps_seconds":[3]}),
            ("video_extract_frames",{"timestamps_seconds":[0],"max_width":4096}),
        ]:
            client.tool(name,args,error=True)
        vfr=directory/"vfr.mkv"
        transcode(video,vfr,"-vf","select='eq(n,0)+eq(n,2)+eq(n,5)+eq(n,9)+eq(n,17)'","-fps_mode","vfr","-c:v","ffv1")
        client.tool("media_open",{"path":str(vfr),"replace_selection":True})
        metadata,_=call(client,"video_extract_frames",{"timestamps_seconds":[.1,.3,.7,1.5]})
        assert [f["actual_timestamp_seconds"] for f in metadata["frames"]]==[.25,.625,1.125,2.125],metadata
        metadata,visual=call(client,"video_extract_frames",{"timestamps_seconds":[.1,2.24]})
        assert metadata["partial"] and len(visual)==1 and "error" in metadata["frames"][1],metadata
        sar=directory/"sar.mkv"
        transcode(video,sar,"-vf","setsar=2","-c:v","ffv1")
        client.tool("media_open",{"path":str(sar)})
        metadata,_=call(client,"video_extract_frames",{"timestamps_seconds":[1]})
        assert (metadata["frames"][0]["width"],metadata["frames"][0]["height"])==(256,96),metadata
        mp4=directory/"rotation-base.mp4"
        transcode(video,mp4,"-c:v","libx264","-pix_fmt","yuv420p")
        rotated=directory/"rotated.mp4"
        # Write a real track display matrix. New FFmpeg versions no longer turn
        # the legacy rotate tag into this metadata during remuxing.
        data=bytearray(mp4.read_bytes())
        tkhd=data.index(b"tkhd")
        offset=tkhd+(56 if data[tkhd+4]==1 else 44)
        struct.pack_into(">9i",data,offset,0,-65536,0,65536,0,0,0,0,1<<30)
        rotated.write_bytes(data)
        client.tool("media_open",{"path":str(rotated)})
        metadata,_=call(client,"video_extract_frames",{"timestamps_seconds":[1]})
        assert (metadata["frames"][0]["width"],metadata["frames"][0]["height"])==(96,128),metadata
        metadata,_=call(client,"video_contact_sheet",{"sampling":"uniform","max_frames":2,"frame_width":128,"columns":2})
        assert metadata["height"]==199,metadata
        scenes=directory/"scenes.mkv"
        subprocess.run(["ffmpeg","-v","error","-f","lavfi","-i","color=white:s=128x96:r=8:d=1","-f","lavfi","-i","color=black:s=128x96:r=8:d=1","-f","lavfi","-i","color=white:s=128x96:r=8:d=1","-filter_complex","[0:v][1:v][2:v]concat=n=3:v=1:a=0","-c:v","ffv1",str(scenes)],check=True)
        client.tool("media_open",{"path":str(scenes)})
        metadata,_=call(client,"video_contact_sheet",{"sampling":"scene","max_frames":4,"frame_width":128})
        assert not metadata["scene_fallback"] and metadata["frame_count"]==3,metadata
        assert [f["timestamp_seconds"] for f in metadata["frames"]]==[.5,1.5,2.5],metadata
        shots=client.tool("video_timeline_scenes",{"threshold":.35,"max_scenes":100})
        assert shots["scene_count"]==3 and shots["total_scene_count"]==3 and shots["threshold"]==.35,shots
        assert shots["analysis"]["approximate"] and shots["analysis"]["min_scene_seconds"]==.25,shots
        assert [(s["start_seconds"],s["end_seconds"],s["representative_seconds"]) for s in shots["scenes"]]==[(0,1,.5),(1,2,1.5),(2,3,2.5)],shots
        shots,visual=call(client,"video_timeline_scenes",{"include_thumbnails":True,"max_scenes":2,"frame_width":128})
        assert shots["truncated"] and shots["scene_count"]==2 and len(visual)==1,shots
        assert [s["representative_actual_seconds"] for s in shots["scenes"]]==[.5,2.5],shots
        client.tool("video_timeline_scenes",{"threshold":0},error=True)
        client.tool("video_timeline_scenes",{"start_seconds":2,"end_seconds":1},error=True)
        metadata,_=call(client,"video_contact_sheet",{"sampling":"scene","max_frames":2,"frame_width":128})
        assert metadata["truncated"] and metadata["frame_count"]==2,metadata
        static=directory/"static.mkv"
        subprocess.run(["ffmpeg","-v","error","-f","lavfi","-i","color=white:s=128x96:r=8:d=3","-c:v","ffv1",str(static)],check=True)
        client.tool("media_open",{"path":str(static)})
        metadata,_=call(client,"video_contact_sheet",{"sampling":"scene","max_frames":3,"frame_width":128})
        assert metadata["scene_fallback"] and metadata["effective_sampling"]=="uniform",metadata
        shots=client.tool("video_timeline_scenes")
        assert shots["scene_count"]==1 and shots["scenes"][0]["representative_seconds"]==1.5,shots
        long_video=directory/"long-static.mkv"
        subprocess.run(["ffmpeg","-v","error","-f","lavfi","-i","color=white:s=128x96:r=8:d=20","-c:v","ffv1",str(long_video)],check=True)
        client.tool("media_open",{"path":str(long_video)})
        metadata,visual=call(client,"video_extract_frames",{"timestamps_seconds":[15.1,.1,7.2,15.1]})
        assert [f["actual_timestamp_seconds"] for f in metadata["frames"]]==[15.125,.125,7.25,15.125],metadata
        assert len(visual)==4
        metadata,_=call(client,"video_contact_sheet",{"sampling":"uniform","max_frames":3,"frame_width":128})
        assert [f["actual_timestamp_seconds"] for f in metadata["frames"]]==[3.375,10,16.75],metadata
        shifted=directory/"shifted.mkv"
        transcode(video,shifted,"-c","copy","-output_ts_offset","5")
        client.tool("media_open",{"path":str(shifted)})
        metadata,_=call(client,"video_extract_frames",{"timestamps_seconds":[.5,1]})
        assert [f["actual_timestamp_seconds"] for f in metadata["frames"]]==[.5,1],metadata
        client.close()
    finally:
        client.terminate()
    small=Client(binary,["--headless","--mcp"],dict(env,WEEBSHOT_INSPECTION_MAX_RESPONSE_BYTES="65536"),log)
    try:
        small.tool("media_open",{"path":str(video)})
        response=small.tool("video_extract_frames",{"timestamps_seconds":[0,1,2],"format":"png"},error=True)
        assert "fewer frames" in str(response),response
        small.close()
    finally:
        small.terminate()
    interruption(binary,directory,video,env,log,Client)
