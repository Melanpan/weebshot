"""Known ASS cues, explicit identifier namespaces, external subs and resolved paths."""
import base64
from pathlib import Path
import subprocess


def fixture(directory):
    ass=directory/"known.ass"
    ass.write_text("""[Script Info]
ScriptType: v4.00+
PlayResX: 320
PlayResY: 180
[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,DejaVu Sans,24,&H000000FF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,8,8,8,1
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.50,0:00:01.50,Default,,0,0,0,,VISIBLE
""")
    video=directory/"known-subs.mkv"
    subprocess.run(["ffmpeg","-v","error","-f","lavfi","-i","color=black:s=320x180:r=8:d=3","-f","lavfi","-i","sine=frequency=440:duration=3","-i",str(ass),"-c:v","ffv1","-c:a","pcm_s16le","-c:s","ass","-metadata:s:s:0","language=eng","-metadata:s:s:0","title=English ASS",str(video)],check=True)
    return video,ass


def rgb(path):
    return subprocess.check_output(["ffmpeg","-v","error","-i",str(path),"-f","rawvideo","-pix_fmt","rgb24","pipe:1"])


def exercise(client,directory,gui=False):
    video,ass=fixture(directory)
    client.tool("media_open",{"path":str(video),"replace_selection":True})
    if gui:
        import time
        deadline=time.monotonic()+5
        while client.tool("gui_get_state")["duration_seconds"]<=0 and time.monotonic()<deadline:
            time.sleep(.02)
    info=client.tool("media_info")
    tracks=client.tool("media_tracks")["tracks"]
    sub=next(t for t in tracks if t["type"]=="subtitle" and not t.get("external"))
    assert sub["ffprobe_index"]==2,sub
    if gui:
        assert sub["mpv_id"]==1,sub
        invalid=client.tool("media_select_track",{"kind":"subtitle","track_id":2},error=True)
        assert "Valid subtitle track IDs" in str(invalid) and "mpv" in str(invalid),invalid
    else:
        assert sub["mpv_id"] is None and info["track_id_namespace"]=="ffprobe"
        client.tool("media_select_track",{"kind":"subtitle","track_id":1,"id_namespace":"mpv"},error=True)
    selected=client.tool("media_select_track",{"kind":"subtitle","track_id":2,"id_namespace":"ffprobe"})
    assert selected["track_id"]==(1 if gui else 2),selected
    # No sleep or intervening property poll: a capture must observe the selection
    # as soon as media_select_track returns, including in a shared GUI frame.
    def shot(name,time,subs=True,**extra):
        out=directory/(name+".png")
        job=client.tool("screenshot_extract",{"output":str(out),"seconds":time,"subtitles":subs,**extra})
        assert Path(job["output"])==out.resolve() and job["outputs"]==[str(out.resolve())],job
        client.wait_job(job)
        return rgb(out)
    inside=shot("ass-inside",1)
    raw=shot("ass-off",1,False)
    outside=shot("ass-outside",2)
    assert inside!=raw and outside==raw,"ASS cue timing/selection was not respected"
    red=sum(1 for i in range(0,len(inside),3) if inside[i]>100 and inside[i+1]<80 and inside[i+2]<80)
    assert red>20,"ASS color/style was lost"
    client.tool("media_select_track",{"kind":"subtitle","external_path":str(ass)})
    assert shot("ass-external",1)==inside,"external ASS differs from embedded styling"
    client.tool("media_select_track",{"kind":"subtitle","track_id":None})
    empty=client.tool("screenshot_extract",{"output":str(directory/"no-active-sub.png"),"seconds":1,"subtitles":True})
    assert empty["warnings"],"missing subtitle source was silently accepted"
    client.wait_job(empty)
    # Both video inspection tools deliberately ignore the selected subtitle track.
    client.tool("media_select_track",{"kind":"subtitle","external_path":str(ass)})
    response=client.rpc("tools/call",{"name":"video_extract_frames","arguments":{"timestamps_seconds":[1],"format":"png"}})
    image=next(c for c in response["result"]["content"] if c["type"]=="image")
    check=directory/"inspection-no-sub.png"
    check.write_bytes(base64.b64decode(image["data"]))
    assert rgb(check)==raw,"inspection unexpectedly burned subtitles"
    base=directory/"explicit-base"
    base.mkdir()
    job=client.tool("screenshot_extract",{"output":"relative.png","base_directory":str(base),"seconds":1,"subtitles":False})
    assert job["output"]==str(base/"relative.png") and job["base_directory"]==str(base),job
    client.wait_job(job)
    failed=client.tool("screenshot_extract",{"output":"missing-parent/frame.png","base_directory":str(base),"subtitles":False},error=True)
    assert str(base/"missing-parent") in str(failed),failed
    assert not (base/"missing-parent").exists()
    cwd=client.tool("gui_get_state" if gui else "timeline_get_state")["output_base_directory"]
    failed=client.tool("screenshot_extract",{"output":"weebshot-nonexistent-output-base/frame.png","subtitles":False},error=True)
    assert str(Path(cwd)/"weebshot-nonexistent-output-base") in str(failed),failed
