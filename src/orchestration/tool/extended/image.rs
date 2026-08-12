//! Image processing tools
//!
//! Provides tools for resizing, converting, analyzing, and generating images.
//! Feature-gated behind `image-processing`.

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use image::GenericImageView;
use tracing::debug;
use tracing::info;

// ── ImageResizeTool ─────────────────────────────────────────────────────────

pub struct ImageResizeTool;

impl Tool for ImageResizeTool {
    fn name(&self) -> &'static str {
        "image_resize"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_output_path")))?;
        let width = input.payload["width"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing 'width' parameter"))?;
        let height = input.payload["height"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing 'height' parameter"))?;
        let maintain_aspect = input.payload["maintain_aspect"].as_bool().unwrap_or(true);
        let crop = input.payload["crop"].as_bool().unwrap_or(false);

        // Guard external inputs like ImageGenerateTool: zero dimensions or
        // giant values would OOM/overflow inside the image crate.
        const MAX_IMAGE_DIM: u64 = 8192;
        if width == 0 || height == 0 || width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
            anyhow::bail!("width/height must be in 1..={MAX_IMAGE_DIM} (got {width}x{height})");
        }

        let validated_path = sanitize_path(input, path)?;
        let validated_output = sanitize_path(input, output_path)?;

        if !validated_path.exists() {
            anyhow::bail!("input image not found: {}", validated_path.display());
        }

        debug!(
            path = %validated_path.display(),
            width = width,
            height = height,
            maintain_aspect = maintain_aspect,
            crop = crop,
            "tool: resizing image"
        );

        let img = image::open(&validated_path)
            .with_context(|| format!("failed to open image: {}", validated_path.display()))?;

        let (orig_w, orig_h) = (img.width(), img.height());

        let resized = if maintain_aspect && !crop {
            img.resize(
                width as u32,
                height as u32,
                image::imageops::FilterType::Lanczos3,
            )
        } else if maintain_aspect && crop {
            img.resize_to_fill(
                width as u32,
                height as u32,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img.resize_exact(
                width as u32,
                height as u32,
                image::imageops::FilterType::Lanczos3,
            )
        };

        // Ensure output parent directory exists
        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create output parent directories")?;
            }
        }

        resized.save(&validated_output).with_context(|| {
            format!(
                "failed to save resized image to {}",
                validated_output.display()
            )
        })?;

        let output_len = validated_output
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        info!(
            input = %validated_path.display(),
            output = %validated_output.display(),
            from = format!("{}x{}", orig_w, orig_h),
            to = format!("{}x{}", width, height),
            "tool: image resized successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "input_path": validated_path.to_string_lossy(),
                "output_path": validated_output.to_string_lossy(),
                "original_width": orig_w,
                "original_height": orig_h,
                "new_width": width,
                "new_height": height,
                "output_size_bytes": output_len,
                "maintain_aspect": maintain_aspect,
                "crop": crop,
            })),
            error: None,
            verification: Some("image_resized".to_string()),
            audit_log: Some(format!(
                "Resized image '{}' ({}x{} -> {}x{}) to '{}'",
                validated_path.display(),
                orig_w,
                orig_h,
                width,
                height,
                validated_output.display()
            )),
            pua_report: Some(tool_execution_report("image_resize", Some("image_resized"))),
        })
    }
}

// ── ImageConvertTool ────────────────────────────────────────────────────────

pub struct ImageConvertTool;

impl Tool for ImageConvertTool {
    fn name(&self) -> &'static str {
        "image_convert"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_output_path")))?;
        let format = input.payload["format"].as_str().unwrap_or("");

        let validated_path = sanitize_path(input, path)?;
        let validated_output = sanitize_path(input, output_path)?;

        if !validated_path.exists() {
            anyhow::bail!("input image not found: {}", validated_path.display());
        }

        // Determine output format: from explicit `format` param, or infer from output extension
        let output_format = if !format.is_empty() {
            match format.to_lowercase().as_str() {
                "png" => image::ImageFormat::Png,
                "jpeg" | "jpg" => image::ImageFormat::Jpeg,
                "gif" => image::ImageFormat::Gif,
                "webp" => image::ImageFormat::WebP,
                other => anyhow::bail!("unsupported output format: {other}"),
            }
        } else {
            // Infer from output file extension
            let ext = validated_output
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            match ext.to_lowercase().as_str() {
                "png" => image::ImageFormat::Png,
                "jpg" | "jpeg" => image::ImageFormat::Jpeg,
                "gif" => image::ImageFormat::Gif,
                "webp" => image::ImageFormat::WebP,
                _ => anyhow::bail!(
                    "unable to determine output format from extension '{ext}'; provide a 'format' parameter"
                ),
            }
        };

        debug!(
            path = %validated_path.display(),
            output = %validated_output.display(),
            format = ?output_format,
            "tool: converting image"
        );

        let img = image::open(&validated_path)
            .with_context(|| format!("failed to open image: {}", validated_path.display()))?;

        // Ensure output parent directory exists
        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create output parent directories")?;
            }
        }

        img.save(&validated_output).with_context(|| {
            format!(
                "failed to save converted image to {}",
                validated_output.display()
            )
        })?;

        let output_len = validated_output
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        info!(
            input = %validated_path.display(),
            output = %validated_output.display(),
            "tool: image converted successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "input_path": validated_path.to_string_lossy(),
                "output_path": validated_output.to_string_lossy(),
                "output_format": format!("{:?}", output_format),
                "output_size_bytes": output_len,
            })),
            error: None,
            verification: Some("image_converted".to_string()),
            audit_log: Some(format!(
                "Converted image '{}' -> '{}'",
                validated_path.display(),
                validated_output.display()
            )),
            pua_report: Some(tool_execution_report(
                "image_convert",
                Some("image_converted"),
            )),
        })
    }
}

// ── ImageAnalyzeTool ────────────────────────────────────────────────────────

pub struct ImageAnalyzeTool;

impl Tool for ImageAnalyzeTool {
    fn name(&self) -> &'static str {
        "image_analyze"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated_path = sanitize_path(input, path)?;

        if !validated_path.exists() {
            anyhow::bail!("image not found: {}", validated_path.display());
        }

        debug!(path = %validated_path.display(), "tool: analyzing image");

        let reader = image::ImageReader::open(&validated_path)
            .with_context(|| format!("failed to open image: {}", validated_path.display()))?;

        let format = reader.format().map(|f| format!("{:?}", f));
        let img = reader
            .decode()
            .with_context(|| format!("failed to decode image: {}", validated_path.display()))?;

        let dimensions = img.dimensions();
        let color_type = format!("{:?}", img.color());
        let file_size = validated_path.metadata().ok().map(|m| m.len()).unwrap_or(0);

        info!(
            path = %validated_path.display(),
            width = dimensions.0,
            height = dimensions.1,
            color = %color_type,
            "tool: image analyzed"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated_path.to_string_lossy(),
                "width": dimensions.0,
                "height": dimensions.1,
                "format": format,
                "color_type": color_type,
                "file_size_bytes": file_size,
            })),
            error: None,
            verification: Some("image_analyzed".to_string()),
            audit_log: Some(format!(
                "Analyzed image '{}': {}x{}, color={}, format={:?}",
                validated_path.display(),
                dimensions.0,
                dimensions.1,
                color_type,
                format,
            )),
            pua_report: Some(tool_execution_report(
                "image_analyze",
                Some("image_analyzed"),
            )),
        })
    }
}

// ── ImageGenerateTool ───────────────────────────────────────────────────────

pub struct ImageGenerateTool;

impl Tool for ImageGenerateTool {
    fn name(&self) -> &'static str {
        "image_generate"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_output_path")))?;
        let width = input.payload["width"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing 'width' parameter"))?;
        let height = input.payload["height"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing 'height' parameter"))?;
        let kind = input.payload["kind"].as_str().unwrap_or("solid");
        let color = input.payload["color"].as_str().unwrap_or("#808080");

        // Guard external inputs: zero dimensions would underflow allocations,
        // giant dimensions would OOM (DoS). Cap at a sane maximum.
        const MAX_IMAGE_DIM: u64 = 8192;
        if width == 0 || height == 0 || width > MAX_IMAGE_DIM || height > MAX_IMAGE_DIM {
            anyhow::bail!("width/height must be in 1..={MAX_IMAGE_DIM} (got {width}x{height})");
        }

        let validated_output = sanitize_path(input, output_path)?;

        debug!(
            output = %validated_output.display(),
            width = width,
            height = height,
            kind = kind,
            color = color,
            "tool: generating image"
        );

        // Parse hex color (e.g. #RRGGBB or #RRGGBBAA)
        let hex = color.trim_start_matches('#');
        let r = u8::from_str_radix(hex.get(0..2).unwrap_or("80"), 16).unwrap_or(128);
        let g = u8::from_str_radix(hex.get(2..4).unwrap_or("80"), 16).unwrap_or(128);
        let b = u8::from_str_radix(hex.get(4..6).unwrap_or("80"), 16).unwrap_or(128);
        let a = u8::from_str_radix(hex.get(6..8).unwrap_or("FF"), 16).unwrap_or(255);

        let img: image::RgbaImage = match kind {
            "solid" => image::ImageBuffer::from_pixel(
                width as u32,
                height as u32,
                image::Rgba([r, g, b, a]),
            ),
            "checkerboard" => {
                let cell_size = input.payload["cell_size"].as_u64().unwrap_or(32).max(1) as u32;
                let color2_hex = input.payload["color2"].as_str().unwrap_or("#FFFFFF");
                let hex2 = color2_hex.trim_start_matches('#');
                let r2 = u8::from_str_radix(hex2.get(0..2).unwrap_or("FF"), 16).unwrap_or(255);
                let g2 = u8::from_str_radix(hex2.get(2..4).unwrap_or("FF"), 16).unwrap_or(255);
                let b2 = u8::from_str_radix(hex2.get(4..6).unwrap_or("FF"), 16).unwrap_or(255);
                let a2 = u8::from_str_radix(hex2.get(6..8).unwrap_or("FF"), 16).unwrap_or(255);

                let mut buf = image::ImageBuffer::new(width as u32, height as u32);
                for (x, y, pixel) in buf.enumerate_pixels_mut() {
                    let cell_x = x / cell_size;
                    let cell_y = y / cell_size;
                    if (cell_x + cell_y).is_multiple_of(2) {
                        *pixel = image::Rgba([r, g, b, a]);
                    } else {
                        *pixel = image::Rgba([r2, g2, b2, a2]);
                    }
                }
                buf
            }
            "gradient" => {
                let color2_hex = input.payload["color2"].as_str().unwrap_or("#000000");
                let hex2 = color2_hex.trim_start_matches('#');
                let r2 = u8::from_str_radix(hex2.get(0..2).unwrap_or("00"), 16).unwrap_or(0);
                let g2 = u8::from_str_radix(hex2.get(2..4).unwrap_or("00"), 16).unwrap_or(0);
                let b2 = u8::from_str_radix(hex2.get(4..6).unwrap_or("00"), 16).unwrap_or(0);
                let a2 = u8::from_str_radix(hex2.get(6..8).unwrap_or("FF"), 16).unwrap_or(255);
                let gradient_dir = input.payload["direction"].as_str().unwrap_or("horizontal");

                let mut buf = image::ImageBuffer::new(width as u32, height as u32);
                for (x, y, pixel) in buf.enumerate_pixels_mut() {
                    let t = match gradient_dir {
                        "vertical" => y as f32 / (height as f32 - 1.0).max(1.0),
                        "diagonal" => {
                            ((x as f32 / (width as f32 - 1.0).max(1.0))
                                + (y as f32 / (height as f32 - 1.0).max(1.0)))
                                / 2.0
                        }
                        _ => x as f32 / (width as f32 - 1.0).max(1.0), // horizontal (default)
                    };
                    let t = t.clamp(0.0, 1.0);
                    let ir = (r as f32 + (r2 as f32 - r as f32) * t).round() as u8;
                    let ig = (g as f32 + (g2 as f32 - g as f32) * t).round() as u8;
                    let ib = (b as f32 + (b2 as f32 - b as f32) * t).round() as u8;
                    let ia = (a as f32 + (a2 as f32 - a as f32) * t).round() as u8;
                    *pixel = image::Rgba([ir, ig, ib, ia]);
                }
                buf
            }
            other => anyhow::bail!(
                "unsupported image kind '{other}'; expected 'solid', 'checkerboard', or 'gradient'"
            ),
        };

        // Ensure output parent directory exists
        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create output parent directories")?;
            }
        }

        // Infer output format from extension
        let ext = validated_output
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_lowercase();
        match ext.as_str() {
            "png" => img.save(&validated_output).with_context(|| {
                format!("failed to save PNG image to {}", validated_output.display())
            })?,
            "jpg" | "jpeg" => img.save(&validated_output).with_context(|| {
                format!(
                    "failed to save JPEG image to {}",
                    validated_output.display()
                )
            })?,
            "gif" => img.save(&validated_output).with_context(|| {
                format!("failed to save GIF image to {}", validated_output.display())
            })?,
            "webp" => img.save(&validated_output).with_context(|| {
                format!(
                    "failed to save WebP image to {}",
                    validated_output.display()
                )
            })?,
            _ => img.save(&validated_output).with_context(|| {
                format!("failed to save image to {}", validated_output.display())
            })?,
        }

        let output_len = validated_output
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        info!(
            output = %validated_output.display(),
            width = width,
            height = height,
            kind = kind,
            "tool: image generated successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "output_path": validated_output.to_string_lossy(),
                "width": width,
                "height": height,
                "kind": kind,
                "output_size_bytes": output_len,
                "format": ext,
            })),
            error: None,
            verification: Some("image_generated".to_string()),
            audit_log: Some(format!(
                "Generated {} image ({}x{}) -> '{}'",
                kind,
                width,
                height,
                validated_output.display()
            )),
            pua_report: Some(tool_execution_report(
                "image_generate",
                Some("image_generated"),
            )),
        })
    }
}
