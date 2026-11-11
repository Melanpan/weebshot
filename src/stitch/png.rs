//! Pixel aspect metadata without resampling source pixels. PNG pHYs stores pixel
//! densities, so a wide source pixel requires a *lower* horizontal density.
use super::{Artifact, check_cancel};
use anyhow::{Result, bail};
use image::{ImageEncoder, RgbaImage};
use std::{
    io::{BufWriter, Write},
    path::Path,
    sync::atomic::AtomicBool,
};

/// FFmpeg's PNG encoder serializes the input SAR numerator/denominator in pHYs.
/// Read it before the image crate discards that ancillary metadata.
pub fn ffmpeg_aspect(bytes: &[u8]) -> (u32, u32) {
    let mut offset = 8;
    while offset + 12 <= bytes.len() {
        let n = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        if n > bytes.len() - offset - 12 {
            break;
        }
        if &bytes[offset + 4..offset + 8] == b"pHYs" && n == 9 {
            let num = u32::from_be_bytes(bytes[offset + 8..offset + 12].try_into().unwrap());
            let den = u32::from_be_bytes(bytes[offset + 12..offset + 16].try_into().unwrap());
            if num > 0 && den > 0 {
                return (num, den);
            }
        }
        offset += n + 12;
    }
    (1, 1)
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}

/// Signature plus the IHDR chunk; pHYs must follow IHDR and precede IDAT.
const IHDR_END: usize = 33;

fn phys_chunk(aspect: (u32, u32)) -> Vec<u8> {
    let mut chunk = Vec::from(&b"pHYs"[..]);
    chunk.extend_from_slice(&aspect.1.to_be_bytes());
    chunk.extend_from_slice(&aspect.0.to_be_bytes());
    chunk.push(0); // unspecified units; only the ratio matters
    let mut out = Vec::with_capacity(chunk.len() + 8);
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out
}

/// Inserts the pHYs chunk right after IHDR while the encoder streams, so the (often
/// 100+ MB) PNG is written once instead of being copied through a second file.
/// Cancellation is checked on every write.
struct PhysWriter<'a> {
    file: BufWriter<std::fs::File>,
    cancel: &'a AtomicBool,
    phys: Vec<u8>,
    /// Bytes seen before the IHDR chunk was complete.
    head: Vec<u8>,
    injected: bool,
}

impl Write for PhysWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(std::io::Error::other("cancelled"));
        }
        if self.injected {
            self.file.write_all(bytes)?;
            return Ok(bytes.len());
        }
        self.head.extend_from_slice(bytes);
        if self.head.len() >= IHDR_END {
            if &self.head[12..16] != b"IHDR" {
                return Err(std::io::Error::other("Invalid PNG header"));
            }
            self.file.write_all(&self.head[..IHDR_END])?;
            self.file.write_all(&self.phys)?;
            self.file.write_all(&self.head[IHDR_END..])?;
            self.head = Vec::new();
            self.injected = true;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Encode into a scratch artifact (removed on any error) and rename it into place.
pub fn save(
    canvas: &RgbaImage,
    path: &Path,
    aspect: (u32, u32),
    cancel: &AtomicBool,
) -> Result<()> {
    let scratch = Artifact(path.with_extension("pixels.png"));
    let mut writer = PhysWriter {
        file: BufWriter::with_capacity(256 * 1024, std::fs::File::create(&scratch.0)?),
        cancel,
        phys: phys_chunk(aspect),
        head: Vec::new(),
        injected: false,
    };
    let result = image::codecs::png::PngEncoder::new(&mut writer).write_image(
        canvas.as_raw(),
        canvas.width(),
        canvas.height(),
        image::ExtendedColorType::Rgba8,
    );
    check_cancel(cancel)?;
    result?;
    if !writer.injected {
        bail!("Invalid PNG header");
    }
    writer.flush()?;
    drop(writer);
    std::fs::rename(&scratch.0, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_crc() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
}
