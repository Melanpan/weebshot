//! Shared bounded FFmpeg decoding, independent of the GUI.
use crate::export::hide_console;
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    Ok(())
}
use anyhow::{Context, Result, bail};
use crossbeam_channel::bounded;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
/// Read stdout on a helper thread while the owning worker polls cancellation.
/// Both pipe readers are joined and the process is reaped on every exit path.
pub fn pipe(
    ffmpeg: &Path,
    args: &[String],
    cancel: &AtomicBool,
    consume: impl FnMut(&mut dyn Read) -> Result<()>,
) -> Result<()> {
    pipe_with_lines(
        ffmpeg,
        args,
        cancel,
        Duration::from_secs(120),
        None,
        consume,
        |_| {},
    )
}

/// Shared bounded decoder pipe. Inspection can observe showinfo metadata without
/// changing the existing Stitch decoder's output or timeout semantics.
pub fn pipe_with_lines(
    ffmpeg: &Path,
    args: &[String],
    cancel: &AtomicBool,
    idle_timeout: Duration,
    deadline: Option<Instant>,
    mut consume: impl FnMut(&mut dyn Read) -> Result<()>,
    mut line: impl FnMut(&str) + Send + 'static,
) -> Result<()> {
    check_cancel(cancel)?;
    let mut cmd = Command::new(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn().context("Could not start FFmpeg")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = bounded(2);
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        loop {
            let mut chunk = vec![0; 64 * 1024];
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    chunk.truncate(n);
                    if tx.send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });
    let errors = thread::spawn(move || {
        // Retain a bounded tail, while draining the entire pipe.
        let mut input = stderr;
        let mut tail = Vec::new();
        let mut pending = Vec::new();
        let mut buf = [0; 4096];
        while let Ok(n) = input.read(&mut buf) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buf[..n]);
            if tail.len() > 8192 {
                tail.drain(..tail.len() - 8192);
            }
            for &byte in &buf[..n] {
                if byte == b'\n' {
                    line(&String::from_utf8_lossy(&pending));
                    pending.clear();
                } else if pending.len() < 8192 {
                    pending.push(byte);
                }
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    });
    struct Stream<'a> {
        rx: crossbeam_channel::Receiver<std::io::Result<Vec<u8>>>,
        chunk: std::io::Cursor<Vec<u8>>,
        cancel: &'a AtomicBool,
        /// Last time the decoder produced output. A long but steadily progressing
        /// decode (a 60 s HEVC range on a slow CPU) must not be cut off; a silent one must.
        last_output: Instant,
        idle_timeout: Duration,
        deadline: Option<Instant>,
    }
    impl Read for Stream<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    return Err(std::io::Error::other("Cancelled"));
                }
                if self.last_output.elapsed() > self.idle_timeout
                    || self.deadline.is_some_and(|d| Instant::now() >= d)
                {
                    return Err(std::io::Error::other("FFmpeg decode timed out"));
                }
                let n = self.chunk.read(out)?;
                if n > 0 {
                    return Ok(n);
                }
                match self.rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(chunk) => {
                        self.chunk = std::io::Cursor::new(chunk?);
                        self.last_output = Instant::now();
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(0),
                    Err(_) => (),
                }
            }
        }
    }
    let result = {
        let mut stream = Stream {
            rx,
            chunk: std::io::Cursor::new(Vec::new()),
            cancel,
            last_output: Instant::now(),
            idle_timeout,
            deadline,
        };
        consume(&mut stream)
    };
    // On errors, kill immediately. On clean EOF, allow the decoder to exit;
    // never turn a decoder failure without stderr into a successful stitch.
    if result.is_err() {
        let _ = child.kill();
    }
    let wait_started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e);
            }
            Ok(None) => {
                if cancel.load(Ordering::Relaxed) || wait_started.elapsed() > Duration::from_secs(2)
                {
                    let _ = child.kill();
                    break child.wait();
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let _ = reader.join();
    let errors = errors.join().unwrap_or_default();
    check_cancel(cancel)?;
    result?;
    let status = status?;
    if !status.success() {
        bail!(
            "FFmpeg could not decode the video ({status}): {}",
            errors.trim()
        );
    }
    Ok(())
}
