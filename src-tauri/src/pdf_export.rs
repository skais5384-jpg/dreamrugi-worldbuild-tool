//! A saved document is rendered in an isolated WebView2 surface, then published from an
//! owned, verified temporary file. This module never prints the interactive editor DOM.
use crate::commands::{
    document_workspace::{ReadField, Response},
    dto::FieldDto,
};
use crate::data::{
    edit_input::{Mark, RichNode, ValueDto},
    edit_recovery::model::Intent,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const MAX_HTML_BYTES: usize = 32 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) image: bool,
}

pub(crate) struct Snapshot {
    pub(crate) document: String,
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) read: Response,
    pub(crate) assets: BTreeMap<String, Option<Asset>>,
    pub(crate) links: BTreeMap<String, Option<String>>,
    pub(crate) missing_images: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Published {
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) cleanup_warning: bool,
}

#[cfg(windows)]
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
#[cfg(all(windows, debug_assertions))]
static PROBE_FAILED_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(all(windows, debug_assertions))]
fn probe_trace(stage: &str) {
    if let Some(folder) = std::env::var_os("WORLDBUILD_PDF_PROBE_DIR") {
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .open(PathBuf::from(folder).join("probe-trace.txt"))
        {
            let _ = writeln!(file, "{stage}");
        }
    }
}

#[cfg(any(not(windows), not(debug_assertions)))]
fn probe_trace(_: &str) {}

#[cfg(windows)]
pub(crate) fn install(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

#[cfg(not(windows))]
pub(crate) fn install<R: tauri::Runtime>(_: tauri::AppHandle<R>) {}

fn diagnostic_stage(stage: &'static str) {
    crate::diagnostic_log::record("pdf", stage, "pipeline", "reached", None, None);
}

fn diagnostic_error(stage: &'static str, error: &io::Error) {
    let category = match error.kind() {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::AlreadyExists => "already_exists",
        io::ErrorKind::Interrupted => "interrupted",
        io::ErrorKind::InvalidInput => "invalid_input",
        io::ErrorKind::InvalidData => "invalid_data",
        io::ErrorKind::TimedOut => "timed_out",
        _ => "other",
    };
    crate::diagnostic_log::record("pdf", stage, category, "failed", None, None);
}

fn escaped(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
    output
}

fn rich(node: &RichNode) -> String {
    fn children(nodes: &[RichNode]) -> String {
        nodes.iter().map(rich).collect::<String>()
    }
    match node {
        RichNode::Root { children: values } => children(values),
        RichNode::Paragraph { children: values } => format!("<p>{}</p>", children(values)),
        RichNode::Heading {
            level,
            children: values,
        } => {
            let level = (*level).clamp(1, 6);
            format!("<h{level}>{}</h{level}>", children(values))
        }
        RichNode::Text { text, marks } => {
            let mut value = escaped(text);
            for mark in marks {
                let tag = match mark {
                    Mark::Bold => "strong",
                    Mark::Italic => "em",
                    Mark::Underline => "u",
                    Mark::Strikethrough => "s",
                };
                value = format!("<{tag}>{value}</{tag}>");
            }
            value
        }
        RichNode::HardBreak {} => "<br>".into(),
        RichNode::Blockquote { children: values } => {
            format!("<blockquote>{}</blockquote>", children(values))
        }
        RichNode::BulletList { children: values } | RichNode::TaskList { children: values } => {
            format!("<ul>{}</ul>", children(values))
        }
        RichNode::OrderedList { children: values } => {
            format!("<ol>{}</ol>", children(values))
        }
        RichNode::ListItem { children: values } => {
            format!("<li>{}</li>", children(values))
        }
        RichNode::TaskItem {
            checked,
            children: values,
        } => format!(
            "<li>{} {}</li>",
            if *checked { "☑" } else { "☐" },
            children(values)
        ),
    }
}

fn display_value(
    value: &ValueDto,
    definition: Option<&FieldDto>,
    assets: &BTreeMap<String, Option<Asset>>,
    links: &BTreeMap<String, Option<String>>,
    images: &BTreeMap<String, String>,
) -> String {
    let option = |id: &str| {
        definition
            .and_then(|field| field.options.iter().find(|item| item.id == id))
            .map_or_else(|| id.to_owned(), |item| item.label.clone())
    };
    match value {
        ValueDto::Unset {} => "<span class=\"muted\">입력되지 않음</span>".into(),
        ValueDto::NumberUnknown { .. } => "<span>불명</span>".into(),
        ValueDto::SingleLineText { value }
        | ValueDto::Number { value }
        | ValueDto::Date { value }
        | ValueDto::Time { value }
        | ValueDto::Url { value } => escaped(value),
        ValueDto::Duration { milliseconds } => escaped(milliseconds),
        ValueDto::SingleChoice { option: id } => escaped(&option(id)),
        ValueDto::MultiChoice { options } => escaped(
            &options
                .iter()
                .map(|id| option(id))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        ValueDto::RichText { content } => rich(content),
        ValueDto::Image { value: ids } => {
            let mut output = String::from("<div class=\"gallery\">");
            for id in ids {
                match images.get(id) {
                    Some(file) => {
                        output.push_str(&format!(
                            "<figure><img src=\"{}\" alt=\"{}\"><figcaption>{}</figcaption></figure>",
                            escaped(file),
                            escaped(&assets.get(id).and_then(Option::as_ref).map_or_else(|| "이미지".into(), |item| item.name.clone())),
                            escaped(&assets.get(id).and_then(Option::as_ref).map_or_else(|| "이미지".into(), |item| item.name.clone()))
                        ));
                    }
                    None => output.push_str("<div class=\"missing\">이미지를 읽을 수 없음</div>"),
                }
            }
            output.push_str("</div>");
            output
        }
        ValueDto::File { value: ids } => {
            let names = ids
                .iter()
                .map(|id| {
                    assets.get(id).and_then(Option::as_ref).map_or_else(
                        || "첨부 파일을 읽을 수 없음".into(),
                        |item| item.name.clone(),
                    )
                })
                .map(|name| format!("<li>{}</li>", escaped(&name)))
                .collect::<String>();
            format!("<ul class=\"attachments\">{names}</ul>")
        }
        ValueDto::Relation { links: targets } => format!(
            "<ul>{}</ul>",
            targets
                .iter()
                .map(|target| {
                    let name = links
                        .get(&target.document)
                        .and_then(Option::as_ref)
                        .map_or("대상 문서를 읽을 수 없음", String::as_str);
                    format!("<li>{}</li>", escaped(name))
                })
                .collect::<String>()
        ),
        ValueDto::DocumentLink { documents } => format!(
            "<ul>{}</ul>",
            documents
                .iter()
                .map(|id| {
                    let name = links
                        .get(id)
                        .and_then(Option::as_ref)
                        .map_or("대상 문서를 읽을 수 없음", String::as_str);
                    format!("<li>{}</li>", escaped(name))
                })
                .collect::<String>()
        ),
        ValueDto::Group { instances } => {
            let mut output = String::from("<div class=\"group\">");
            for (index, instance) in instances.iter().enumerate() {
                output.push_str(&format!(
                    "<section class=\"group-card\"><h3>항목 {}</h3>",
                    index + 1
                ));
                let mut fields = instance.fields.iter().collect::<Vec<_>>();
                if let Some(definition) = definition {
                    fields.sort_by_key(|field| {
                        definition
                            .member_order
                            .iter()
                            .position(|id| *id == field.field)
                            .unwrap_or(usize::MAX)
                    });
                }
                for field in fields {
                    let member = definition.and_then(|definition| {
                        definition
                            .members
                            .iter()
                            .find(|child| child.id == field.field)
                    });
                    let name = member
                        .map(|child| child.label.as_str())
                        .or_else(|| {
                            instance
                                .labels
                                .iter()
                                .find(|(id, _)| id.to_string() == field.field)
                                .map(|(_, label)| label.as_str())
                        })
                        .unwrap_or("보존된 필드");
                    let body = match &field.value {
                        Intent::Set(value) => display_value(value, member, assets, links, images),
                        Intent::Unset => "입력되지 않음".into(),
                        Intent::Keep => "보존된 값을 읽을 수 없음".into(),
                    };
                    output.push_str(&format!(
                        "<div class=\"group-row\"><strong>{}</strong><div>{}</div></div>",
                        escaped(name),
                        body
                    ));
                }
                output.push_str("</section>");
            }
            output.push_str("</div>");
            output
        }
    }
}

fn html(snapshot: &Snapshot, images: &BTreeMap<String, String>, nonce: &str) -> io::Result<String> {
    let Response::Read {
        name,
        english_name,
        glossary_summary,
        template,
        fields,
        ..
    } = &snapshot.read
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PDF input is not a saved read",
        ));
    };
    let definitions = template
        .fields
        .iter()
        .map(|field| (field.id.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut content = String::new();
    content.push_str("<!doctype html><html lang=\"ko\"><head><meta charset=\"utf-8\">");
    content.push_str(&format!("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src 'self'; font-src 'self'; style-src 'nonce-{nonce}'; script-src 'nonce-{nonce}'; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'\">"));
    content.push_str(&format!("<title>{}</title>", escaped(name)));
    content.push_str(&format!(
        "<style nonce=\"{nonce}\">{} </style></head><body>",
        STYLE
    ));
    content.push_str(&format!(
        "<header class=\"document-heading\"><h1>{}</h1>",
        escaped(name)
    ));
    if !english_name.is_empty() {
        content.push_str(&format!(
            "<p class=\"english\">{}</p>",
            escaped(english_name)
        ));
    }
    if !glossary_summary.is_empty() {
        content.push_str(&format!(
            "<p class=\"summary\">{}</p>",
            escaped(glossary_summary)
        ));
    }
    content.push_str("</header><main>");
    let order = &template.field_order;
    let mut ordered = fields.iter().collect::<Vec<&ReadField>>();
    ordered.sort_by_key(|field| {
        order
            .iter()
            .position(|id| id == &field.id)
            .unwrap_or(usize::MAX)
    });
    for field in ordered {
        let definition = definitions.get(field.id.as_str()).copied();
        content.push_str(&format!(
            "<section class=\"field\"><h2>{}{}</h2><div class=\"value\">",
            escaped(&field.label),
            if field.state == "Orphan" {
                " <span class=\"muted\">(보존된 필드)</span>"
            } else {
                ""
            }
        ));
        match &field.value {
            Some(value) => content.push_str(&display_value(
                value,
                definition,
                &snapshot.assets,
                &snapshot.links,
                images,
            )),
            None => content.push_str("<span class=\"muted\">저장된 값이 없습니다</span>"),
        }
        if field.problem.is_some() {
            content.push_str("<p class=\"missing\">이 필드의 저장 상태를 확인해 주세요.</p>");
        }
        content.push_str("</div></section>");
    }
    content.push_str("</main>");
    content.push_str(&format!("<script nonce=\"{nonce}\">window.addEventListener('load',async()=>{{try{{await document.fonts.ready;if(!document.fonts.check('12pt Noto'))throw Error('font');await Promise.all([...document.images].map(i=>i.decode()));window.chrome.webview.postMessage('worldbuild-pdf-ready')}}catch{{window.chrome.webview.postMessage('worldbuild-pdf-asset-failed')}}}})</script></body></html>"));
    if content.len() > MAX_HTML_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "PDF HTML limit",
        ));
    }
    Ok(content)
}

const STYLE: &str = r#"
@font-face{font-family:Noto;src:url('NotoSansCJKkr-Regular.otf') format('opentype');font-weight:400}
@font-face{font-family:Noto;src:url('NotoSansCJKkr-Medium.otf') format('opentype');font-weight:500}
@font-face{font-family:Noto;src:url('NotoSansCJKkr-Bold.otf') format('opentype');font-weight:700}
@page{size:A4;margin:20mm;@bottom-center{content:counter(page);font-family:Noto,sans-serif;font-size:9pt;color:#586474}}
html,body{margin:0;padding:0;background:white;color:#20252c;font-family:Noto,sans-serif;font-size:10.5pt;line-height:1.55}
body{overflow-wrap:anywhere;word-break:normal}*{box-sizing:border-box}h1,h2,h3,p{margin-top:0}
.document-heading{border-bottom:1px solid #b8c3ce;padding-bottom:8mm;margin-bottom:8mm}
h1{font-size:21pt;line-height:1.35;margin-bottom:2mm;break-after:avoid}.english{font-size:10pt;color:#536171;margin-bottom:3mm}
.summary{font-size:10.5pt;margin-bottom:0;white-space:pre-wrap}.field{margin-bottom:6mm;break-inside:auto}
h2{font-size:12pt;line-height:1.4;margin:0 0 2mm;border-bottom:1px solid #e0e5e9;padding-bottom:1mm;break-after:avoid}
h3{font-size:10pt;line-height:1.3;margin-bottom:2mm;break-after:avoid}.value{white-space:pre-wrap;min-width:0}
.value p{margin:0 0 3mm}.value ul,.value ol{margin:1mm 0 3mm;padding-left:6mm}.value li{margin-bottom:1mm}
.value blockquote{border-left:2px solid #a8b7c6;padding-left:4mm;margin:3mm 0;color:#394450}
.muted{color:#697584}.group-card{border:1px solid #d7dfe6;border-radius:2mm;padding:3mm;margin:2mm 0 4mm;break-inside:avoid-page}
.group-row{display:grid;grid-template-columns:35mm minmax(0,1fr);gap:3mm;border-top:1px solid #e7ebef;padding:2mm 0;break-inside:avoid-page}
.group-row:first-of-type{border-top:0}.group-row strong{font-weight:500}.gallery{display:block}.gallery figure{margin:3mm 0 5mm;break-inside:avoid}
.gallery img{display:block;max-width:100%;max-height:210mm;object-fit:contain}.gallery figcaption{font-size:8.5pt;color:#596575;margin-top:1mm}
.missing{display:block;border:1px dashed #b57e69;padding:2mm;color:#8f4f32}.attachments{list-style:disc}
img,table,pre{max-width:100%}a{color:inherit;text-decoration:none}
"#;

fn temporary_dir() -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("worldbuild-pdf-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&path)?;
    Ok(path)
}

fn decode_image(bytes: &[u8]) -> io::Result<image::DynamicImage> {
    use image::GenericImageView;
    let mut reader = image::ImageReader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "image format"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "image decode"))?;
    let (width, height) = decoded.dimensions();
    if u64::from(width) * u64::from(height) > 16_000_000 {
        return Err(io::Error::new(io::ErrorKind::FileTooLarge, "image pixels"));
    }
    Ok(decoded)
}

pub(crate) fn image_decodable(bytes: &[u8]) -> bool {
    decode_image(bytes).is_ok()
}

fn write_input(snapshot: &Snapshot, directory: &Path) -> io::Result<PathBuf> {
    fs::write(
        directory.join("NotoSansCJKkr-Regular.otf"),
        include_bytes!("../../src/assets/fonts/NotoSansCJKkr-Regular.otf"),
    )?;
    fs::write(
        directory.join("NotoSansCJKkr-Medium.otf"),
        include_bytes!("../../src/assets/fonts/NotoSansCJKkr-Medium.otf"),
    )?;
    fs::write(
        directory.join("NotoSansCJKkr-Bold.otf"),
        include_bytes!("../../src/assets/fonts/NotoSansCJKkr-Bold.otf"),
    )?;
    let mut images = BTreeMap::new();
    for (index, (id, asset)) in snapshot
        .assets
        .iter()
        .filter(|(_, asset)| asset.as_ref().is_some_and(|asset| asset.image))
        .enumerate()
    {
        let Some(asset) = asset else { continue };
        let decoded = decode_image(&asset.bytes)?;
        let filename = format!("image-{index}.png");
        decoded
            .save(directory.join(&filename))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "image encode"))?;
        images.insert(id.clone(), filename);
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let path = directory.join("document.html");
    fs::write(&path, html(snapshot, &images, &nonce)?)?;
    Ok(path)
}

fn managed(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn publish(
    bytes: &[u8],
    destination: &Path,
    project_root: &Path,
    cancelled: &impl Fn() -> bool,
) -> io::Result<Published> {
    use crate::data::{edit_recovery::native, project_file::directory::ProjectDirectory};
    diagnostic_stage("publish_entered");
    if !destination.is_absolute()
        || !destination
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PDF destination",
        ));
    }
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && value.len() <= 240)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "PDF filename"))?;
    let chosen_parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "PDF parent"))?;
    let guard = ProjectDirectory::open_move_destination(chosen_parent)?;
    let parent = fs::canonicalize(chosen_parent)?;
    diagnostic_stage("publish_parent_opened");
    if managed(&parent, &fs::canonicalize(project_root)?) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "PDF project destination",
        ));
    }
    #[cfg(windows)]
    if let Some(app) = APP.get() {
        use tauri::Manager;
        let app_data = crate::app_paths::local_data(app)?;
        if let Ok(root) = fs::canonicalize(app_data) {
            if managed(&parent, &root) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "PDF application destination",
                ));
            }
        }
        for root in app
            .state::<std::sync::Arc<crate::state::AppState>>()
            .managed_project_roots()
        {
            if let Ok(root) = fs::canonicalize(root) {
                if managed(&parent, &root) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "PDF managed project destination",
                    ));
                }
            }
        }
    }
    diagnostic_stage("publish_path_allowed");
    let temporary = format!(".worldbuild-pdf-{}.tmp", uuid::Uuid::new_v4());
    let path = chosen_parent.join(&temporary);
    let mut file = native::open(&path, true, true)?;
    diagnostic_stage("publish_temporary_opened");
    let checked = (|| -> io::Result<()> {
        guard.validate_file(&file, &temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        file.seek(io::SeekFrom::Start(0))?;
        let mut offset = 0;
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            let count = file.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            if bytes.get(offset..offset + count) != Some(&chunk[..count]) {
                return Err(io::Error::other("PDF readback mismatch"));
            }
            offset += count;
        }
        if offset != bytes.len() {
            return Err(io::Error::other("PDF readback length mismatch"));
        }
        Ok(())
    })();
    if let Err(error) = checked {
        let _ = native::cleanup(&file);
        return Err(error);
    }
    diagnostic_stage("publish_readback_verified");
    if cancelled() {
        let _ = native::cleanup(&file);
        return Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
    }
    if let Err(error) = native::publish(&file, name) {
        let _ = native::cleanup(&file);
        return Err(error);
    }
    diagnostic_stage("publish_committed");
    Ok(Published {
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        cleanup_warning: false,
    })
}

pub(crate) fn export(
    snapshot: &Snapshot,
    destination: &Path,
    project_root: &Path,
    cancelled: impl Fn() -> bool,
) -> io::Result<Published> {
    let directory = temporary_dir().map_err(|error| {
        diagnostic_error("temporary_directory", &error);
        error
    })?;
    let result = (|| {
        let html = write_input(snapshot, &directory).map_err(|error| {
            diagnostic_error("input", &error);
            error
        })?;
        diagnostic_stage("input_ready");
        let pdf = directory.join("document.pdf");
        if cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
        }
        render(&html, &pdf, &cancelled).map_err(|error| {
            diagnostic_error("render", &error);
            error
        })?;
        diagnostic_stage("render_ready");
        if cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
        }
        if fs::metadata(&pdf)
            .map_err(|error| {
                diagnostic_error("verify_metadata", &error);
                error
            })?
            .len()
            > MAX_PDF_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "PDF byte limit",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(&pdf)
            .map_err(|error| {
                diagnostic_error("verify_read", &error);
                error
            })?
            .take(MAX_PDF_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                diagnostic_error("verify_read", &error);
                error
            })?;
        if bytes.len() < 100
            || bytes.len() as u64 > MAX_PDF_BYTES
            || !bytes.starts_with(b"%PDF-")
            || !bytes
                .windows(5)
                .rev()
                .take(2048)
                .any(|window| window == b"%%EOF")
        {
            diagnostic_stage("verify_invalid");
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PDF output",
            ));
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&pdf)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                diagnostic_error("verify_sync", &error);
                error
            })?;
        diagnostic_stage("verify_ready");
        publish(&bytes, destination, project_root, &cancelled).map_err(|error| {
            diagnostic_error("publish", &error);
            error
        })
    })();
    match fs::remove_dir_all(&directory) {
        Ok(()) => result,
        Err(error) => {
            diagnostic_error("cleanup", &error);
            result.map(|mut published| {
                published.cleanup_warning = true;
                published
            })
        }
    }
}

#[cfg(not(windows))]
fn render(_: &Path, _: &Path, _: &impl Fn() -> bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "WebView2 is required",
    ))
}

#[cfg(windows)]
fn render(html: &Path, pdf: &Path, cancelled: &impl Fn() -> bool) -> io::Result<()> {
    for attempt in 0..2 {
        let (result, stopped) = render_once(html, pdf, cancelled);
        match result {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt == 0
                    && stopped
                    && !cancelled()
                    && error.kind() != io::ErrorKind::Interrupted
                    && error.kind() != io::ErrorKind::InvalidData
                    && error.kind() != io::ErrorKind::InvalidInput =>
            {
                // WebView2 can fail while its browser process starts. Retry only after the
                // first STA has stopped, and never reuse a partial PDF from that attempt.
                probe_trace(&format!("renderer-retry kind={}", error.kind()));
                diagnostic_error("renderer_retry", &error);
                match fs::remove_file(pdf) {
                    Ok(()) => (),
                    Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => (),
                    Err(cleanup) => return Err(cleanup),
                }
                for _ in 0..10 {
                    if cancelled() {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("PDF renderer has two attempts")
}

#[cfg(windows)]
fn render_once(html: &Path, pdf: &Path, cancelled: &impl Fn() -> bool) -> (io::Result<()>, bool) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::time::{Duration, Instant};
    let Some(folder) = html.parent() else {
        return (
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PDF HTML folder",
            )),
            true,
        );
    };
    let folder = folder.to_string_lossy().to_string();
    let output = pdf.to_string_lossy().to_string();
    let abort = Arc::new(AtomicBool::new(false));
    let thread_abort = abort.clone();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _ = tx.send(render_sta(&folder, &output, &thread_abort));
    });
    let started = Instant::now();
    let result = loop {
        if cancelled() {
            abort.store(true, Ordering::Release);
            break Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
        }
        if started.elapsed() > Duration::from_secs(180) {
            abort.store(true, Ordering::Release);
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PDF renderer timeout",
            ));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(io::Error::other("PDF renderer disconnected"))
            }
        }
    };
    let cleanup_wait = Instant::now();
    while !thread.is_finished() && cleanup_wait.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let stopped = thread.is_finished();
    if stopped {
        let _ = thread.join();
    }
    (result, stopped)
}

#[cfg(windows)]
fn render_sta(folder: &str, output: &str, abort: &std::sync::atomic::AtomicBool) -> io::Result<()> {
    #[cfg(debug_assertions)]
    if std::env::var_os("WORLDBUILD_PDF_PROBE_FAIL_ONCE").is_some()
        && !PROBE_FAILED_ONCE.swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return Err(io::Error::other("injected PDF renderer startup failure"));
    }
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::time::{Duration, Instant};
    use webview2_com::{
        CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2Controller,
            ICoreWebView2Environment6, ICoreWebView2EnvironmentOptions, ICoreWebView2_3,
            ICoreWebView2_7, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
            COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT, COREWEBVIEW2_PROCESS_FAILED_KIND,
            COREWEBVIEW2_WEB_ERROR_STATUS,
        },
        NavigationCompletedEventHandler, NavigationStartingEventHandler,
        PrintToPdfCompletedHandler, ProcessFailedEventHandler, WebMessageReceivedEventHandler,
    };
    use windows::{
        core::{w, Interface, HSTRING, PCWSTR, PWSTR},
        Win32::{
            Foundation::RECT,
            System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
                RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WNDCLASSW, WS_EX_TOOLWINDOW,
                WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        },
    };

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(io::Error::other)?;
    }
    unsafe extern "system" fn window_proc(
        hwnd: windows::Win32::Foundation::HWND,
        message: u32,
        wparam: windows::Win32::Foundation::WPARAM,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }
    fn pump_until<T>(
        receiver: &mpsc::Receiver<T>,
        abort: &std::sync::atomic::AtomicBool,
        limit: Duration,
    ) -> io::Result<T> {
        let started = Instant::now();
        loop {
            if abort.load(Ordering::Acquire) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
            }
            if started.elapsed() > limit {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "PDF initialization timeout",
                ));
            }
            if let Ok(result) = receiver.try_recv() {
                return Ok(result);
            }
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    probe_trace("sta-initialized");
    diagnostic_stage("renderer_sta_initialized");
    let result = (|| -> io::Result<()> {
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: w!("WorldbuildPdfRenderer"),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&window_class) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(1410) {
                return Err(error);
            }
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("WorldbuildPdfRenderer"),
                w!("PDF"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                -32000,
                -32000,
                800,
                1100,
                None,
                None,
                None,
                None,
            )
            .map_err(io::Error::other)?
        };
        probe_trace("sta-window");
        diagnostic_stage("renderer_window");
        let result = (|| -> io::Result<()> {
            // The WebView2 profile outlives a single print. Keep it outside the
            // owned PDF temporary directory so cancellation can remove every
            // input and partial output even while the browser process winds down.
            let user_data = crate::app_paths::local_data(
                APP.get()
                    .ok_or_else(|| io::Error::other("PDF app unavailable"))?,
            )?
            .join("pdf-renderer-profile");
            fs::create_dir_all(&user_data)?;
            let user_data = HSTRING::from(user_data.to_string_lossy().as_ref());
            let (env_tx, env_rx) = mpsc::channel();
            let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                move |status, environment| {
                    let _ = env_tx.send(status.map(|_| environment));
                    Ok(())
                },
            ));
            unsafe {
                CreateCoreWebView2EnvironmentWithOptions(
                    PCWSTR::null(),
                    &user_data,
                    None::<&ICoreWebView2EnvironmentOptions>,
                    &env_handler,
                )
            }
            .map_err(io::Error::other)?;
            let environment = pump_until(&env_rx, abort, Duration::from_secs(30))?
                .map_err(io::Error::other)?
                .ok_or_else(|| io::Error::other("PDF environment missing"))?;
            probe_trace("sta-environment");
            diagnostic_stage("renderer_environment");
            let (controller_tx, controller_rx) = mpsc::channel();
            let controller_handler = CreateCoreWebView2ControllerCompletedHandler::create(
                Box::new(move |status, controller| {
                    let _ = controller_tx.send(status.map(|_| controller));
                    Ok(())
                }),
            );
            unsafe { environment.CreateCoreWebView2Controller(hwnd, &controller_handler) }
                .map_err(io::Error::other)?;
            let controller: ICoreWebView2Controller =
                pump_until(&controller_rx, abort, Duration::from_secs(30))?
                    .map_err(io::Error::other)?
                    .ok_or_else(|| io::Error::other("PDF controller missing"))?;
            probe_trace("sta-controller");
            diagnostic_stage("renderer_controller");
            let printed = (|| -> io::Result<()> {
                unsafe {
                    controller
                        .SetBounds(RECT {
                            left: 0,
                            top: 0,
                            right: 800,
                            bottom: 1100,
                        })
                        .map_err(io::Error::other)?;
                    controller.SetIsVisible(true).map_err(io::Error::other)?;
                }
                let core = unsafe { controller.CoreWebView2() }.map_err(io::Error::other)?;
                let virtual_host: ICoreWebView2_3 = core.cast().map_err(io::Error::other)?;
                unsafe {
                    virtual_host.SetVirtualHostNameToFolderMapping(
                        w!("worldbuild-pdf.invalid"),
                        &HSTRING::from(folder),
                        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
                    )
                }
                .map_err(io::Error::other)?;
                let url = "https://worldbuild-pdf.invalid/document.html";
                let settings = unsafe { core.Settings() }.map_err(io::Error::other)?;
                unsafe {
                    settings
                        .SetAreDefaultContextMenusEnabled(false)
                        .map_err(io::Error::other)?;
                    settings
                        .SetAreDevToolsEnabled(false)
                        .map_err(io::Error::other)?;
                }
                let (result_tx, result_rx) = mpsc::channel::<Result<(), &'static str>>();
                let starting_handler =
                    NavigationStartingEventHandler::create(Box::new(|_, args| {
                        if let Some(args) = args {
                            let mut uri = PWSTR::null();
                            if unsafe { args.Uri(&mut uri) }.is_ok() {
                                probe_trace(&format!(
                                    "sta-nav-start uri={}",
                                    CoTaskMemPWSTR::from(uri).to_string()
                                ));
                            }
                        }
                        Ok(())
                    }));
                let mut starting_token = 0;
                unsafe { core.add_NavigationStarting(&starting_handler, &mut starting_token) }
                    .map_err(io::Error::other)?;
                let failed_tx = result_tx.clone();
                let failed_handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(0);
                        let _ = unsafe { args.ProcessFailedKind(&mut kind) };
                        probe_trace(&format!("sta-process-failed kind={}", kind.0));
                        if matches!(kind.0, 0 | 1 | 3) {
                            diagnostic_stage("renderer_process_failed");
                            let _ = failed_tx.send(Err("renderer_process"));
                        }
                    }
                    Ok(())
                }));
                let mut failed_token = 0;
                unsafe { core.add_ProcessFailed(&failed_handler, &mut failed_token) }
                    .map_err(io::Error::other)?;
                let target_url = url.to_owned();
                let nav_tx = result_tx.clone();
                let nav_handler =
                    NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                        probe_trace("sta-nav-callback");
                        let (Some(sender), Some(args)) = (sender, args) else {
                            return Ok(());
                        };
                        let mut source = PWSTR::null();
                        if unsafe { sender.Source(&mut source) }.is_err() {
                            return Ok(());
                        }
                        let source = CoTaskMemPWSTR::from(source);
                        let mut outcome = windows::core::BOOL(0);
                        let mut web_error = COREWEBVIEW2_WEB_ERROR_STATUS(0);
                        let _ = unsafe { args.IsSuccess(&mut outcome) };
                        let _ = unsafe { args.WebErrorStatus(&mut web_error) };
                        probe_trace(&format!(
                            "sta-nav-result success={} error={}",
                            outcome.as_bool(),
                            web_error.0
                        ));
                        if source.to_string() != target_url {
                            probe_trace(&format!(
                                "sta-nav-other-source source={} target={}",
                                source.to_string(),
                                target_url
                            ));
                            return Ok(());
                        }
                        probe_trace("sta-navigation");
                        diagnostic_stage("renderer_navigation");
                        let mut success = windows::core::BOOL(0);
                        if unsafe { args.IsSuccess(&mut success) }.is_err() || !success.as_bool() {
                            diagnostic_stage("renderer_navigation_failed");
                            let _ = nav_tx.send(Err("navigation"));
                        }
                        Ok(())
                    }));
                let mut nav_token = 0;
                unsafe { core.add_NavigationCompleted(&nav_handler, &mut nav_token) }
                    .map_err(io::Error::other)?;
                let ready_url = url.to_owned();
                let pdf_path = output.to_owned();
                let started_print = Arc::new(AtomicBool::new(false));
                let message_tx = result_tx.clone();
                let ready_handler =
                    WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
                        probe_trace("sta-web-message");
                        let (Some(sender), Some(args)) = (sender, args) else {
                            return Ok(());
                        };
                        let mut source = PWSTR::null();
                        if unsafe { args.Source(&mut source) }.is_err() {
                            return Ok(());
                        }
                        let source = CoTaskMemPWSTR::from(source);
                        if source.to_string() != ready_url {
                            probe_trace(&format!(
                                "sta-message-other-source source={} target={}",
                                source.to_string(),
                                ready_url
                            ));
                            return Ok(());
                        }
                        let mut value = PWSTR::null();
                        if unsafe { args.WebMessageAsJson(&mut value) }.is_err() {
                            return Ok(());
                        }
                        let value = CoTaskMemPWSTR::from(value);
                        if value.to_string() == "\"worldbuild-pdf-asset-failed\"" {
                            diagnostic_stage("renderer_assets_failed");
                            let _ = message_tx.send(Err("render_assets"));
                            return Ok(());
                        }
                        if value.to_string() != "\"worldbuild-pdf-ready\""
                            || started_print.swap(true, Ordering::AcqRel)
                        {
                            return Ok(());
                        }
                        probe_trace("sta-ready");
                        diagnostic_stage("renderer_ready");
                        let start = (|| -> windows::core::Result<()> {
                            unsafe {
                                let printer: ICoreWebView2_7 = sender.cast()?;
                                let environment: ICoreWebView2Environment6 =
                                    printer.Environment()?.cast()?;
                                let print_settings = environment.CreatePrintSettings()?;
                                print_settings
                                    .SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT)?;
                                print_settings.SetPageWidth(210.0 / 25.4)?;
                                print_settings.SetPageHeight(297.0 / 25.4)?;
                                let margin = 20.0 / 25.4;
                                print_settings.SetMarginTop(margin)?;
                                print_settings.SetMarginBottom(margin)?;
                                print_settings.SetMarginLeft(margin)?;
                                print_settings.SetMarginRight(margin)?;
                                print_settings.SetShouldPrintHeaderAndFooter(false)?;
                                print_settings.SetShouldPrintBackgrounds(true)?;
                                let complete_tx = message_tx.clone();
                                let completed = PrintToPdfCompletedHandler::create(Box::new(
                                    move |status, ok| {
                                        probe_trace("sta-print-complete");
                                        diagnostic_stage(if status.is_ok() && ok {
                                            "renderer_print_complete"
                                        } else {
                                            "renderer_print_failed"
                                        });
                                        let _ = complete_tx.send(if status.is_ok() && ok {
                                            Ok(())
                                        } else {
                                            Err("print")
                                        });
                                        Ok(())
                                    },
                                ));
                                printer.PrintToPdf(
                                    &HSTRING::from(&pdf_path),
                                    &print_settings,
                                    &completed,
                                )?;
                                probe_trace("sta-print-start");
                                Ok(())
                            }
                        })();
                        if start.is_err() {
                            diagnostic_stage("renderer_print_start_failed");
                            let _ = message_tx.send(Err("print_start"));
                        }
                        Ok(())
                    }));
                let mut message_token = 0;
                unsafe { core.add_WebMessageReceived(&ready_handler, &mut message_token) }
                    .map_err(io::Error::other)?;
                probe_trace("sta-navigation-installed");
                let started = Instant::now();
                let mut target_requested = false;
                loop {
                    if !target_requested && started.elapsed() > Duration::from_secs(1) {
                        probe_trace("sta-navigate-target");
                        unsafe { core.Navigate(&HSTRING::from(url)) }.map_err(io::Error::other)?;
                        target_requested = true;
                    }
                    if abort.load(Ordering::Acquire) {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "PDF cancelled"));
                    }
                    if started.elapsed() > Duration::from_secs(175) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "PDF renderer timeout",
                        ));
                    }
                    if let Ok(result) = result_rx.try_recv() {
                        return result.map_err(|reason| {
                            if reason == "render_assets" {
                                io::Error::new(io::ErrorKind::InvalidData, reason)
                            } else {
                                io::Error::other(reason)
                            }
                        });
                    }
                    let mut message = MSG::default();
                    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                        unsafe {
                            let _ = TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })();
            let _ = unsafe { controller.Close() };
            printed
        })();
        let _ = unsafe { DestroyWindow(hwnd) };
        result
    })();
    unsafe {
        CoUninitialize();
    }
    result
}

/// The debug probe exercises the actual embedded WebView2 renderer without changing a user
/// project. It runs only when explicitly requested by the local verification command.
#[cfg(all(windows, debug_assertions))]
pub(crate) fn maybe_probe() {
    let Some(folder) = std::env::var_os("WORLDBUILD_PDF_PROBE_DIR") else {
        return;
    };
    std::thread::spawn(move || {
        let output = PathBuf::from(folder);
        let result = (|| -> io::Result<()> {
            fs::create_dir_all(&output)?;
            let protected = output.join("source-fixture");
            fs::create_dir_all(&protected)?;
            if std::env::var_os("WORLDBUILD_PDF_PROBE_MODE").as_deref()
                == Some(std::ffi::OsStr::new("cancel-long"))
            {
                let mut long = probe_snapshots()?
                    .pop()
                    .ok_or_else(|| io::Error::other("probe snapshot"))?
                    .1;
                if let Response::Read { fields, .. } = &mut long.read {
                    if let Some(intro) = fields.iter_mut().find(|field| field.id == "intro") {
                        intro.value = Some(ValueDto::RichText {
                            content: RichNode::Root {
                                children: (0..900)
                                    .map(|index| RichNode::Paragraph {
                                        children: vec![RichNode::Text {
                                            text: format!("긴 문서 {}번째 단락. 인물과 지명의 관계를 설명합니다. ", index + 1).repeat(8),
                                            marks: vec![],
                                        }],
                                    })
                                    .collect(),
                            },
                        });
                    }
                }
                let destination = output.join("cancel-long.pdf");
                let cancelled = export(&long, &destination, &protected, || {
                    output.join("cancel.signal").exists()
                });
                if cancelled
                    .as_ref()
                    .is_err_and(|error| error.kind() == io::ErrorKind::Interrupted)
                    && !destination.exists()
                {
                    fs::write(
                        output.join("cancel-observed.txt"),
                        "render cancellation observed; destination absent",
                    )?;
                    let short = probe_snapshots()?.remove(0).1;
                    let published =
                        export(&short, &output.join("after-cancel.pdf"), &protected, || {
                            false
                        })?;
                    fs::write(output.join("after-cancel.sha256"), published.sha256)?;
                    return Ok(());
                }
                return Err(cancelled
                    .err()
                    .unwrap_or_else(|| io::Error::other("render completed before cancellation")));
            }
            for (name, snapshot) in probe_snapshots()? {
                let destination = output.join(format!("{name}.pdf"));
                let published = export(&snapshot, &destination, &protected, || false)?;
                fs::write(output.join(format!("{name}.sha256")), published.sha256)?;
            }
            Ok(())
        })();
        let message = match result {
            Ok(()) => "ok".to_owned(),
            Err(error) => format!("{}: {}", error.kind(), error),
        };
        let _ = fs::write(output.join("probe-result.txt"), message);
    });
}

#[cfg(all(windows, debug_assertions))]
fn probe_snapshots() -> io::Result<Vec<(&'static str, Snapshot)>> {
    use crate::commands::dto::TemplateDto;
    use crate::data::{artifact::group::InstanceDraft, edit_recovery::model::DraftValue};
    fn field(id: &str, label: &str, kind: &str) -> FieldDto {
        FieldDto {
            members: Vec::new(),
            member_order: Vec::new(),
            minimum: None,
            maximum: None,
            multiple: None,
            allowed_templates: Vec::new(),
            reciprocal_notice: None,
            writing_guide: None,
            id: id.into(),
            label: label.into(),
            kind: kind.into(),
            lifecycle: "Active".into(),
            required: false,
            presentation: None,
            default: ValueDto::Unset {},
            initial_default: ValueDto::Unset {},
            introduced_revision: "1".into(),
            options: Vec::new(),
            option_order: Vec::new(),
        }
    }
    fn snapshot(name: &str, fields: Vec<(FieldDto, ValueDto)>) -> Snapshot {
        let order = fields.iter().map(|(field, _)| field.id.clone()).collect();
        let read_fields = fields
            .iter()
            .map(|(field, value)| ReadField {
                id: field.id.clone(),
                label: field.label.clone(),
                state: "Active".into(),
                provenance: Some("ExistingValue".into()),
                value: Some(value.clone()),
                problem: None,
            })
            .collect();
        let definitions = fields.into_iter().map(|(field, _)| field).collect();
        Snapshot {
            document: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            source: "probe".into(),
            read: Response::Read {
                schema: 8,
                id: uuid::Uuid::new_v4().to_string(),
                name: name.into(),
                english_name: "Worldbuilding sample".into(),
                glossary_summary: "완성된 저장 문서를 외부에 전달하는 PDF 품질 검증 자료입니다."
                    .into(),
                glossary_excluded: false,
                template: TemplateDto {
                    schema: 8,
                    sections: Vec::new(),
                    id: uuid::Uuid::new_v4().to_string(),
                    revision: "1".into(),
                    name: "검증 템플릿".into(),
                    lifecycle: "Active".into(),
                    glossary_excluded: false,
                    presentation: None,
                    field_order: order,
                    fields: definitions,
                },
                fields: read_fields,
                warnings: Vec::new(),
            },
            assets: BTreeMap::new(),
            links: BTreeMap::new(),
            missing_images: Vec::new(),
        }
    }
    let ordinary = snapshot(
        "M6-3 일반 문서",
        vec![
            (
                field("lead", "개요", "SingleLineText"),
                ValueDto::SingleLineText {
                    value: "한글과 English 2026 / 가독성 확인".into(),
                },
            ),
            (
                field("count", "수량", "Number"),
                ValueDto::Number { value: "0".into() },
            ),
            (
                field("unknown", "미상", "Number"),
                ValueDto::NumberUnknown { previous_raw: None },
            ),
            (
                field("body", "본문", "RichText"),
                ValueDto::RichText {
                    content: RichNode::Root {
                        children: vec![
                            RichNode::Heading {
                                level: 2,
                                children: vec![RichNode::Text {
                                    text: "부제목".into(),
                                    marks: vec![],
                                }],
                            },
                            RichNode::Paragraph {
                                children: vec![
                                    RichNode::Text {
                                        text: "강조된 문장과 설명입니다. ".into(),
                                        marks: vec![],
                                    },
                                    RichNode::Text {
                                        text: "굵게".into(),
                                        marks: vec![Mark::Bold],
                                    },
                                    RichNode::Text {
                                        text: " · 기울임".into(),
                                        marks: vec![Mark::Italic],
                                    },
                                ],
                            },
                            RichNode::BulletList {
                                children: vec![
                                    RichNode::ListItem {
                                        children: vec![RichNode::Text {
                                            text: "첫 항목".into(),
                                            marks: vec![],
                                        }],
                                    },
                                    RichNode::ListItem {
                                        children: vec![RichNode::Text {
                                            text: "두 번째 항목".into(),
                                            marks: vec![],
                                        }],
                                    },
                                ],
                            },
                        ],
                    },
                },
            ),
        ],
    );
    let mut complex = snapshot("M6-3 긴 문서 · 복합 내용", vec![
        (field("intro", "긴 소개", "RichText"), ValueDto::RichText { content: RichNode::Root { children: (0..60).map(|index| RichNode::Paragraph { children: vec![RichNode::Text { text: format!("{}번째 단락 — 세계관 설정과 인물 관계, 매우 긴 한국어 본문 및 English wrapping을 확인합니다. ", index + 1).repeat(4), marks: vec![] }] }).collect() } }),
        (field("image", "삽화", "Image"), ValueDto::Image { value: vec!["image-probe".into()] }),
        (field("attachment", "첨부", "File"), ValueDto::File { value: vec!["file-probe".into()] }),
        (field("link", "연결 문서", "DocumentLink"), ValueDto::DocumentLink { documents: vec!["target-probe".into()] }),
        (field("safe", "특수 문자", "SingleLineText"), ValueDto::SingleLineText { value: "<script>alert('unsafe')</script> & 내용".into() }),
        (field("url", "외부 영상 주소", "Url"), ValueDto::Url { value: "https://example.invalid/watch?v=sample&k=한글".into() }),
    ]);
    let mut image = image::RgbImage::new(1400, 900);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x / 8).min(255) as u8, (y / 5).min(255) as u8, 150]);
    }
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(io::Error::other)?;
    complex.assets.insert(
        "image-probe".into(),
        Some(Asset {
            name: "샘플 그림.png".into(),
            bytes: encoded.into_inner(),
            image: true,
        }),
    );
    complex.assets.insert(
        "file-probe".into(),
        Some(Asset {
            name: "자료.pdf".into(),
            bytes: Vec::new(),
            image: false,
        }),
    );
    complex
        .links
        .insert("target-probe".into(), Some("연결된 인물 이름".into()));
    if let Response::Read {
        template, fields, ..
    } = &mut complex.read
    {
        let mut parent = field("group", "반복 목록", "Group");
        parent.member_order = vec!["row-name".into(), "row-value".into()];
        parent.members = vec![
            field("row-name", "이름", "SingleLineText"),
            field("row-value", "설명", "SingleLineText"),
        ];
        template.field_order.push(parent.id.clone());
        template.fields.push(parent.clone());
        fields.push(ReadField {
            id: parent.id,
            label: parent.label,
            state: "Active".into(),
            provenance: Some("ExistingValue".into()),
            problem: None,
            value: Some(ValueDto::Group {
                instances: (0..25)
                    .map(|index| InstanceDraft {
                        id: uuid::Uuid::new_v4().to_string().parse().expect("probe ID"),
                        source: None,
                        lineage: vec![],
                        fields: vec![
                            DraftValue {
                                field: "row-name".into(),
                                value: Intent::Set(ValueDto::SingleLineText {
                                    value: format!("항목 {}", index + 1),
                                }),
                            },
                            DraftValue {
                                field: "row-value".into(),
                                value: Intent::Set(ValueDto::SingleLineText {
                                    value: format!("길고 반복되는 설명 {}", index + 1).repeat(6),
                                }),
                            },
                        ],
                        labels: BTreeMap::new(),
                        protected: vec![],
                    })
                    .collect(),
            }),
        });
    }
    Ok(vec![("ordinary", ordinary), ("complex", complex)])
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn saved_content_is_escaped_and_keeps_rich_group_and_media_meaning() {
        let samples = probe_snapshots().expect("probe snapshots");
        let complex = &samples[1].1;
        let document = html(complex, &BTreeMap::new(), "fixed-nonce").expect("HTML");
        assert!(document.contains("&lt;script&gt;alert(&#39;unsafe&#39;)&lt;/script&gt;"));
        assert!(!document.contains("<script>alert('unsafe')</script>"));
        assert!(document.contains("연결된 인물 이름"));
        assert!(document.contains("자료.pdf"));
        assert!(document.contains("항목 25"));
        assert!(document.contains("이미지를 읽을 수 없음"));
        assert!(document.contains("counter(page)"));
        assert!(!document.contains("target-probe"));
        assert!(!image_decodable(b"corrupt image"));
    }

    #[test]
    fn pdf_publish_refuses_project_paths_and_existing_file_without_overwrite() {
        let base =
            std::env::temp_dir().join(format!("worldbuild-pdf-test-{}", uuid::Uuid::new_v4()));
        let project = base.join("project");
        let external = base.join("external");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&external).expect("external");
        let bytes = b"%PDF-1.7\nfixture\n%%EOF";
        let forbidden = publish(bytes, &project.join("report.pdf"), &project, &|| false)
            .expect_err("project path");
        assert_eq!(forbidden.kind(), io::ErrorKind::PermissionDenied);
        let target = external.join("report.pdf");
        fs::write(&target, b"existing").expect("existing file");
        let conflict = publish(bytes, &target, &project, &|| false).expect_err("collision");
        assert_eq!(conflict.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target).expect("existing preserved"), b"existing");
        let new_target = external.join("new.pdf");
        let completed = publish(bytes, &new_target, &project, &|| false).expect("new PDF");
        assert_eq!(completed.size, bytes.len() as u64);
        assert_eq!(fs::read(&new_target).expect("published"), bytes);
        let cancelled_target = external.join("cancelled.pdf");
        let cancelled = publish(bytes, &cancelled_target, &project, &|| true)
            .expect_err("cancel before publish");
        assert_eq!(cancelled.kind(), io::ErrorKind::Interrupted);
        assert!(!cancelled_target.exists());
        assert_eq!(
            fs::read_dir(&external).expect("external entries").count(),
            2
        );
        fs::remove_dir_all(base).expect("test cleanup");
    }
}
