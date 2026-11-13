#![cfg(target_os = "linux")]
//! End-to-end tests against the real binary and generated media (no external fixtures).
#[test]
fn headless_stdio_media_workflow() {
    let status = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/test-mcp.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_weebshot")])
        .status()
        .expect("python3 is required for the MCP integration harness");
    assert!(status.success());
}
#[test]
#[ignore = "requires Xvfb, Openbox, xdotool, xprop and software OpenGL; run cargo test --test mcp gui_attachment -- --ignored"]
fn gui_attachment() {
    let status = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/test-mcp.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_weebshot"), "--gui"])
        .status()
        .expect("python3 is required for the MCP integration harness");
    assert!(status.success());
}

#[test]
fn visual_inspection_workflow() {
    let status = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/test-mcp.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_weebshot"), "--inspection"])
        .status()
        .expect("python3 is required for the MCP integration harness");
    assert!(status.success());
}

#[test]
#[ignore = "requires Xvfb/software OpenGL"]
fn gui_bridge_reliability() {
    let status = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/test-mcp.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_weebshot"), "--reliability"])
        .status()
        .expect("python3 is required for the MCP integration harness");
    assert!(status.success());
}
