use crate::AgentError;
use std::{fs::OpenOptions, io::BufWriter, path::Path};

/// Normalize premultiplied linear RGBA8 render-target bytes to a straight-alpha
/// PNG with gAMA=1.0. Encoding runs on an artifact worker, never the UI thread.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), AgentError> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|v| v.checked_mul(4))
        .and_then(|v| usize::try_from(v).ok());
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(AgentError::new(
            "artifact_error",
            "invalid capture dimensions or byte length",
        ));
    }
    if path.exists() {
        return Err(AgentError::new(
            "artifact_error",
            "capture destination already exists",
        ));
    }
    let partial = path.with_extension("png.partial");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
    let mut pixels = rgba.to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = if alpha == 0 {
                0
            } else {
                ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
            };
        }
    }
    let result = (|| {
        let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_gamma(png::ScaledFloat::new(1.));
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&pixels)?;
        writer.finish()?;
        Ok::<_, png::EncodingError>(())
    })();
    result.map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
    std::fs::rename(partial, path).map_err(|e| AgentError::new("artifact_error", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_round_trips_straight_alpha_and_refuses_overwrite() {
        let path =
            std::env::temp_dir().join(format!("flutter-agent-png-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        write_png(
            &path,
            3,
            1,
            &[128, 64, 0, 128, 255, 0, 255, 255, 9, 9, 9, 0],
        )
        .unwrap();
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        assert_eq!(reader.info().gama_chunk, Some(png::ScaledFloat::new(1.)));
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(
            &buffer[..info.buffer_size()],
            &[255, 128, 0, 128, 255, 0, 255, 255, 0, 0, 0, 0]
        );
        assert!(write_png(&path, 3, 1, &[0; 12]).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
