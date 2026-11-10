//! Pixel aspect metadata without resampling source pixels. PNG pHYs stores pixel
//! densities, so a wide source pixel requires a *lower* horizontal density.
use super::{Artifact, check_cancel};
use anyhow::{Result, bail};
use image::{ImageEncoder, RgbaImage};
use std::{
    io::{Read, Write},
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

/// Cancellation is checked during PNG compression writes and the bounded metadata
/// copy. A separate scratch artifact guarantees partial files are removed on errors.
pub fn save(
    canvas: &RgbaImage,
    path: &Path,
    aspect: (u32, u32),
    cancel: &AtomicBool,
) -> Result<()> {
    struct CancelWriter<'a> {
        file: std::fs::File,
        cancel: &'a AtomicBool,
    }
    impl Write for CancelWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(std::io::Error::other("cancelled"));
            }
            self.file.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    let scratch = Artifact(path.with_extension("pixels.png"));
    let writer = CancelWriter {
        file: std::fs::File::create(&scratch.0)?,
        cancel,
    };
    let result = image::codecs::png::PngEncoder::new(writer).write_image(
        canvas.as_raw(),
        canvas.width(),
        canvas.height(),
        image::ExtendedColorType::Rgba8,
    );
    check_cancel(cancel)?;
    result?;
    let mut src = std::fs::File::open(&scratch.0)?;
    let mut dst = std::fs::File::create(path)?;
    let mut header = [0; 33];
    src.read_exact(&mut header)?;
    if &header[12..16] != b"IHDR" {
        bail!("Invalid PNG header");
    }
    dst.write_all(&header)?;
    let mut chunk = Vec::from(&b"pHYs"[..]);
    chunk.extend_from_slice(&aspect.1.to_be_bytes());
    chunk.extend_from_slice(&aspect.0.to_be_bytes());
    chunk.push(0); // unspecified units; only the ratio matters
    dst.write_all(&9u32.to_be_bytes())?;
    dst.write_all(&chunk)?;
    dst.write_all(&crc32(&chunk).to_be_bytes())?;
    let mut buf = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
    }
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
