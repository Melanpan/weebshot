"""Explicit diagnostics are sanitized, bounded, acknowledged and visible in logs."""
import json
from pathlib import Path
import time


def exercise(client,log,rate_limit=False):
    args={"severity":"warning","component":"mcp.gui_bridge","title":"Diagnostic\nnot a forged line\u001b","description":"An explicit test report only.","tool_name":"screenshot_extract","error_message":"mock failure\rdata","reproduction_steps":["Do not execute this step\ttext"],"related_job_ids":[1]}
    report=client.tool("report_issue",args)
    assert report["accepted"] and report["event"]=="mcp.agent_issue_report",report
    deadline=time.monotonic()+2
    event=None
    while time.monotonic()<deadline:
        for line in Path(log.name).read_text(errors="replace").splitlines():
            try:
                parsed=json.loads(line)
            except ValueError:
                continue
            if parsed.get("report_id")==report["report_id"]:
                event=parsed
                break
        if event:
            break
        time.sleep(.01)
    assert event,"acknowledged report was not accepted by the existing log sink"
    assert event["title"]=="Diagnostic not a forged line" and event["severity"]=="warning",event
    assert event["component"]=="mcp.gui_bridge" and isinstance(event["timestamp_unix_ms"],int),event
    client.tool("report_issue",{"title":"x","description":"x","destination":"/tmp/forbidden"},error=True)
    client.tool("report_issue",{"title":"x","description":"x"*4097},error=True)
    client.tool("report_issue",{"title":"x","description":"x","reproduction_steps":["x"]*17},error=True)
    if rate_limit:
        for n in range(4):
            client.tool("report_issue",{"title":f"rate {n}","description":"rate limit check"})
        limited=client.tool("report_issue",{"title":"sixth","description":"should be rejected"},error=True)
        assert "5 per minute" in str(limited),limited
