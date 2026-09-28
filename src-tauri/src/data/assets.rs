//! 자산 생성은 binary 게시 후 metadata 게시로 확정한다. 문서 transaction과 별도 수명이다.
//! 완료 전 중단된 디렉터리는 참조로 반환하지 않으며 자동 삭제/GC하지 않는다.
use crate::data::{
    edit_recovery::{
        error::{Category, RecoveryError, Stage},
        model::digest,
        native,
    },
    project_file::directory::ProjectDirectory,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{BufReader, Cursor, Read, Write},
    path::{Path, PathBuf},
};
#[cfg(test)]
mod tests;

pub(crate) const MAX_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_PIXELS: u64 = 16_000_000;
pub(crate) const MAX_ANIMATION_FRAMES: u64 = 240;
pub(crate) const MAX_ANIMATION_PIXELS: u64 = 120_000_000;
pub(crate) const MAX_ANIMATION_DECODE_BYTES: u64 = 480 * 1024 * 1024;
const MAX_IMAGE_DECODE_BYTES: u64 = 128 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Metadata {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) image: bool,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
}
pub(crate) struct Store {
    path: PathBuf,
    _guards: Vec<ProjectDirectory>,
}
fn invalid() -> RecoveryError {
    RecoveryError::new(Category::Corrupt, Stage::Validate)
}
fn io(e: std::io::Error) -> RecoveryError {
    RecoveryError::io(Stage::Read, e)
}
impl Store {
    pub(crate) fn open(root: &Path, create: bool) -> Result<Self, RecoveryError> {
        let parent = ProjectDirectory::open_root(root).map_err(io)?;
        let path = root.join("assets");
        if create {
            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(io(e)),
            }
        }
        let guard = ProjectDirectory::open_root(&path).map_err(io)?;
        Ok(Self {
            path,
            _guards: vec![parent, guard],
        })
    }

    /// 복구 저장소와 휴지통처럼 이미 검증된 asset-id flat namespace를 연다.
    /// caller가 프로젝트 binding을 별도로 소유한 내부 경로에서만 사용한다.
    pub(crate) fn open_namespace(path: &Path) -> Result<Self, RecoveryError> {
        let guard = ProjectDirectory::open_root(path).map_err(io)?;
        Ok(Self {
            path: path.to_owned(),
            _guards: vec![guard],
        })
    }
    fn directory(
        &self,
        id: &str,
        create: bool,
    ) -> Result<(PathBuf, ProjectDirectory), RecoveryError> {
        if !super::media::valid_id(id) {
            return Err(invalid());
        }
        for guard in &self._guards {
            guard.validate().map_err(io)?;
        }
        let path = self.path.join(id);
        if create {
            fs::create_dir(&path).map_err(|e| RecoveryError::io(Stage::Create, e))?;
        }
        let guard = ProjectDirectory::open_root(&path).map_err(io)?;
        Ok((path, guard))
    }
    pub(crate) fn read(&self, id: &str) -> Result<(Metadata, Vec<u8>), RecoveryError> {
        let (path, guard) = self.directory(id, false)?;
        self.read_guarded(id, &path, &guard)
    }

    /// 유지보수 이동 전용: 검증한 package directory handle을 먼저 잡고 그 handle 아래
    /// metadata/content를 읽는다. caller는 반환한 handle 자체만 이동 권한으로 사용한다.
    pub(crate) fn read_owned(
        &self,
        id: &str,
    ) -> Result<(Metadata, Vec<u8>, ProjectDirectory), RecoveryError> {
        if !super::media::valid_id(id) {
            return Err(invalid());
        }
        for guard in &self._guards {
            guard.validate().map_err(io)?;
        }
        let path = self.path.join(id);
        let guard = ProjectDirectory::open_owned_root(&path).map_err(io)?;
        let (metadata, bytes) = self.read_guarded(id, &path, &guard)?;
        Ok((metadata, bytes, guard))
    }

    fn read_guarded(
        &self,
        id: &str,
        path: &Path,
        guard: &ProjectDirectory,
    ) -> Result<(Metadata, Vec<u8>), RecoveryError> {
        let meta = read(path, guard, "metadata.json", 8192)?;
        // strict duplicate-key 검사도 적용한다.
        super::json::parse_strict_json_object(&meta)
            .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Read, e))?;
        let meta: Metadata = serde_json::from_slice(&meta)
            .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Read, e))?;
        if meta.schema_version != 1
            || meta.id != id
            || !safe_name(&meta.name)
            || meta.size > MAX_BYTES as u64
            || (!meta.image && (meta.width.is_some() || meta.height.is_some()))
        {
            return Err(invalid());
        }
        let bytes = read(path, guard, &filename(&meta), MAX_BYTES)?;
        if bytes.len() as u64 != meta.size || digest(&bytes) != meta.sha256 {
            return Err(RecoveryError::new(Category::DigestMismatch, Stage::Read));
        }
        if meta.image {
            let (w, h) = image_info(&meta.name, &bytes)?.dimensions;
            if meta.width != Some(w) || meta.height != Some(h) {
                return Err(invalid());
            }
        }
        Ok((meta, bytes))
    }
    pub(crate) fn import(
        &self,
        source: &Path,
        image: bool,
        id: &str,
    ) -> Result<Metadata, RecoveryError> {
        // 선택된 원본 handle을 읽기 전 검증하고 복사만 한다. 원본 경로는 metadata에 넣지 않는다.
        let parent = source.parent().ok_or_else(invalid)?;
        let parent = ProjectDirectory::open_root(parent).map_err(io)?;
        let name = source
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| safe_name(s))
            .ok_or_else(invalid)?;
        let mut file = native::open(source, false, false).map_err(io)?;
        parent.validate_file(&file, name).map_err(io)?;
        let limit = if image { MAX_IMAGE_BYTES } else { MAX_BYTES };
        if file.metadata().map_err(io)?.len() > limit as u64 {
            return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > limit {
            return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
        }
        let dimensions = image
            .then(|| image_info(name, &bytes).map(|info| info.dimensions))
            .transpose()?;
        let name = self.unique_name(name)?;
        let meta = Metadata {
            schema_version: 1,
            id: id.into(),
            name,
            size: bytes.len() as u64,
            sha256: digest(&bytes),
            image,
            width: dimensions.map(|d| d.0),
            height: dimensions.map(|d| d.1),
        };
        self.put(&meta, &bytes)?;
        Ok(meta)
    }
    pub(crate) fn put(&self, meta: &Metadata, bytes: &[u8]) -> Result<(), RecoveryError> {
        if bytes.len() > MAX_BYTES
            || meta.schema_version != 1
            || !super::media::valid_id(&meta.id)
            || meta.size != bytes.len() as u64
            || digest(bytes) != meta.sha256
            || !safe_name(&meta.name)
            || (!meta.image && (meta.width.is_some() || meta.height.is_some()))
        {
            return Err(invalid());
        }
        if meta.image {
            let (w, h) = image_info(&meta.name, bytes)?.dimensions;
            if meta.width != Some(w) || meta.height != Some(h) {
                return Err(invalid());
            }
        }
        // 같은 ID의 재시도는 완성본이 정확히 일치할 때만 성공한다. 다른 자산 덮어쓰기 금지.
        match self.directory(&meta.id, true) {
            Ok((path, guard)) => {
                publish(&path, &guard, &filename(meta), bytes)?;
                let json = serde_json::to_vec(meta)
                    .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Write, e))?;
                publish(&path, &guard, "metadata.json", &json)?;
            }
            Err(e)
                if e.source
                    .as_ref()
                    .is_some_and(|s| s.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                // 중단 뒤 content만 게시된 경우, hash가 같은 것만 다시 사용하고 metadata를 완성한다.
                let (path, guard) = self.directory(&meta.id, false)?;
                match read(&path, &guard, &filename(meta), MAX_BYTES) {
                    Ok(stored) if stored == bytes => (),
                    Ok(_) => return Err(invalid()),
                    Err(e)
                        if e.source
                            .as_ref()
                            .is_some_and(|s| s.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        publish(&path, &guard, &filename(meta), bytes)?
                    }
                    Err(e) => return Err(e),
                }
                match read(&path, &guard, "metadata.json", 8192) {
                    Ok(_) => (),
                    Err(e)
                        if e.source
                            .as_ref()
                            .is_some_and(|s| s.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        publish(
                            &path,
                            &guard,
                            "metadata.json",
                            &serde_json::to_vec(meta).map_err(|_| invalid())?,
                        )?
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
        let (actual, stored) = self.read(&meta.id)?;
        if &actual != meta || stored != bytes {
            return Err(invalid());
        }
        Ok(())
    }
    fn unique_name(&self, name: &str) -> Result<String, RecoveryError> {
        let mut names = BTreeSet::new();
        for (count, entry) in self._guards[1].read_dir().map_err(io)?.enumerate() {
            if count >= 100_000 {
                return Err(RecoveryError::new(Category::TooLarge, Stage::List));
            }
            let entry = entry.map_err(io)?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if super::media::valid_id(&id) {
                let (path, guard) = self.directory(&id, false)?;
                match read(&path, &guard, "metadata.json", 8192) {
                    Ok(b) => {
                        let m: Metadata = serde_json::from_slice(&b).map_err(|_| invalid())?;
                        names.insert(m.name.to_lowercase());
                    }
                    Err(e)
                        if e.source
                            .as_ref()
                            .is_some_and(|s| s.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        ()
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        if !names.contains(&name.to_lowercase()) {
            return Ok(name.into());
        }
        let p = Path::new(name);
        let stem = p.file_stem().and_then(|s| s.to_str()).ok_or_else(invalid)?;
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        for n in 2..=100_001 {
            let candidate = format!("{stem} ({n}){ext}");
            if !names.contains(&candidate.to_lowercase()) && safe_name(&candidate) {
                return Ok(candidate);
            }
        }
        Err(invalid())
    }
}
pub(crate) fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && !name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        && !name.ends_with(['.', ' '])
        && name != "."
        && name != ".."
}
fn read(
    path: &Path,
    guard: &ProjectDirectory,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, RecoveryError> {
    let mut file = native::open(&path.join(name), false, false).map_err(io)?;
    guard.validate_file(&file, name).map_err(io)?;
    if file.metadata().map_err(io)?.len() > limit as u64 {
        return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > limit {
        return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
    }
    Ok(bytes)
}
fn publish(
    path: &Path,
    guard: &ProjectDirectory,
    name: &str,
    bytes: &[u8],
) -> Result<(), RecoveryError> {
    let temp = format!(".{}.tmp", uuid::Uuid::new_v4());
    let mut file = native::open(&path.join(&temp), true, true)
        .map_err(|e| RecoveryError::io(Stage::Create, e))?;
    let result = (|| {
        guard.validate_file(&file, &temp).map_err(io)?;
        file.write_all(bytes)
            .map_err(|e| RecoveryError::io(Stage::Write, e))?;
        file.sync_all()
            .map_err(|e| RecoveryError::io(Stage::Sync, e))?;
        native::publish(&file, name).map_err(|e| RecoveryError::io(Stage::Publish, e))
    })();
    if let Err(mut e) = result {
        e.cleanup = native::cleanup(&file).err();
        return Err(e);
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageKind {
    Png,
    Jpeg,
    WebP,
    Gif,
    Bmp,
}

impl ImageKind {
    fn format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::WebP => image::ImageFormat::WebP,
            Self::Gif => image::ImageFormat::Gif,
            Self::Bmp => image::ImageFormat::Bmp,
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImageInfo {
    dimensions: (u32, u32),
    kind: ImageKind,
}

fn image_kind_from_name(name: &str) -> Option<ImageKind> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" | "apng" => Some(ImageKind::Png),
        "jpg" | "jpeg" | "jpe" | "jfif" => Some(ImageKind::Jpeg),
        "webp" => Some(ImageKind::WebP),
        "gif" => Some(ImageKind::Gif),
        "bmp" => Some(ImageKind::Bmp),
        _ => None,
    }
}

fn image_kind_from_bytes(bytes: &[u8]) -> Result<ImageKind, RecoveryError> {
    match image::guess_format(bytes).map_err(image_error)? {
        image::ImageFormat::Png => Ok(ImageKind::Png),
        image::ImageFormat::Jpeg => Ok(ImageKind::Jpeg),
        image::ImageFormat::WebP => Ok(ImageKind::WebP),
        image::ImageFormat::Gif => Ok(ImageKind::Gif),
        image::ImageFormat::Bmp => Ok(ImageKind::Bmp),
        _ => Err(RecoveryError::new(
            Category::UnsupportedKind,
            Stage::Validate,
        )),
    }
}

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    // image의 allocation 제한은 decoder에 따라 best-effort이므로 아래 픽셀·프레임
    // 누적 제한도 함께 검사한다.
    limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
    limits
}

fn image_error(error: image::ImageError) -> RecoveryError {
    let category = match error {
        image::ImageError::Limits(_) => Category::TooLarge,
        image::ImageError::Unsupported(_) => Category::UnsupportedKind,
        _ => Category::Corrupt,
    };
    RecoveryError::caused(category, Stage::Validate, error)
}

fn validate_dimensions(dimensions: (u32, u32)) -> Result<(), RecoveryError> {
    let (width, height) = dimensions;
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(RecoveryError::new(Category::TooLarge, Stage::Validate));
    }
    Ok(())
}

fn validate_frames(
    mut frames: image::Frames<'_>,
    dimensions: (u32, u32),
) -> Result<(), RecoveryError> {
    let frame_pixels = u64::from(dimensions.0) * u64::from(dimensions.1);
    let mut count = 0_u64;
    let mut decoded_pixels = 0_u64;
    let mut decoded_bytes = 0_u64;
    while let Some(frame) = frames.next() {
        let frame = frame.map_err(image_error)?;
        count += 1;
        decoded_pixels = decoded_pixels
            .checked_add(frame_pixels)
            .ok_or_else(|| RecoveryError::new(Category::TooLarge, Stage::Validate))?;
        decoded_bytes = decoded_bytes
            .checked_add(frame.buffer().as_raw().len() as u64)
            .ok_or_else(|| RecoveryError::new(Category::TooLarge, Stage::Validate))?;
        if count > MAX_ANIMATION_FRAMES
            || decoded_pixels > MAX_ANIMATION_PIXELS
            || decoded_bytes > MAX_ANIMATION_DECODE_BYTES
        {
            return Err(RecoveryError::new(Category::TooLarge, Stage::Validate));
        }
    }
    if count == 0 {
        return Err(invalid());
    }
    Ok(())
}

fn decode_static(bytes: &[u8], kind: ImageKind) -> Result<image::DynamicImage, RecoveryError> {
    let mut reader =
        image::ImageReader::with_format(BufReader::new(Cursor::new(bytes)), kind.format());
    reader.limits(decode_limits());
    reader.decode().map_err(image_error)
}

fn image_info(name: &str, bytes: &[u8]) -> Result<ImageInfo, RecoveryError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(RecoveryError::new(Category::TooLarge, Stage::Validate));
    }
    let extension_kind = image_kind_from_name(name)
        .ok_or_else(|| RecoveryError::new(Category::UnsupportedKind, Stage::Validate))?;
    let kind = image_kind_from_bytes(bytes)?;
    // 확장자/MIME 추측보다 실제 signature를 우선하되, 지원 형식끼리의 위장도 받지 않는다.
    if extension_kind != kind {
        return Err(RecoveryError::new(
            Category::UnsupportedKind,
            Stage::Validate,
        ));
    }

    use image::{AnimationDecoder, GenericImageView, ImageDecoder};
    let cursor = || BufReader::new(Cursor::new(bytes));
    let dimensions = match kind {
        ImageKind::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(cursor()).map_err(image_error)?;
            decoder.set_limits(decode_limits()).map_err(image_error)?;
            let dimensions = decoder.dimensions();
            validate_dimensions(dimensions)?;
            validate_frames(decoder.into_frames(), dimensions)?;
            dimensions
        }
        ImageKind::WebP => {
            let mut decoder =
                image::codecs::webp::WebPDecoder::new(cursor()).map_err(image_error)?;
            decoder.set_limits(decode_limits()).map_err(image_error)?;
            let dimensions = decoder.dimensions();
            validate_dimensions(dimensions)?;
            if decoder.has_animation() {
                validate_frames(decoder.into_frames(), dimensions)?;
            } else {
                let decoded = decode_static(bytes, kind)?;
                if decoded.dimensions() != dimensions {
                    return Err(invalid());
                }
            }
            dimensions
        }
        ImageKind::Png => {
            let decoder = image::codecs::png::PngDecoder::with_limits(cursor(), decode_limits())
                .map_err(image_error)?;
            let dimensions = decoder.dimensions();
            validate_dimensions(dimensions)?;
            if decoder.is_apng().map_err(image_error)? {
                validate_frames(
                    decoder.apng().map_err(image_error)?.into_frames(),
                    dimensions,
                )?;
            } else {
                let decoded = decode_static(bytes, kind)?;
                if decoded.dimensions() != dimensions {
                    return Err(invalid());
                }
            }
            dimensions
        }
        ImageKind::Jpeg | ImageKind::Bmp => {
            // 압축 본문을 풀기 전에 header의 총 픽셀 상한부터 적용한다.
            let dimensions = match kind {
                ImageKind::Jpeg => image::codecs::jpeg::JpegDecoder::new(cursor())
                    .map_err(image_error)?
                    .dimensions(),
                ImageKind::Bmp => image::codecs::bmp::BmpDecoder::new(cursor())
                    .map_err(image_error)?
                    .dimensions(),
                _ => unreachable!(),
            };
            validate_dimensions(dimensions)?;
            let decoded = decode_static(bytes, kind)?;
            if decoded.dimensions() != dimensions {
                return Err(invalid());
            }
            dimensions
        }
    };
    Ok(ImageInfo { dimensions, kind })
}

pub(crate) fn image_content_type(name: &str) -> Option<&'static str> {
    image_kind_from_name(name).map(ImageKind::content_type)
}

/// 보이는 필드뿐 아니라 원본/archived/unknown 하위 객체의 명시적 자산 참조도 보수적으로 수집한다.
pub(crate) fn references(value: &serde_json::Value) -> Result<BTreeSet<String>, RecoveryError> {
    fn walk(v: &serde_json::Value, ids: &mut BTreeSet<String>) -> Result<(), RecoveryError> {
        match v {
            serde_json::Value::Object(m) => {
                if matches!(
                    m.get("kind").and_then(|v| v.as_str()),
                    Some("image" | "file")
                ) {
                    if let Some(values) = m.get("value").and_then(|v| v.as_array()) {
                        for v in values {
                            let id = v
                                .as_str()
                                .filter(|s| super::media::valid_id(s))
                                .ok_or_else(invalid)?;
                            ids.insert(id.into());
                        }
                    }
                }
                for v in m.values() {
                    walk(v, ids)?;
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    walk(v, ids)?;
                }
            }
            _ => (),
        }
        if ids.len() > 512 {
            return Err(RecoveryError::new(Category::TooLarge, Stage::Validate));
        }
        Ok(())
    }
    let mut ids = BTreeSet::new();
    walk(value, &mut ids)?;
    Ok(ids)
}

pub(crate) fn filename(meta: &Metadata) -> String {
    let extension = Path::new(&meta.name)
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or("bin")
        .to_ascii_lowercase();
    format!("content.{extension}")
}

pub(crate) fn validate_document(root: &Path, bytes: &[u8]) -> Result<(), RecoveryError> {
    let raw = super::json::parse_strict_json_object(bytes)
        .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
    let ids = references(&raw)?;
    if ids.is_empty() {
        return Ok(());
    }
    let store = Store::open(root, false)?;
    for id in ids {
        store.read(&id)?;
    }
    store.validate_image_references(&raw)?;
    Ok(())
}
impl Store {
    pub(crate) fn validate_image_references(
        &self,
        raw: &serde_json::Value,
    ) -> Result<(), RecoveryError> {
        match raw {
            serde_json::Value::Object(m) => {
                if m.get("kind").and_then(|v| v.as_str()) == Some("image") {
                    if let Some(ids) = m.get("value").and_then(|v| v.as_array()) {
                        for id in ids {
                            if !self.read(id.as_str().ok_or_else(invalid)?)?.0.image {
                                return Err(invalid());
                            }
                        }
                    }
                }
                for v in m.values() {
                    self.validate_image_references(v)?;
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    self.validate_image_references(v)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
}
