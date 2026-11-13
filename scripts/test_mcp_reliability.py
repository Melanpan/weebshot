"""GUI bridge load tests and a deterministic reproduction of UI-thread probe starvation."""
import json
import os
from pathlib import Path
import signal
import shlex
import shutil
import subprocess
import time


def send(client,name,args=None):
    client.next_id+=1
    key=client.next_id
    client.process.stdin.write(json.dumps({"jsonrpc":"2.0","id":key,"method":"tools/call","params":{"name":name,"arguments":args or {}}})+"\n")
    client.process.stdin.flush()
    return key


def receive(client,keys,timeout=15):
    pending=set(keys)
    result={}
    deadline=time.monotonic()+timeout
    while pending:
        message=client.messages.get(timeout=max(.01,deadline-time.monotonic()))
        if message.get("id") in pending:
            result[message["id"]]=message
            pending.remove(message["id"])
        elif "id" in message:
            client.responses[message["id"]]=message
    return result


def data(response):
    assert "error" not in response,response
    value=response["result"]
    assert not value.get("isError"),value
    return value.get("structuredContent") or json.loads(value["content"][0]["text"])


def gui(binary,directory,video,env,log,Client,diagnose=False):
    read_fd,write_fd=os.pipe()
    xvfb=subprocess.Popen(["Xvfb","-displayfd",str(write_fd),"-screen","0","1600x1000x24"],pass_fds=(write_fd,),stdout=log,stderr=log)
    os.close(write_fd)
    with os.fdopen(read_fd) as pipe:
        display=pipe.readline().strip()
    tools=directory/"tools"
    tools.mkdir()
    marker=directory/"gifsicle-probes"
    gifsicle=tools/"gifsicle"
    gifsicle.write_text("#!/bin/sh\nif [ -f "+shlex.quote(str(marker))+" ]; then sleep 4; fi\nprintf x >> "+shlex.quote(str(marker))+"\nprintf 'gifsicle 1.96\\n'\n")
    gifsicle.chmod(0o700)
    if not diagnose:
        ffmpeg=tools/"ffmpeg"
        ffmpeg.write_text("#!"+shutil.which("python3")+"\nimport os,sys,time\ntime.sleep(.35)\nos.execv("+repr(shutil.which("ffmpeg"))+",["+repr(shutil.which("ffmpeg"))+",*sys.argv[1:]])\n")
        ffmpeg.chmod(0o700)
    app_env=dict(env,DISPLAY=":"+display,WINIT_UNIX_BACKEND="x11",LIBGL_ALWAYS_SOFTWARE="1",XDG_DATA_HOME=str(directory/"data"),PATH=str(tools)+os.pathsep+env["PATH"])
    settings=directory/"data"/"weebshot"/"app.ron"
    settings.parent.mkdir(parents=True)
    settings.write_text('{"app":"(mcp_enabled:true,check_updates:false,sounds:false,hwdec:false)"}')
    app=subprocess.Popen([binary],env=app_env,stdout=log,stderr=log)
    client=None
    try:
        socket=directory/f"weebshot-mcp-{os.getuid()}"/"gui.sock"
        deadline=time.monotonic()+20
        while not socket.exists() and app.poll() is None and time.monotonic()<deadline:
            time.sleep(.05)
        assert socket.exists(),"GUI did not start"
        client=Client(binary,["--mcp","--mcp-socket",str(socket)],env,log)
        client.tool("gui_open_media",{"path":str(video)})
        deadline=time.monotonic()+5
        while client.tool("gui_get_state")["duration_seconds"]<=0 and time.monotonic()<deadline:
            time.sleep(.05)
        started=time.monotonic()
        submissions=[send(client,"screenshot_extract",{"output":str(directory/f"probe-{i}.png"),"subtitles":False}) for i in range(4)]
        status=send(client,"jobs_list")
        responses=receive(client,submissions+[status])
        elapsed=time.monotonic()-started
        failures=sum(bool(r.get("error") or r.get("result",{}).get("isError")) for r in responses.values())
        if diagnose:
            assert failures>0 and elapsed>3,(failures,elapsed)
            print(f"Reproduced starvation: {failures} failed request(s), status delayed {elapsed:.2f}s, only four submissions; synchronous second gifsicle probe slept 4s")
        else:
            assert failures==0,(failures,[r for r in responses.values() if r.get("result",{}).get("isError")])
            assert elapsed<2,f"submission/status took {elapsed:.2f}s"
            assert marker.read_text()=="x","GUI submission reran the startup tool probe"
            for key in submissions:
                client.wait_job(data(responses[key]))
            completed=data(responses[submissions[0]])["job_id"]
            for count in (1,3,4,8):
                start=time.monotonic()
                keys=[send(client,"screenshot_extract",{"output":str(directory/f"load-{count}-{i}.png"),"subtitles":False}) for i in range(count)]
                poll=send(client,"jobs_get",{"job_id":completed})
                results=receive(client,keys+[poll])
                assert time.monotonic()-start<2,"submission queue starved polling"
                assert data(results[poll])["status"]=="done"
                jobs=[data(results[key]) for key in keys]
                active=client.tool("jobs_get",{"job_id":jobs[0]["job_id"]})
                assert active["status"] in ("queued","running","done")
                cancelled=set()
                if count>=3:
                    for job in (jobs[-1],jobs[0]):
                        client.tool("jobs_cancel",{"job_id":job["job_id"]})
                        cancelled.add(job["job_id"])
                for job in jobs:
                    client.wait_job(job,"cancelled" if job["job_id"] in cancelled else "done")
                assert client.tool("jobs_get",{"job_id":completed})["status"]=="done"
            # Deliberately exceed ingress capacity: reject predictably, keep the
            # control lane available, and never disguise backpressure as timeout.
            keys=[send(client,"screenshot_extract",{"output":str(directory/f"burst-{i}.png"),"subtitles":False}) for i in range(64)]
            poll=send(client,"jobs_get",{"job_id":completed})
            results=receive(client,keys+[poll])
            assert data(results[poll])["status"]=="done"
            accepted=[];rejected=[]
            for key in keys:
                result=results[key]
                if result.get("error") or result.get("result",{}).get("isError"):
                    message=json.dumps(result).lower()
                    assert "timeout" not in message and "deadline" not in message,result
                    assert any(word in message for word in ("full","capacity","busy")),result
                    rejected.append(result)
                else:
                    accepted.append(data(result))
            assert accepted and rejected
            for job in accepted:
                current=client.tool("jobs_get",{"job_id":job["job_id"]})
                if current["status"] in ("queued","running"):
                    client.tool("jobs_cancel",{"job_id":job["job_id"]})
            for job in accepted:
                deadline=time.monotonic()+10
                while client.tool("jobs_get",{"job_id":job["job_id"]})["status"] not in ("done","cancelled","failed"):
                    assert time.monotonic()<deadline
                    time.sleep(.02)
            app.terminate();app.wait(timeout=10)
            client.tool("jobs_get",{"job_id":completed},error=True)
            print("GUI reliability passed: 1/3/4/8 concurrent submissions, polling, cancellation and explicit overload responses")
        client.close()
    finally:
        if client:
            client.terminate()
        if app.poll() is None:
            app.terminate()
            app.wait(timeout=10)
        xvfb.terminate()
        xvfb.wait(timeout=10)
