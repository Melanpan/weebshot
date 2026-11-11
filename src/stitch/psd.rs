//! Layered Photoshop export with editable masks derived from pixel ownership.
use super::check_cancel;
use ag_psd::{
    PixelData,
    psd::{
        ImageResources, Layer, LayerAdditionalInfo, LayerMaskData, PixelAspectRatio, Psd,
        WriteOptions,
    },
};
use anyhow::{Result, bail};
use image::{GrayImage, RgbImage, RgbaImage};
use std::sync::atomic::AtomicBool;

// ag-psd encodes from in-memory RGBA layers/masks and retains encoded data.
// Bound both source layer storage and the merged preview before allocating.
pub(super) fn check_budget(layer_pixels: u64, canvas_pixels: u64) -> Result<()> {
    if layer_pixels > 28_000_000 || 8 * layer_pixels + 5 * canvas_pixels > 300_000_000 {
        bail!(
            "The selected layers are too large for PSD export. Include fewer frames or use a shorter range."
        );
    }
    Ok(())
}

pub(super) struct Source {
    pub plane: usize,
    frame: u8,
    offset: (u32, u32),
    layer: Layer,
}

impl Source {
    pub fn new(
        rgb: &RgbImage,
        plane: usize,
        frame: usize,
        origin: (i32, i32),
        offset: (u32, u32),
        time: f64,
    ) -> Self {
        let (width, height) = rgb.dimensions();
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for pixel in rgb.as_raw().as_chunks::<3>().0 {
            pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        let (left, top) = (f64::from(origin.0), f64::from(origin.1));
        let name = format!("Frame {:02} - {:.3}s", frame + 1, time);
        let layer = Layer {
            left: Some(left),
            top: Some(top),
            image_data: Some(PixelData {
                width,
                height,
                data: pixels,
            }),
            additional_info: LayerAdditionalInfo {
                name: Some(name),
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            plane,
            frame: frame as u8 + 1,
            offset,
            layer,
        }
    }

    pub fn make_mask(&mut self, owners: &GrayImage, cancel: &AtomicBool) -> Result<()> {
        let bitmap = self.layer.image_data.as_ref().unwrap();
        let mut data = Vec::with_capacity(bitmap.data.len());
        for y in 0..bitmap.height {
            if y % 32 == 0 {
                check_cancel(cancel)?;
            }
            for x in 0..bitmap.width {
                let value =
                    if owners.get_pixel(self.offset.0 + x, self.offset.1 + y)[0] == self.frame {
                        255
                    } else {
                        0
                    };
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        self.layer.additional_info.mask = Some(LayerMaskData {
            left: self.layer.left,
            top: self.layer.top,
            default_color: Some(0.0),
            image_data: Some(PixelData {
                width: bitmap.width,
                height: bitmap.height,
                data,
            }),
            ..Default::default()
        });
        Ok(())
    }
}

pub(super) fn encode(
    canvas: RgbaImage,
    sources: Vec<Source>,
    aspect: (u32, u32),
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    check_cancel(cancel)?;
    let (width, height) = canvas.dimensions();
    let document = Psd {
        width: f64::from(width),
        height: f64::from(height),
        image_data: Some(PixelData {
            width,
            height,
            data: canvas.into_raw(),
        }),
        image_resources: Some(ImageResources {
            pixel_aspect_ratio: Some(PixelAspectRatio {
                aspect: f64::from(aspect.0) / f64::from(aspect.1),
            }),
            ..Default::default()
        }),
        children: Some(sources.into_iter().map(|source| source.layer).collect()),
        ..Default::default()
    };
    let bytes = ag_psd::write_psd(
        &document,
        &WriteOptions {
            no_background: Some(true),
            ..Default::default()
        },
    );
    check_cancel(cancel)?;
    Ok(bytes)
}
