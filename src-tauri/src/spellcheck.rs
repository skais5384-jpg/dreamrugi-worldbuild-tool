use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{path::BaseDirectory, Manager, Runtime, State, Webview};

use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpellWord {
    word: String,
    valid: bool,
    suggestions: Vec<String>,
}

struct Job {
    cancelled: AtomicBool,
    child: Mutex<Option<Child>>,
}

static JOBS: OnceLock<Mutex<HashMap<String, Arc<Job>>>> = OnceLock::new();
fn jobs() -> &'static Mutex<HashMap<String, Arc<Job>>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct Files(PathBuf, PathBuf);
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(&self.1);
    }
}

fn resource<R: Runtime>(app: &tauri::AppHandle<R>, name: &str) -> Result<PathBuf, String> {
    let expected = expected_resource_hash(name)?;
    let path = app
        .path()
        .resolve(format!("spellcheck/{name}"), BaseDirectory::Resource)
        .map_err(|_| "검사기 위치를 확인할 수 없습니다.".to_string())?;
    if path.is_file() {
        return verified(path, expected);
    }
    #[cfg(debug_assertions)]
    {
        let local = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("spellcheck/resources")
            .join(name);
        if local.is_file() {
            return verified(local, expected);
        }
    }
    Err("로컬 검사기 파일이 없습니다. 앱 설치를 확인해 주세요.".into())
}

fn expected_resource_hash(name: &str) -> Result<&'static str, String> {
    Ok(match name {
        "hunspell-runner.exe" => "8cf5636bb13ed4fc0ef08c82d5f88d23cac5f92dd5c926e5ba1524f19b8ff27a",
        "ko.aff" => "7b8930ce1357691591a3c38afc9cba787ec6dce0e30d0438bd156c4b4c94dbdf",
        "ko.dic" => "14117a72811ed6a083ed374ceb728b0846a689da6f4dbfe0a84d3b3bce124e80",
        _ => return Err("검사기 자료 이름이 올바르지 않습니다.".into()),
    })
}

fn verified(path: PathBuf, expected: &str) -> Result<PathBuf, String> {
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Err("로컬 검사기 파일 형식이 올바르지 않습니다.".into());
    }
    let bytes = fs::read(&path).map_err(|_| "로컬 검사기 파일을 읽을 수 없습니다.".to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != expected {
        return Err("로컬 검사기 파일이 배포본과 다릅니다. 앱을 다시 설치해 주세요.".into());
    }
    Ok(path)
}

fn run<R: Runtime>(
    app: tauri::AppHandle<R>,
    words: Vec<String>,
    job: Arc<Job>,
) -> Result<Vec<SpellWord>, String> {
    if job.cancelled.load(Ordering::Acquire) {
        return Err("검사를 취소했습니다.".into());
    }
    let binary = resource(&app, "hunspell-runner.exe")?;
    let aff = resource(&app, "ko.aff")?;
    let dic = resource(&app, "ko.dic")?;
    let dir = crate::app_paths::local_data(&app)
        .map_err(|_| "검사 작업 공간을 확인할 수 없습니다.".to_string())?
        .join("spellcheck-tmp");
    fs::create_dir_all(&dir).map_err(|_| "검사 작업 공간을 만들 수 없습니다.".to_string())?;
    if !fs::symlink_metadata(&dir).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err("검사 작업 공간이 안전하지 않습니다.".into());
    }
    let id = uuid::Uuid::new_v4();
    let files = Files(dir.join(format!("{id}.in")), dir.join(format!("{id}.out")));
    let mut input = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&files.0)
        .map_err(|_| "검사 입력을 만들 수 없습니다.".to_string())?;
    for word in &words {
        writeln!(input, "{word}").map_err(|_| "검사 입력을 쓸 수 없습니다.".to_string())?;
    }
    input
        .sync_all()
        .map_err(|_| "검사 입력을 저장할 수 없습니다.".to_string())?;
    drop(input);
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&files.1)
        .map_err(|_| "검사 출력을 만들 수 없습니다.".to_string())?;
    let input = File::open(&files.0).map_err(|_| "검사 입력을 읽을 수 없습니다.".to_string())?;
    let child = Command::new(binary)
        .arg(aff)
        .arg(dic)
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "로컬 검사기를 시작할 수 없습니다.".to_string())?;
    {
        let mut current = job.child.lock().unwrap_or_else(|e| e.into_inner());
        *current = Some(child);
        if job.cancelled.load(Ordering::Acquire) {
            if let Some(child) = current.as_mut() {
                let _ = child.kill();
            }
        }
    }
    let started = Instant::now();
    let status = loop {
        let status = {
            let mut current = job.child.lock().unwrap_or_else(|e| e.into_inner());
            let child = current.as_mut().ok_or("검사 작업이 사라졌습니다.")?;
            if job.cancelled.load(Ordering::Acquire) {
                let _ = child.kill();
            }
            if started.elapsed() > Duration::from_secs(90) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "검사 시간이 길어 중단했습니다. 문서를 나눠 다시 검사해 주세요.".into(),
                );
            }
            child
                .try_wait()
                .map_err(|_| "검사기 응답을 읽을 수 없습니다.".to_string())?
        };
        if let Some(status) = status {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    job.child.lock().unwrap_or_else(|e| e.into_inner()).take();
    if job.cancelled.load(Ordering::Acquire) {
        return Err("검사를 취소했습니다.".into());
    }
    if !status.success() {
        return Err("로컬 검사기가 입력을 처리하지 못했습니다.".into());
    }
    let bytes = fs::read(&files.1).map_err(|_| "검사 결과를 읽을 수 없습니다.".to_string())?;
    if bytes.len() > 4_000_000 {
        return Err("검사 결과가 너무 큽니다. 문서를 나눠 검사해 주세요.".into());
    }
    let output =
        String::from_utf8(bytes).map_err(|_| "검사 결과 인코딩을 읽을 수 없습니다.".to_string())?;
    let mut rows = Vec::new();
    for line in output.lines() {
        let columns: Vec<_> = line.split('\t').collect();
        if columns.len() < 2 || !["0", "1"].contains(&columns[1]) {
            return Err("검사 결과 형식이 올바르지 않습니다.".into());
        }
        rows.push(SpellWord {
            word: columns[0].into(),
            valid: columns[1] == "1",
            suggestions: columns[2..].iter().map(|s| (*s).into()).collect(),
        });
    }
    if rows.len() != words.len() || rows.iter().zip(&words).any(|(row, word)| &row.word != word) {
        return Err("검사 결과와 요청이 일치하지 않습니다.".into());
    }
    Ok(rows)
}

#[tauri::command]
pub(crate) async fn spell_check<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    request_id: String,
    words: Vec<String>,
) -> Result<Vec<SpellWord>, String> {
    state
        .caller(webview.label(), webview.window().label())
        .map_err(|_| "검사 요청 권한이 없습니다.".to_string())?;
    if uuid::Uuid::parse_str(&request_id).is_err()
        || words.len() > 4096
        || words.iter().any(|word| {
            word.is_empty()
                || word.len() > 1024
                || !word
                    .chars()
                    .all(|character| ('가'..='힣').contains(&character))
        })
    {
        return Err("검사 범위가 올바르지 않습니다.".into());
    }
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let job = Arc::new(Job {
        cancelled: AtomicBool::new(false),
        child: Mutex::new(None),
    });
    {
        let mut active = jobs().lock().unwrap_or_else(|e| e.into_inner());
        if active.len() >= 2 || active.contains_key(&request_id) {
            return Err("다른 검사가 진행 중입니다.".into());
        }
        active.insert(request_id.clone(), job.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || run(app, words, job)).await;
    jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&request_id);
    result.map_err(|_| "검사 작업을 완료할 수 없습니다.".to_string())?
}

#[tauri::command]
pub(crate) fn spell_cancel<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    request_id: String,
) -> Result<bool, String> {
    state
        .caller(webview.label(), webview.window().label())
        .map_err(|_| "검사 요청 권한이 없습니다.".to_string())?;
    let job = jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&request_id)
        .cloned();
    let Some(job) = job else {
        return Ok(false);
    };
    job.cancelled.store(true, Ordering::Release);
    if let Some(child) = job.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = child.kill();
    }
    Ok(true)
}

pub(crate) fn cancel_all() {
    let active: Vec<_> = jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    for job in active {
        job.cancelled.store(true, Ordering::Release);
        if let Some(child) = job.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = child.kill();
        }
    }
}

pub(crate) fn cleanup_abandoned<R: Runtime>(app: &tauri::AppHandle<R>) {
    let Ok(root) = crate::app_paths::local_data(app) else {
        return;
    };
    let dir = root.join("spellcheck-tmp");
    if !fs::symlink_metadata(&dir).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((id, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_err() || !["in", "out"].contains(&extension) {
            continue;
        }
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;

    #[test]
    fn bundled_spellcheck_inputs_match_pinned_hashes() {
        for name in ["hunspell-runner.exe", "ko.aff", "ko.dic"] {
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("spellcheck/resources")
                .join(name);
            verified(source, expected_resource_hash(name).unwrap()).unwrap();
        }
    }

    #[test]
    fn canonical_dictionary_verification_rejects_changed_or_crlf_bytes() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let folder = Fixture(std::env::temp_dir().join(format!(
            "worldbuild-spell-resource-{}",
            uuid::Uuid::new_v4()
        )));
        fs::create_dir(&folder.0).unwrap();
        for name in ["ko.aff", "ko.dic"] {
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("spellcheck/resources")
                .join(name);
            let bytes = fs::read(source).unwrap();
            assert!(!bytes.windows(2).any(|pair| pair == b"\r\n"));
            let target = folder.0.join(name);
            let crlf = String::from_utf8(bytes.clone())
                .unwrap()
                .replace('\n', "\r\n");
            fs::write(&target, crlf).unwrap();
            assert!(verified(target.clone(), expected_resource_hash(name).unwrap()).is_err());
            let mut changed = bytes;
            changed.push(b' ');
            fs::write(&target, changed).unwrap();
            assert!(verified(target, expected_resource_hash(name).unwrap()).is_err());
        }
    }
}

/// Local packaging probe: use the same resource resolver and runner as the command.
/// It is opt-in and absent from release builds.
#[cfg(debug_assertions)]
pub(crate) fn maybe_probe<R: Runtime>(app: tauri::AppHandle<R>) {
    let Some(folder) = std::env::var_os("WORLDBUILD_SPELL_PROBE_DIR") else {
        return;
    };
    std::thread::spawn(move || {
        let folder = PathBuf::from(folder);
        let result = (|| -> Result<String, String> {
            fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
            let mut locations = Vec::new();
            for name in ["hunspell-runner.exe", "ko.aff", "ko.dic"] {
                let bundled = app
                    .path()
                    .resolve(format!("spellcheck/{name}"), BaseDirectory::Resource)
                    .map_err(|error| error.to_string())?;
                locations.push(format!(
                    "{name}: bundled={} path={}",
                    bundled.is_file(),
                    bundled.display()
                ));
            }
            let job = Arc::new(Job {
                cancelled: AtomicBool::new(false),
                child: Mutex::new(None),
            });
            let words = vec!["안녕하세요".into(), "안녕하세오".into()];
            let checked = run(app, words, job)?;
            if checked.len() != 2 || !checked[0].valid || checked[1].valid {
                return Err("검사기 응답이 예상과 다릅니다.".into());
            }
            Ok(format!("ok\n{}\n", locations.join("\n")))
        })();
        let message = result.unwrap_or_else(|error| format!("error: {error}"));
        let _ = fs::write(folder.join("spell-probe-result.txt"), message);
    });
}
