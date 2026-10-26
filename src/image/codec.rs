//! WXGF 仅接受有边界的 length-prefixed Annex B 分区，不把容器尾部当裸流。
use super::Failure;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 20_000_000;
const CODEC_VERSION: &str = "8.0.1";
const SOURCE_SHA: &str = "05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41";

pub(super) struct StandardImage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub converted_from: Option<&'static str>,
}

pub(super) fn standard(data: &[u8], deadline: Instant) -> Result<StandardImage, Failure> {
    if data.len() > MAX_BYTES {
        return Err(Failure::DecodeFailed);
    }
    if data.starts_with(b"wxgf") {
        let parts = partitions(data)?;
        // 多分区近似等大时，已知 WXGF 布局为 mask/color 交替；只取首个可解 color。
        let max = parts.iter().map(|p| p.len()).max().unwrap_or(0);
        let animated = parts.len() > 1 && max * 10 < data.len() * 6;
        let mut last = Failure::TranscodeFailed;
        for (i, part) in parts.iter().enumerate() {
            if animated && i % 2 == 0 {
                continue;
            }
            match transcode(part, deadline).and_then(|b| standard(&b, deadline)) {
                Ok(mut img) => {
                    img.converted_from = Some("wxgf");
                    return Ok(img);
                }
                Err(Failure::Timeout) => return Err(Failure::Timeout),
                Err(e) => last = e,
            }
        }
        return Err(last);
    }
    let format = image::guess_format(data).map_err(|_| Failure::FormatUnsupported)?;
    let reader = ImageReader::with_format(Cursor::new(data), format);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| Failure::DecodeFailed)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Failure::DecodeFailed);
    }
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(100 * 1024 * 1024);
    limits.max_image_width = Some(20000);
    limits.max_image_height = Some(20000);
    reader.limits(limits.clone());
    let mut converted_from = match format {
        ImageFormat::Gif => Some("gif"),
        ImageFormat::Bmp => Some("bmp"),
        ImageFormat::Tiff => Some("tiff"),
        _ => None,
    };
    let img = match format {
        ImageFormat::Gif => {
            let mut d = image::codecs::gif::GifDecoder::new(Cursor::new(data))
                .map_err(|_| Failure::DecodeFailed)?;
            d.set_limits(limits).map_err(|_| Failure::DecodeFailed)?;
            image::DynamicImage::ImageRgba8(
                d.into_frames()
                    .next()
                    .ok_or(Failure::DecodeFailed)?
                    .map_err(|_| Failure::DecodeFailed)?
                    .into_buffer(),
            )
        }
        ImageFormat::Png => {
            let mut d = image::codecs::png::PngDecoder::new(Cursor::new(data))
                .map_err(|_| Failure::DecodeFailed)?;
            d.set_limits(limits).map_err(|_| Failure::DecodeFailed)?;
            if d.is_apng().map_err(|_| Failure::DecodeFailed)? {
                converted_from = Some("png");
                image::DynamicImage::ImageRgba8(
                    d.apng()
                        .map_err(|_| Failure::DecodeFailed)?
                        .into_frames()
                        .next()
                        .ok_or(Failure::DecodeFailed)?
                        .map_err(|_| Failure::DecodeFailed)?
                        .into_buffer(),
                )
            } else {
                reader.decode().map_err(|_| Failure::DecodeFailed)?
            }
        }
        ImageFormat::WebP => {
            let mut d = image::codecs::webp::WebPDecoder::new(Cursor::new(data))
                .map_err(|_| Failure::DecodeFailed)?;
            d.set_limits(limits).map_err(|_| Failure::DecodeFailed)?;
            if d.has_animation() {
                converted_from = Some("webp");
                image::DynamicImage::ImageRgba8(
                    d.into_frames()
                        .next()
                        .ok_or(Failure::DecodeFailed)?
                        .map_err(|_| Failure::DecodeFailed)?
                        .into_buffer(),
                )
            } else {
                reader.decode().map_err(|_| Failure::DecodeFailed)?
            }
        }
        ImageFormat::Jpeg | ImageFormat::Bmp | ImageFormat::Tiff => {
            reader.decode().map_err(|_| Failure::DecodeFailed)?
        }
        _ => return Err(Failure::FormatUnsupported),
    };
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .map_err(|_| Failure::DecodeFailed)?;
    if out.get_ref().len() > MAX_BYTES {
        return Err(Failure::DecodeFailed);
    }
    Ok(StandardImage {
        bytes: out.into_inner(),
        width: img.width(),
        height: img.height(),
        converted_from,
    })
}

fn partitions(data: &[u8]) -> Result<Vec<&[u8]>, Failure> {
    if data.len() < 15 {
        return Err(Failure::ResourceIncomplete);
    }
    if !data.starts_with(b"wxgf") {
        return Err(Failure::FormatUnsupported);
    }
    let header = data[4] as usize;
    if header < 5 || header >= data.len() {
        return Err(Failure::DecodeFailed);
    }
    let mut parts = Vec::new();
    // 头后允许少量未知元数据，但起点须有 BE 长度和完整参数集，而非只匹配起始码。
    let mut offset = (header..data.len().min(header + 256))
        .find(|&o| {
            if o + 8 > data.len() {
                return false;
            }
            let len = u32::from_be_bytes(data[o..o + 4].try_into().unwrap()) as usize;
            len > 6
                && (data[o + 4..].starts_with(&[0, 0, 0, 1])
                    || data[o + 4..].starts_with(&[0, 0, 1]))
        })
        .ok_or(Failure::DecodeFailed)?;
    while offset < data.len() {
        if parts.len() >= 64 {
            return Err(Failure::FormatUnsupported);
        }
        if offset + 4 > data.len() {
            return Err(Failure::ResourceIncomplete);
        }
        let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if size == 0 {
            return Err(Failure::DecodeFailed);
        }
        let end = offset
            .checked_add(size)
            .filter(|&e| e <= data.len())
            .ok_or(Failure::ResourceIncomplete)?;
        let part = &data[offset..end];
        validate_nalus(part, parts.is_empty())?;
        parts.push(part);
        offset = end;
    }
    if parts.is_empty() {
        return Err(Failure::DecodeFailed);
    }
    Ok(parts)
}
fn validate_nalus(data: &[u8], require_sets: bool) -> Result<(), Failure> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let code = if data[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        starts.push((i, i + code));
        i += code;
        if starts.len() > 65536 {
            return Err(Failure::DecodeFailed);
        }
    }
    if starts.first().map(|p| p.0) != Some(0) {
        return Err(Failure::DecodeFailed);
    }
    let mut sets = [false; 3];
    let mut vcl = false;
    for (index, &(_, start)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(data.len(), |p| p.0);
        if end < start + 3 || data[start] & 0x80 != 0 || data[start + 1] & 7 == 0 {
            return Err(Failure::DecodeFailed);
        }
        let ty = (data[start] >> 1) & 63;
        if (32..=34).contains(&ty) {
            sets[(ty - 32) as usize] = true
        }
        if ty <= 31 {
            vcl = true
        }
    }
    if !vcl || (require_sets && sets != [true; 3]) {
        return Err(Failure::DecodeFailed);
    }
    Ok(())
}
#[derive(serde::Deserialize)]
struct Manifest {
    version: String,
    #[serde(rename = "sourceSha256")]
    source_sha: String,
    #[serde(rename = "binarySha256")]
    binary_sha: String,
}
fn bundled_codec() -> Result<PathBuf, Failure> {
    let dir = std::env::current_exe()
        .map_err(|_| Failure::TranscodeFailed)?
        .parent()
        .ok_or(Failure::TranscodeFailed)?
        .join("image-codec");
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(dir.join("manifest.json")).map_err(|_| Failure::TranscodeFailed)?,
    )
    .map_err(|_| Failure::TranscodeFailed)?;
    if manifest.version != CODEC_VERSION || manifest.source_sha != SOURCE_SHA {
        return Err(Failure::TranscodeFailed);
    }
    let binary = dir.join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    if super::secure_fs::is_link(&binary).map_err(|_| Failure::TranscodeFailed)? {
        return Err(Failure::TranscodeFailed);
    }
    let bytes = std::fs::read(&binary).map_err(|_| Failure::TranscodeFailed)?;
    if format!("{:x}", Sha256::digest(bytes)) != manifest.binary_sha {
        return Err(Failure::TranscodeFailed);
    }
    Ok(binary)
}
fn transcode(stream: &[u8], deadline: Instant) -> Result<Vec<u8>, Failure> {
    if Instant::now() >= deadline {
        return Err(Failure::Timeout);
    }
    let binary = bundled_codec()?;
    let mut command = Command::new(binary);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "quiet",
            "-max_alloc",
            "100000000",
            "-threads",
            "1",
            "-max_pixels",
            "20000000",
            "-f",
            "hevc",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-c:v",
            "bmp",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|_| Failure::TranscodeFailed)?;
    let mut stdin = child.stdin.take().ok_or(Failure::TranscodeFailed)?;
    let stdout = child.stdout.take().ok_or(Failure::TranscodeFailed)?;
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(stream));
        let reader = scope.spawn(move || {
            let mut b = Vec::new();
            stdout
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut b)
                .map(|_| b)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Ok(s),
                Err(_) => break Err(Failure::TranscodeFailed),
                _ => {}
            }
            if Instant::now() >= deadline {
                break Err(Failure::Timeout);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if status.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let written = writer.join().map_err(|_| Failure::TranscodeFailed)?;
        let bytes = reader
            .join()
            .map_err(|_| Failure::TranscodeFailed)?
            .map_err(|_| Failure::TranscodeFailed)?;
        if !status?.success() || written.is_err() || bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Failure::TranscodeFailed);
        }
        Ok(bytes)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn stream() -> Vec<u8> {
        [32u8, 33, 34, 19]
            .into_iter()
            .flat_map(|t| [0, 0, 0, 1, t << 1, 1, 0x80])
            .collect()
    }
    fn wrap(stream: &[u8]) -> Vec<u8> {
        let mut v = b"wxgf\x0f\0\0\0\0\0\0\0\0\0\0".to_vec();
        v.extend_from_slice(&(stream.len() as u32).to_be_bytes());
        v.extend_from_slice(stream);
        v
    }
    #[test]
    fn wxgf_uses_bounded_partitions_and_validates_all_nalus() {
        let stream = stream();
        let w = wrap(&stream);
        assert_eq!(partitions(&w).unwrap(), vec![stream.as_slice()]);
        let mut bad = w.clone();
        bad.pop();
        assert_eq!(partitions(&bad).unwrap_err(), Failure::ResourceIncomplete);
        let mut bad = w.clone();
        bad.extend_from_slice(&[0, 0, 0, 0]);
        assert!(partitions(&bad).is_err());
        let mut bad = stream.clone();
        bad[4] = 0xff;
        assert_eq!(partitions(&wrap(&bad)).unwrap_err(), Failure::DecodeFailed);
    }
    #[test]
    fn wxgf_rejects_missing_parameter_sets_and_fake_start_codes() {
        assert!(partitions(&wrap(&[0, 0, 1, 38, 1, 0x80])).is_err());
        assert!(partitions(b"wxgfgarbage\0\0\0\x01").is_err());
    }
    #[test]
    fn expired_codec_budget_returns_timeout() {
        assert_eq!(
            standard(&wrap(&stream()), Instant::now() - Duration::from_secs(1)).err(),
            Some(Failure::Timeout)
        );
    }
}
