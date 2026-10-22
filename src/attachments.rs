//! Images and videos the user attaches to a message.
//!
//! Nemotron 3.5 Super VL takes RGB JPEG or PNG images (about 512×512 for simple
//! pictures, 1536×1024 for documents) and MP4 videos of one to two minutes.
//! Files are sent inline as base64 `data:` URLs; public links are passed as is.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use image::metadata::Orientation;
use image::{DynamicImage, GenericImageView as _, ImageDecoder as _, ImageFormat, RgbImage};
use serde_json::{Value, json};

/// Videos above this size are refused; hosted APIs reject far smaller requests anyway.
pub const MAX_VIDEO_BYTES: usize = 100 * 1024 * 1024;
/// Videos above this size get a warning.
const LARGE_VIDEO_BYTES: usize = 20 * 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm"];

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttachmentKind {
    Image,
    Video,
}

#[derive(Clone, Debug)]
pub enum Source {
    Data { mime: String, bytes: Arc<[u8]> },
    Url(String),
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: u64,
    pub name: String,
    pub kind: AttachmentKind,
    pub source: Source,
    /// Shown under the attachment, e.g. that an image was scaled down.
    pub note: Option<String>,
}

impl Attachment {
    pub fn from_bytes(name: &str, kind: AttachmentKind, mime: &str, bytes: Vec<u8>) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: name.to_owned(),
            kind,
            source: Source::Data {
                mime: mime.to_owned(),
                bytes: Arc::from(bytes),
            },
            note: None,
        }
    }

    pub fn from_url(url: &str, kind: AttachmentKind) -> Self {
        let url = url.trim();
        let name = url
            .split(['?', '#'])
            .next()
            .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
            .filter(|s| !s.is_empty())
            .unwrap_or(url);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: name.to_owned(),
            kind,
            source: Source::Url(url.to_owned()),
            note: None,
        }
    }

    pub fn url(&self) -> String {
        match &self.source {
            Source::Url(url) => url.clone(),
            Source::Data { mime, bytes } => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                format!("data:{mime};base64,{b64}")
            }
        }
    }

    pub fn to_content_part(&self) -> Value {
        match self.kind {
            AttachmentKind::Image => {
                json!({ "type": "image_url", "image_url": { "url": self.url() } })
            }
            AttachmentKind::Video => {
                json!({ "type": "video_url", "video_url": { "url": self.url() } })
            }
        }
    }

    /// URI under which egui caches the decoded preview image.
    pub fn preview_uri(&self) -> String {
        format!("bytes://anhang-{}-{}", self.id, self.name)
    }

    pub fn image_bytes(&self) -> Option<Arc<[u8]>> {
        match (&self.source, self.kind) {
            (Source::Data { bytes, .. }, AttachmentKind::Image) => Some(bytes.clone()),
            _ => None,
        }
    }

    pub fn size_label(&self) -> String {
        match &self.source {
            Source::Data { bytes, .. } => format_size(bytes.len()),
            Source::Url(_) => "Link".to_owned(),
        }
    }
}

pub fn format_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn kind_for_extension(ext: &str) -> Option<AttachmentKind> {
    let ext = ext.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(AttachmentKind::Image)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(AttachmentKind::Video)
    } else {
        None
    }
}

/// Guesses from the link's file extension; anything unknown is treated as an image.
pub fn guess_kind_from_url(url: &str) -> AttachmentKind {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('.')
        .next()
        .and_then(kind_for_extension)
        .unwrap_or(AttachmentKind::Image)
}

pub fn is_supported_url(url: &str) -> bool {
    let url = url.trim();
    (url.starts_with("https://") || url.starts_with("http://")) && url.len() > "https://".len()
}

/// Extensions for the file picker.
pub fn picker_extensions() -> Vec<&'static str> {
    IMAGE_EXTENSIONS
        .iter()
        .chain(VIDEO_EXTENSIONS)
        .copied()
        .collect()
}

/// Reads a file from disk and prepares it for sending.
pub fn load_file(path: &Path, max_image_dim: u32) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match (ext.as_str(), kind_for_extension(&ext)) {
        (_, Some(kind)) => kind,
        ("pdf", _) => {
            return Err(format!(
                "{name}: PDFs nimmt das Modell nicht direkt an. Exportiere die Seiten als PNG und hänge die Bilder an."
            ));
        }
        ("heic" | "heif", _) => {
            return Err(format!(
                "{name}: HEIC wird nicht unterstützt. Bitte als JPEG oder PNG exportieren."
            ));
        }
        _ => {
            return Err(format!(
                "{name}: Dateityp nicht unterstützt. Erlaubt sind Bilder ({}) und Videos ({}).",
                IMAGE_EXTENSIONS.join(", "),
                VIDEO_EXTENSIONS.join(", ")
            ));
        }
    };
    let bytes =
        std::fs::read(path).map_err(|e| format!("{name}: konnte nicht gelesen werden ({e})."))?;
    match kind {
        AttachmentKind::Image => image_attachment(&name, &bytes, max_image_dim),
        AttachmentKind::Video => video_attachment(&name, &ext, bytes),
    }
}

pub fn image_attachment(name: &str, bytes: &[u8], max_dim: u32) -> Result<Attachment, String> {
    let prepared = prepare_image(bytes, max_dim).map_err(|e| format!("{name}: {e}"))?;
    let mut att =
        Attachment::from_bytes(name, AttachmentKind::Image, prepared.mime, prepared.bytes);
    att.note = prepared.note;
    Ok(att)
}

pub fn video_attachment(name: &str, ext: &str, bytes: Vec<u8>) -> Result<Attachment, String> {
    if bytes.len() > MAX_VIDEO_BYTES {
        return Err(format!(
            "{name}: Video ist zu groß ({}). Maximal {} – besser einen kurzen Clip (1–2 Minuten) oder einen öffentlichen Link verwenden.",
            format_size(bytes.len()),
            format_size(MAX_VIDEO_BYTES)
        ));
    }
    let mime = match ext {
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => "video/mp4",
    };
    let large = bytes.len() > LARGE_VIDEO_BYTES;
    let mut att = Attachment::from_bytes(name, AttachmentKind::Video, mime, bytes);
    att.note = match (ext, large) {
        (_, true) => Some(
            "Großes Video – der Upload kann dauern oder vom Server abgelehnt werden.".to_owned(),
        ),
        ("mp4" | "m4v", false) => None,
        _ => Some("MP4 wird empfohlen.".to_owned()),
    };
    Ok(att)
}

pub struct PreparedImage {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub note: Option<String>,
}

/// Converts any supported image into RGB PNG or JPEG and scales it down so the
/// longer side is at most `max_dim` pixels (0 disables scaling).
///
/// PNG and JPEG files that are already small enough and have no transparency
/// are passed through untouched.
pub fn prepare_image(bytes: &[u8], max_dim: u32) -> Result<PreparedImage, String> {
    let format = image::guess_format(bytes).map_err(|_| "kein lesbares Bild.".to_owned())?;
    let unreadable = |e: image::ImageError| format!("Bild konnte nicht gelesen werden ({e}).");
    let mut decoder = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(unreadable)?;
    // Phone photos are often stored sideways with an EXIF rotation tag.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    let rotated = orientation != Orientation::NoTransforms;
    let (w, h) = img.dimensions();
    let too_big = max_dim > 0 && w.max(h) > max_dim;
    let has_alpha = img.color().has_alpha();

    let passthrough_mime = match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        _ => None,
    };
    if let Some(mime) = passthrough_mime
        && !too_big
        && !has_alpha
        && !rotated
    {
        return Ok(PreparedImage {
            bytes: bytes.to_vec(),
            mime,
            note: None,
        });
    }

    let img = if too_big {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let rgb = flatten_onto_white(&img);
    // Photos stay JPEG; screenshots, graphics and documents keep sharp edges as PNG.
    let as_jpeg = matches!(format, ImageFormat::Jpeg | ImageFormat::WebP);
    let mut out = Cursor::new(Vec::new());
    if as_jpeg {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&rgb)
            .map_err(|e| format!("Bild konnte nicht umgewandelt werden ({e})."))?;
    } else {
        rgb.write_to(&mut out, ImageFormat::Png)
            .map_err(|e| format!("Bild konnte nicht umgewandelt werden ({e})."))?;
    }
    let note = too_big.then(|| {
        format!(
            "verkleinert von {w}×{h} auf {}×{}",
            rgb.width(),
            rgb.height()
        )
    });
    Ok(PreparedImage {
        bytes: out.into_inner(),
        mime: if as_jpeg { "image/jpeg" } else { "image/png" },
        note,
    })
}

/// The model expects RGB; transparent pixels would otherwise turn black.
fn flatten_onto_white(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let a = u16::from(a);
        let blend = |c: u8| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn encode(img: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn small_png_passes_through_unchanged() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(100, 50, image::Rgb([10, 20, 30])));
        let bytes = encode(&img, ImageFormat::Png);
        let p = prepare_image(&bytes, 1536).unwrap();
        assert_eq!(p.bytes, bytes);
        assert_eq!(p.mime, "image/png");
        assert!(p.note.is_none());
    }

    #[test]
    fn large_jpeg_is_scaled_down_keeping_aspect_ratio() {
        let img =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(4000, 2000, image::Rgb([200, 100, 50])));
        let bytes = encode(&img, ImageFormat::Jpeg);
        let p = prepare_image(&bytes, 1536).unwrap();
        assert_eq!(p.mime, "image/jpeg");
        let out = image::load_from_memory(&p.bytes).unwrap();
        assert_eq!(out.dimensions(), (1536, 768));
        assert_eq!(
            p.note.as_deref(),
            Some("verkleinert von 4000×2000 auf 1536×768")
        );
    }

    #[test]
    fn transparency_is_flattened_onto_white() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0])));
        let p = prepare_image(&encode(&img, ImageFormat::Png), 1536).unwrap();
        assert_eq!(p.mime, "image/png");
        let out = image::load_from_memory(&p.bytes).unwrap();
        assert!(!out.color().has_alpha());
        assert_eq!(out.to_rgb8().get_pixel(3, 3).0, [255, 255, 255]);
    }

    #[test]
    fn other_formats_are_converted_to_png() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(20, 20, image::Rgb([1, 2, 3])));
        let p = prepare_image(&encode(&img, ImageFormat::Bmp), 0).unwrap();
        assert_eq!(p.mime, "image/png");
        assert_eq!(image::guess_format(&p.bytes).unwrap(), ImageFormat::Png);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(prepare_image(b"definitely not an image", 1536).is_err());
    }

    #[test]
    fn file_types_are_checked() {
        let dir = std::env::temp_dir();
        assert!(
            load_file(&dir.join("bericht.pdf"), 1536)
                .unwrap_err()
                .contains("PDF")
        );
        assert!(
            load_file(&dir.join("notiz.txt"), 1536)
                .unwrap_err()
                .contains("nicht unterstützt")
        );
        assert_eq!(kind_for_extension("MP4"), Some(AttachmentKind::Video));
        assert_eq!(kind_for_extension("JpG"), Some(AttachmentKind::Image));
    }

    #[test]
    fn video_file_is_loaded_as_data_url() {
        let path =
            std::env::temp_dir().join(format!("fadis-nemotron-test-{}.mp4", std::process::id()));
        std::fs::write(&path, b"\0\0\0\x18ftypmp42").unwrap();
        let att = load_file(&path, 1536).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(att.kind, AttachmentKind::Video);
        assert!(att.url().starts_with("data:video/mp4;base64,"));
        assert_eq!(att.to_content_part()["type"], "video_url");
    }

    #[test]
    fn urls_are_named_and_classified() {
        let a = Attachment::from_url(
            " https://example.com/clips/demo.MP4?x=1 ",
            guess_kind_from_url("https://example.com/clips/demo.MP4?x=1"),
        );
        assert_eq!(a.name, "demo.MP4");
        assert_eq!(a.kind, AttachmentKind::Video);
        assert_eq!(a.url(), "https://example.com/clips/demo.MP4?x=1");
        assert_eq!(
            guess_kind_from_url("https://example.com/chart"),
            AttachmentKind::Image
        );
        assert!(is_supported_url("https://example.com/a.png"));
        assert!(!is_supported_url("file:///etc/passwd"));
        assert!(!is_supported_url("https://"));
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2 KB");
        assert_eq!(format_size(5 * 1024 * 1024 + 100_000), "5.1 MB");
    }
}
