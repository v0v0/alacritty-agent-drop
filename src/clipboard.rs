use std::path::PathBuf;

use anyhow::Result;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn capture_image_to_temp() -> Result<Option<PathBuf>> {
    imp::capture_image_to_temp()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn capture_image_to_temp() -> Result<Option<PathBuf>> {
    Ok(None)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use std::fs;
    use std::path::PathBuf;

    use anyhow::{Context, Result, bail};
    use arboard::{Clipboard, Error as ClipboardError};
    use image::{ColorType, ImageFormat};
    use uuid::Uuid;

    pub fn capture_image_to_temp() -> Result<Option<PathBuf>> {
        let mut clipboard = Clipboard::new().context("failed to open the system clipboard")?;
        let image = match clipboard.get_image() {
            Ok(image) => image,
            Err(ClipboardError::ContentNotAvailable) => return Ok(None),
            Err(error) => return Err(anyhow::anyhow!("failed to read clipboard image: {error}")),
        };

        if image.width == 0 || image.height == 0 {
            bail!("clipboard image has invalid zero dimensions");
        }

        let expected_len = image
            .width
            .checked_mul(image.height)
            .and_then(|pixels| pixels.checked_mul(4))
            .context("clipboard image dimensions overflow")?;
        if image.bytes.len() != expected_len {
            bail!(
                "clipboard image buffer has {} bytes; expected {} RGBA bytes",
                image.bytes.len(),
                expected_len
            );
        }

        let width = u32::try_from(image.width).context("clipboard image width is too large")?;
        let height = u32::try_from(image.height).context("clipboard image height is too large")?;
        let temp_dir = std::env::temp_dir().join("agentdrop").join("clipboard");
        fs::create_dir_all(&temp_dir)
            .with_context(|| format!("failed to create {}", temp_dir.display()))?;

        let path = temp_dir.join(format!(
            "clipboard-{}.png",
            Uuid::new_v4().simple()
        ));
        image::save_buffer_with_format(
            &path,
            image.bytes.as_ref(),
            width,
            height,
            ColorType::Rgba8,
            ImageFormat::Png,
        )
        .with_context(|| format!("failed to encode clipboard image to {}", path.display()))?;

        Ok(Some(path))
    }
}
