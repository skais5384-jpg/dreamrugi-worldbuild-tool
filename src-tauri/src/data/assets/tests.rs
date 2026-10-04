use super::*;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("worldbuild-assets-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::create_dir(p.join("project")).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn png() -> Vec<u8> {
    let mut bytes = vec![];
    {
        let mut e = png::Encoder::new(&mut bytes, 2, 1);
        e.set_color(png::ColorType::Rgb);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&[255, 0, 0, 0, 255, 0]).unwrap();
    }
    bytes
}
#[test]
fn asset_copy_collision_source_independence_and_corruption_preserve_original() {
    let f = Fixture::new();
    let source = f.0.join("sample.png");
    let original = png();
    fs::write(&source, &original).unwrap();
    let store = Store::open(&f.0.join("project"), true).unwrap();
    let a = store.import(&source, true, &id()).unwrap();
    let b = store.import(&source, true, &id()).unwrap();
    assert_eq!(a.name, "sample.png");
    assert_eq!(b.name, "sample (2).png");
    assert_eq!(a.width, Some(2));
    assert_eq!(fs::read(&source).unwrap(), original);
    fs::rename(&source, f.0.join("moved.png")).unwrap();
    assert_eq!(store.read(&a.id).unwrap().1, original);
    assert_eq!(store.read(&b.id).unwrap().1, original);
    fs::write(
        f.0.join("project/assets").join(&a.id).join("content.png"),
        b"bad",
    )
    .unwrap();
    assert_eq!(
        store.read(&a.id).unwrap_err().category,
        Category::DigestMismatch
    );
    assert_eq!(store.read(&b.id).unwrap().1, original);
    assert_eq!(fs::read(f.0.join("moved.png")).unwrap(), original);
}
#[test]
fn asset_partial_units_resume_without_overwrite_and_mismatching_id_is_rejected() {
    let f = Fixture::new();
    let store = Store::open(&f.0.join("project"), true).unwrap();
    let bytes = b"retained bytes";
    let meta = Metadata {
        schema_version: 1,
        id: id(),
        name: "notes.txt".into(),
        size: bytes.len() as u64,
        sha256: digest(bytes),
        image: false,
        width: None,
        height: None,
    };
    let path = f.0.join("project/assets").join(&meta.id);
    fs::create_dir(&path).unwrap();
    fs::write(path.join(".interrupted.tmp"), b"partial").unwrap();
    assert!(store.read(&meta.id).is_err());
    store.put(&meta, bytes).unwrap();
    store.put(&meta, bytes).unwrap();
    assert_eq!(
        store.read(&meta.id).unwrap(),
        (meta.clone(), bytes.to_vec())
    );
    let mut different = meta.clone();
    different.name = "other.txt".into();
    assert!(store.put(&different, bytes).is_err());
    assert_eq!(store.read(&meta.id).unwrap().0, meta);
    assert_eq!(fs::read(path.join(".interrupted.tmp")).unwrap(), b"partial");
}
#[test]
fn asset_rejects_unsafe_aliases_active_images_and_size_limits() {
    let f = Fixture::new();
    let store = Store::open(&f.0.join("project"), true).unwrap();
    for bad in [
        "../escape",
        "C:\\secret",
        "\\\\server\\share",
        "00000000-0000-0000-0000-000000000000",
    ] {
        assert!(store.read(bad).is_err());
    }
    let source = f.0.join("active.png");
    fs::write(&source, b"<svg onload='bad()'/>").unwrap();
    assert!(store.import(&source, true, &id()).is_err());
    fs::write(&source, png()).unwrap();
    let link = f.0.join("alias.png");
    fs::hard_link(&source, &link).unwrap();
    assert!(store.import(&link, true, &id()).is_err());
    fs::remove_file(&link).unwrap();
    let large = fs::OpenOptions::new().write(true).open(&source).unwrap();
    large.set_len((MAX_BYTES + 1) as u64).unwrap();
    drop(large);
    assert_eq!(
        store.import(&source, false, &id()).unwrap_err().category,
        Category::TooLarge
    );
    assert_eq!(
        store.import(&source, true, &id()).unwrap_err().category,
        Category::TooLarge
    );
    let mut bytes = vec![];
    {
        let e = png::Encoder::new(&mut bytes, 8193, 1);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&vec![0; 8193]).unwrap();
    }
    assert_eq!(
        image_info("too-wide.png", &bytes).unwrap_err().category,
        Category::TooLarge
    );
}

#[test]
fn asset_png_pixel_limit_accepts_boundary_and_rejects_one_row_over() {
    for (width, height, accepted) in [(4000, 4000, true), (4000, 4001, false)] {
        let mut bytes = vec![];
        {
            let e = png::Encoder::new(&mut bytes, width, height);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&vec![0; (width * height) as usize])
                .unwrap();
        }
        let result = image_info("boundary.png", &bytes).map(|info| info.dimensions);
        if accepted {
            assert_eq!(result.unwrap(), (width, height));
        } else {
            assert_eq!(result.unwrap_err().category, Category::TooLarge);
        }
    }
}

#[test]
fn common_image_formats_are_decoded_and_extension_spoofing_is_rejected() {
    use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

    let rgb = [255, 0, 0, 0, 255, 0];
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .write_image(&rgb, 2, 1, ExtendedColorType::Rgb8)
        .unwrap();
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .write_image(&rgb, 2, 1, ExtendedColorType::Rgb8)
        .unwrap();
    let mut bmp = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bmp)
        .write_image(&rgb, 2, 1, ExtendedColorType::Rgb8)
        .unwrap();

    let frame = |red: u8| {
        image::Frame::from_parts(
            RgbaImage::from_pixel(2, 1, Rgba([red, 0, 0, 255])),
            0,
            0,
            image::Delay::from_numer_denom_ms(50, 1),
        )
    };
    let mut gif = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut gif)
        .encode_frames([frame(0), frame(255)])
        .unwrap();

    let mut apng = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut apng, 2, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_animated(2, 0).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&rgb).unwrap();
        writer.write_image_data(&[0, 0, 255, 255, 255, 0]).unwrap();
        writer.finish().unwrap();
    }

    for (name, bytes, kind) in [
        ("photo.JFIF", jpeg.as_slice(), ImageKind::Jpeg),
        ("photo.webp", webp.as_slice(), ImageKind::WebP),
        ("animation.GIF", gif.as_slice(), ImageKind::Gif),
        ("animation.apng", apng.as_slice(), ImageKind::Png),
        ("bitmap.bmp", bmp.as_slice(), ImageKind::Bmp),
    ] {
        let info = image_info(name, bytes).unwrap();
        assert_eq!(info.dimensions, (2, 1));
        assert_eq!(info.kind, kind);
    }
    assert_eq!(
        image_info("renamed.png", &jpeg).unwrap_err().category,
        Category::UnsupportedKind
    );
    assert_eq!(
        image_info("active.svg", &png()).unwrap_err().category,
        Category::UnsupportedKind
    );
}

#[test]
fn animation_frame_limit_is_enforced_while_streaming() {
    use image::{Rgba, RgbaImage};
    let frames = (0..=MAX_ANIMATION_FRAMES).map(|index| {
        image::Frame::from_parts(
            RgbaImage::from_pixel(1, 1, Rgba([index as u8, 0, 0, 255])),
            0,
            0,
            image::Delay::from_numer_denom_ms(10, 1),
        )
    });
    let mut gif = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut gif)
        .encode_frames(frames)
        .unwrap();
    assert_eq!(
        image_info("too-many.gif", &gif).unwrap_err().category,
        Category::TooLarge
    );
}

#[test]
fn jpeg_bmp_headers_enforce_pixels_before_corrupt_body_decode() {
    use image::{ExtendedColorType, ImageEncoder};
    for kind in [ImageKind::Jpeg, ImageKind::Bmp] {
        let mut bytes = Vec::new();
        match kind {
            ImageKind::Jpeg => image::codecs::jpeg::JpegEncoder::new(&mut bytes)
                .write_image(&[255, 0, 0, 0, 255, 0], 2, 1, ExtendedColorType::Rgb8)
                .unwrap(),
            ImageKind::Bmp => image::codecs::bmp::BmpEncoder::new(&mut bytes)
                .write_image(&[255, 0, 0, 0, 255, 0], 2, 1, ExtendedColorType::Rgb8)
                .unwrap(),
            _ => unreachable!(),
        }
        let name = if kind == ImageKind::Jpeg {
            "test.jpg"
        } else {
            "test.bmp"
        };
        assert_eq!(image_info(name, &bytes).unwrap().dimensions, (2, 1));
        let mut damaged = bytes.clone();
        if kind == ImageKind::Jpeg {
            let scan = damaged.windows(2).position(|p| p == [0xff, 0xda]).unwrap();
            let length = u16::from_be_bytes([damaged[scan + 2], damaged[scan + 3]]) as usize;
            damaged.truncate(scan + 2 + length);
            // image의 JPEG는 단순 EOF에 관대하다. 없는 entropy table을
            // 지시해 header는 읽히지만 본문 decode는 확실히 실패하게 한다.
            damaged[scan + 6] = 0x33;
        } else {
            let offset = u32::from_le_bytes(damaged[10..14].try_into().unwrap()) as usize;
            damaged.truncate(offset);
        }
        assert!(
            image_info(name, &damaged).is_err(),
            "small corrupt body must still decode"
        );
        if kind == ImageKind::Jpeg {
            let frame = damaged.windows(2).position(|p| p == [0xff, 0xc0]).unwrap();
            damaged[frame + 5..frame + 7].copy_from_slice(&4000_u16.to_be_bytes());
            damaged[frame + 7..frame + 9].copy_from_slice(&6000_u16.to_be_bytes());
        } else {
            damaged[18..22].copy_from_slice(&6000_u32.to_le_bytes());
            damaged[22..26].copy_from_slice(&4000_u32.to_le_bytes());
        }
        // 한 변은 허용 범위지만 총 24M 픽셀이다. 잘린 본문 오류보다 제품 상한이 먼저다.
        let error = image_info(name, &damaged).unwrap_err();
        assert_eq!(error.category, Category::TooLarge, "{name}");
        assert_eq!(error.stage, Stage::Validate);
    }
}

#[test]
fn missing_attachment_permission_preserves_kind_and_requires_new_attachment_bytes() {
    let fixture = Fixture::new();
    let root = fixture.0.join("project");
    let asset = id();
    let original = serde_json::json!({"fields":{"old":{"kind":"file","value":[asset]}}});
    let allowed = reference_kinds(&original).unwrap();
    assert!(validate_document_with_missing(
        &root,
        &serde_json::to_vec(&original).unwrap(),
        &allowed
    )
    .is_ok());
    let image = serde_json::json!({"fields":{"new":{"kind":"image","value":[asset]}}});
    assert!(
        validate_document_with_missing(&root, &serde_json::to_vec(&image).unwrap(), &allowed)
            .is_err()
    );
    let added = serde_json::json!({"fields":{"new":{"kind":"file","value":[id()]}}});
    assert!(
        validate_document_with_missing(&root, &serde_json::to_vec(&added).unwrap(), &allowed)
            .is_err()
    );
    let unsafe_path = serde_json::json!({"kind":"file","value":["../outside"]});
    assert!(validate_document_with_missing(
        &root,
        &serde_json::to_vec(&unsafe_path).unwrap(),
        &allowed
    )
    .is_err());
    let image_allowed = reference_kinds(&image).unwrap();
    assert!(validate_document_with_missing(
        &root,
        &serde_json::to_vec(&image).unwrap(),
        &image_allowed
    )
    .is_ok());
    assert!(validate_document_with_missing(
        &root,
        &serde_json::to_vec(&original).unwrap(),
        &image_allowed
    )
    .is_err());
}
