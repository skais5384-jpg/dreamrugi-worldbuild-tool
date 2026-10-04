//! 실제 guarded IPC와 내부 G9 작업을 기존 M1 경계에서 중단하고 새 process로 연다.
use super::*;
use crate::data::{
    application::composite::tests::g14 as pair,
    transaction::{
        test_support::{
            boundary::{self, Point},
            CommitTestPoint as C, RecoveryTestPoint as R,
        },
        TransactionManifest,
    },
};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

const CHILD: &str = "state::tests::g14::g14_process_child";
const DEADLINE: Duration = Duration::from_secs(40);

fn record(kind: &str, value: &Value) {
    println!("G14_{kind} {value}");
    if let Some(directory) = std::env::var_os("G14_REPORT_DIR") {
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        write_json(
            &directory.join(format!("{kind}-{}.json", uuid::Uuid::new_v4())),
            value,
        );
    }
}

fn write_json(path: &Path, value: &Value) {
    let temporary = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .unwrap();
    file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    fs::rename(temporary, path).unwrap();
}
fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn fingerprint(bytes: &[u8]) -> Value {
    json!({"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))})
}
fn files(root: &Path, relative: &Path, result: &mut Vec<Value>) {
    if !root.join(relative).exists() {
        return;
    }
    let mut entries = fs::read_dir(root.join(relative))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for p in entries {
        let rel = p.strip_prefix(root).unwrap();
        if p.is_dir() {
            files(root, rel, result);
        } else {
            result.push(json!({"path":rel.to_string_lossy().replace('\\',"/"),"fingerprint":fingerprint(&fs::read(&p).unwrap())}));
        }
    }
}
fn disk(base: &Path) -> Value {
    let root = base.join("project");
    let mut entries = vec![];
    for namespace in ["templates", "documents", ".worldbuild/transactions"] {
        files(&root, Path::new(namespace), &mut entries);
    }
    json!(entries)
}
fn capture_oracle(base: &Path) {
    let root = base.join("project");
    let entries = fs::read_dir(root.join(".worldbuild/transactions"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    let journal = &entries[0];
    let manifest: TransactionManifest =
        serde_json::from_slice(&fs::read(journal.join("manifest.json")).unwrap()).unwrap();
    manifest.validate().unwrap();
    let mut targets = vec![];
    for op in &manifest.operations {
        let old = match fs::read(root.join(op.target_path.as_str())) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(_) => panic!("oracle target read failed"),
        };
        let new = fs::read(journal.join(&op.staged_path)).unwrap();
        assert_eq!(fingerprint(&new)["sha256"], op.staged_sha256);
        targets.push(json!({"path":op.target_path.as_str(),"old":old,"new":new}));
    }
    write_json(
        &base.join("oracle.json"),
        &json!({"transaction":manifest.transaction_id.as_str(),"targets":targets}),
    );
}
fn observer(
    base: PathBuf,
    phase: String,
    events: Arc<Mutex<Vec<Value>>>,
    armed: Arc<AtomicBool>,
) -> boundary::Observer {
    Box::new(move |point, index| {
        if !armed.load(Ordering::SeqCst) {
            return Ok(());
        }
        events.lock().unwrap().push(json!({"point":format!("{point:?}"),"index":index,"thread":format!("{:?}",thread::current().id())}));
        if point == Point::Commit(C::ManifestRevalidation) {
            capture_oracle(&base);
        }
        let selected = match phase.as_str() {
            "prepared" => point == Point::Commit(C::ManifestRevalidation),
            "partial" => point == Point::Commit(C::TargetPrecondition) && index == Some(1),
            "applied" => point == Point::Commit(C::CommittedMarkerWrite),
            "committed" => point == Point::Commit(C::CommittedState),
            "rollback" => point == Point::Recovery(R::ProgressState) && index == Some(0),
            "committed_cleanup" | "rollback_cleanup" => point == Point::Owned("cleanup-manifest"),
            _ => false,
        };
        if selected {
            let oracle = read_json(&base.join("oracle.json"));
            write_json(
                &base.join("ready.json"),
                &json!({"pid":std::process::id(),"fixture":base.file_name().unwrap().to_str().unwrap(),"root":root_identity(&base),"transaction":oracle["transaction"],"generation":fingerprint(&fs::read(base.join("oracle.json")).unwrap()),"phase":phase,"point":format!("{point:?}"),"index":index,"events":*events.lock().unwrap()}),
            );
            // ready 이후에는 깨어나더라도 다음 checkpoint로 진행하지 않는다. 부모만 종료할 수 있다.
            loop {
                thread::park();
            }
        }
        if matches!(phase.as_str(), "rollback" | "rollback_cleanup")
            && point == Point::Commit(C::CommittedMarkerWrite)
        {
            // 합성 원인은 apply 후 rollback 진입만 결정한다. 복원과 journal 작업은 실제 M1이다.
            return Err(io::Error::other("G14 controlled apply failure"));
        }
        Ok(())
    })
}
fn reopen(h: &Harness, project: &str, oracle: &Value, expected_new: bool) {
    for target in oracle["targets"].as_array().unwrap() {
        let path = target["path"].as_str().unwrap();
        let id = Path::new(path).file_stem().unwrap().to_str().unwrap();
        let expected = &target[if expected_new { "new" } else { "old" }];
        let input = if path.starts_with("templates/") {
            json!({"kind":"read_template","project":project,"template":id})
        } else {
            json!({"kind":"read_document","project":project,"document":id})
        };
        let result = h.work(input);
        if expected.is_null() {
            assert_eq!(result["kind"], "rejected");
            assert!(!h.root.join(path).exists());
        } else {
            assert!(
                result.get("content").is_some(),
                "official repository read available"
            );
            let expected = bytes(expected).unwrap();
            if path.starts_with("templates/") {
                let typed = artifact::decode_template(&expected).unwrap();
                assert_eq!(
                    result["content"]["revision"],
                    typed.revision().get().to_string()
                );
                assert_eq!(
                    result["content"]["lifecycle"],
                    format!("{:?}", typed.lifecycle())
                );
                assert_eq!(result["content"]["id"], typed.template_id().to_string());
                let list = h.work(json!({"kind":"list_templates","project":project}));
                let rows = list["templates"].as_array().unwrap();
                let matching = rows.iter().filter(|r| r["id"] == id).collect::<Vec<_>>();
                assert_eq!(
                    matching.len(),
                    1,
                    "stored identity appears exactly once in authoritative list"
                );
                assert_eq!(matching[0]["lifecycle"], format!("{:?}", typed.lifecycle()));
            } else {
                let typed = artifact::decode_document(&expected).unwrap();
                assert_eq!(
                    result["content"]["templateRevision"],
                    typed.template_revision().get().to_string()
                );
                assert_eq!(result["content"]["id"], typed.document_id().to_string());
            }
        }
    }
}

#[test]
#[ignore = "mandatory child of g14 representative and crash matrix parents"]
fn g14_process_child() {
    let base = PathBuf::from(std::env::var_os("G14_BASE").expect("parent base required"));
    let action = std::env::var("G14_ACTION").unwrap();
    let scenario = std::env::var("G14_SCENARIO").unwrap();
    if action == "seed" && scenario == "pair" {
        pair::seed(&base);
        // This matrix isolates transaction recovery from subsequent policy
        // admission. Seed current canonical headers before capturing either
        // transaction oracle; keep every lossless payload subtree unchanged.
        for namespace in ["templates", "documents"] {
            for entry in fs::read_dir(base.join("project").join(namespace)).unwrap() {
                let path = entry.unwrap().path();
                let relative = format!(
                    "{namespace}/{}",
                    path.file_name().unwrap().to_str().unwrap()
                );
                let original = fs::read(&path).unwrap();
                let admitted = current_policy_fixture(&original, &relative);
                fs::write(path, admitted).unwrap();
            }
        }
        return;
    }
    let phase = std::env::var("G14_PHASE").unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let armed = Arc::new(AtomicBool::new(false));
    if action == "store" && scenario == "pair" {
        let _guard =
            boundary::install(observer(base.clone(), phase, events.clone(), armed.clone()));
        armed.store(true, Ordering::SeqCst);
        pair::store(&base);
        write_json(
            &base.join("normal.json"),
            &json!({"events":*events.lock().unwrap()}),
        );
        return;
    }
    gates::observe_io(
        base.join("project"),
        observer(base.clone(), phase.clone(), events.clone(), armed.clone()),
    );
    let h = Harness::at(base.clone(), backend::provider(), false);
    if action == "blocked" {
        armed.store(true, Ordering::SeqCst);
        let before = disk(&base);
        let marker = fs::read(h.root.join(".worldbuild")).unwrap();
        let opened = h.work(json!({"kind":"open","root":h.root}));
        let p = opened["project"].as_str().unwrap();
        let status = h.call(json!({"action":"project_status","project":p}));
        assert_eq!(status["runtime"], "Blocked");
        for input in [
            json!({"kind":"list_templates","project":p}),
            json!({"kind":"read_template","project":p,"template":"99999999-9999-4999-8999-000000000064"}),
            json!({"kind":"create_template","project":p,"name":"blocked write","presentation":null}),
        ] {
            let result = h.work(input);
            assert_eq!(result["kind"], "rejected");
            assert!(!result.to_string().contains("G14_PRIVATE"));
        }
        assert!(disk(&base) == before);
        assert!(fs::read(h.root.join(".worldbuild")).unwrap() == marker);
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["point"].as_str().unwrap().starts_with("Prepare(")));
        h.call(json!({"action":"app_shutdown"}));
        let end = Instant::now() + LIMIT;
        loop {
            h.state.poll();
            let status = h.call(json!({"action":"project_status","project":p}));
            if status["shutdown"]["phase"] == "Blocked" {
                break;
            }
            assert!(Instant::now() < end, "blocked shutdown report deadline");
            thread::yield_now();
        }
        assert!(
            !h.state.poll(),
            "Blocked initialization cannot become normal exit success"
        );
        write_json(
            &base.join(format!("blocked-{phase}.json")),
            &json!({"pid":std::process::id(),"runtime":status["runtime"],"evidence":fingerprint(&marker),"disk":disk(&base),"events":*events.lock().unwrap()}),
        );
        return;
    }
    let p = h.open();
    if action == "reopen" {
        let oracle = read_json(&base.join("oracle.json"));
        reopen(&h, &p, &oracle, std::env::var("G14_NEW").unwrap() == "true");
        if scenario == "pair" {
            pair::preserved_pair(&base);
        }
        let status = h.call(json!({"action":"project_status","project":p}));
        write_json(
            &base.join(format!("reopen-{}.json", phase)),
            &json!({"pid":std::process::id(),"runtime":status["runtime"],"disk":disk(&base)}),
        );
        h.close_clean();
        return;
    }
    if action == "seed" {
        if scenario == "tombstone" {
            let (id, _) = h.template(&p);
            write_json(&base.join("seed.json"), &json!({"template":id}));
        }
        h.close_clean();
        return;
    }
    let input = if scenario == "tombstone" {
        let id = read_json(&base.join("seed.json"))["template"]
            .as_str()
            .unwrap()
            .to_owned();
        let read = h.read_template(&p, &id);
        let session = h.session(&p, vec![read["view"].clone()], "template");
        json!({"kind":"tombstone_template","project":p,"session":session,"view":read["view"],"revision":"1"})
    } else {
        json!({"kind":"create_template","project":p,"name":"G14 fixed template","presentation":null})
    };
    armed.store(true, Ordering::SeqCst);
    let result = h.work(input);
    assert_eq!(result["disk"], "committed");
    write_json(
        &base.join("normal.json"),
        &json!({"pid":std::process::id(),"events":*events.lock().unwrap(),"result":result}),
    );
    h.close_clean();
}

struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
fn spawn(base: &Path, scenario: &str, action: &str, phase: &str, new: bool) -> ChildOwner {
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env("G14_BASE", base)
        .env("G14_SCENARIO", scenario)
        .env("G14_ACTION", action)
        .env("G14_PHASE", phase)
        .env("G14_NEW", new.to_string())
        .stdin(Stdio::null())
        .stdout(fs::File::create(base.join(format!("{action}-{phase}.stdout"))).unwrap())
        .stderr(fs::File::create(base.join(format!("{action}-{phase}.stderr"))).unwrap())
        .spawn()
        .unwrap();
    ChildOwner(child)
}
fn completed(mut child: ChildOwner) {
    let end = Instant::now() + DEADLINE;
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "G14 child failed; preserved fixture contains original output"
            );
            return;
        }
        assert!(
            Instant::now() < end,
            "G14 child completion deadline; fixture preserved"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn bytes(value: &Value) -> Option<Vec<u8>> {
    if value.is_null() {
        None
    } else {
        Some(serde_json::from_value(value.clone()).unwrap())
    }
}
fn assert_generation(base: &Path, oracle: &Value, new: bool) {
    for target in oracle["targets"].as_array().unwrap() {
        let actual = match fs::read(base.join("project").join(target["path"].as_str().unwrap())) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(_) => panic!("target observation failed"),
        };
        assert!(
            actual == bytes(&target[if new { "new" } else { "old" }]),
            "one fixed logical generation required"
        );
    }
}
fn assert_boundary_disk(base: &Path, oracle: &Value, phase: &str) {
    let targets = oracle["targets"].as_array().unwrap();
    for (index, target) in targets.iter().enumerate() {
        let new = match phase {
            "prepared" => false,
            "partial" => index == 0,
            // rollback은 역순이다. index1의 실제 복원 뒤 index0은 아직 새 세대다.
            "rollback" => index == 0,
            "rollback_cleanup" => false,
            _ => true,
        };
        let one = json!({"targets":[target]});
        assert_generation(base, &one, new);
    }
    if phase != "normal" {
        let entries = fs::read_dir(base.join("project/.worldbuild/transactions"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        let journal = &entries[0];
        let state = read_json(&journal.join("state.json"));
        assert_eq!(state["schemaVersion"], 2, "owned journal required");
        if phase.ends_with("cleanup") {
            assert!(
                !journal.join("manifest.json").exists(),
                "actual cleanup crossed manifest removal"
            );
            assert!(
                !journal.join("staged").exists(),
                "actual staged cleanup completed"
            );
        } else {
            assert!(journal.join("manifest.json").exists());
        }
        if phase == "committed" {
            assert!(journal.join("committed.json").exists());
        }
    }
}
fn root_identity(base: &Path) -> Value {
    fingerprint(base.join("project").to_str().unwrap().as_bytes())
}
fn observe(sequence: &mut Vec<Value>, event: &str, value: Value) {
    sequence.push(json!({"sequence":sequence.len() + 1,"event":event,"value":value}));
}
struct StoreCase {
    base: PathBuf,
    before: Value,
    child: ChildOwner,
}
fn start_case(scenario: &str, phase: &str, new: bool) -> StoreCase {
    let base = std::env::temp_dir().join(format!("worldbuild-g14-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&base).unwrap();
    completed(spawn(&base, scenario, "seed", "seed", false));
    let before = disk(&base);
    let child = spawn(&base, scenario, "store", phase, new);
    StoreCase {
        base,
        before,
        child,
    }
}
fn wait_ready(case: &mut StoreCase, phase: &str, sequence: &mut Vec<Value>) -> Value {
    let end = Instant::now() + DEADLINE;
    while !case.base.join("ready.json").exists() {
        assert!(
            case.child.0.try_wait().unwrap().is_none(),
            "child exited before handshake; fixture preserved"
        );
        assert!(
            Instant::now() < end,
            "handshake deadline; fixture preserved"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let ready = read_json(&case.base.join("ready.json"));
    let oracle = read_json(&case.base.join("oracle.json"));
    assert_eq!(ready["pid"], case.child.0.id());
    assert_eq!(
        ready["fixture"],
        case.base.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(ready["root"], root_identity(&case.base));
    assert_eq!(ready["phase"], phase);
    assert!(oracle["transaction"].as_str().is_some());
    assert_eq!(ready["transaction"], oracle["transaction"]);
    let journals = fs::read_dir(case.base.join("project/.worldbuild/transactions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(journals.len(), 1);
    assert_eq!(
        journals[0].file_name().unwrap().to_str().unwrap(),
        oracle["transaction"].as_str().unwrap()
    );
    if !phase.ends_with("cleanup") {
        let manifest: TransactionManifest =
            serde_json::from_slice(&fs::read(journals[0].join("manifest.json")).unwrap()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(ready["transaction"], manifest.transaction_id.as_str());
    }
    // 실제 prepared oracle 지문으로 이 run의 old/new 세대를 묶는다. 세대 값의 전역 유일성은 요구하지 않는다.
    assert_eq!(
        ready["generation"],
        fingerprint(&fs::read(case.base.join("oracle.json")).unwrap())
    );
    let (point, index) = match phase {
        "prepared" => (Point::Commit(C::ManifestRevalidation), None),
        "partial" => (Point::Commit(C::TargetPrecondition), Some(1)),
        "applied" => (Point::Commit(C::CommittedMarkerWrite), None),
        "committed" => (
            Point::Commit(C::CommittedState),
            Some(u32::try_from(oracle["targets"].as_array().unwrap().len() - 1).unwrap()),
        ),
        "rollback" => (Point::Recovery(R::ProgressState), Some(0)),
        "committed_cleanup" | "rollback_cleanup" => (Point::Owned("cleanup-manifest"), Some(0)),
        _ => panic!("selected crash phase required"),
    };
    assert_eq!(ready["point"], format!("{point:?}"));
    assert_eq!(ready["index"], json!(index));
    let last = ready["events"].as_array().unwrap().last().unwrap();
    assert_eq!(last["point"], ready["point"]);
    assert_eq!(last["index"], ready["index"]);
    assert!(
        case.child.0.try_wait().unwrap().is_none(),
        "ready child must remain alive"
    );
    observe(sequence, "ready", ready.clone());
    ready
}
fn finish_case(
    case: StoreCase,
    scenario: &str,
    phase: &str,
    new: bool,
    ready: Value,
    sequence: &mut Vec<Value>,
) {
    let StoreCase {
        base,
        before,
        mut child,
    } = case;
    let pid = child.0.id();
    let killed = phase != "normal";
    let handshake = if killed {
        assert_eq!(ready["pid"], pid);
        assert_eq!(ready["phase"], phase);
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "unexpected exit is not a process kill"
        );
        observe(sequence, "kill_start", json!({"pid":pid}));
        child.0.kill().unwrap();
        let status = child.0.wait().unwrap();
        assert!(!status.success());
        observe(
            sequence,
            "kill_wait",
            json!({"pid":pid,"success":status.success(),"code":status.code()}),
        );
        ready
    } else {
        completed(child);
        read_json(&base.join("normal.json"))
    };
    let oracle = read_json(&base.join("oracle.json"));
    let events = handshake["events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["point"] == "Commit(ManifestRevalidation)")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["point"] == "Prepare(TransactionDirectoryAllocated)")
            .count(),
        1
    );
    if scenario == "pair" {
        assert_eq!(oracle["targets"].as_array().unwrap().len(), 2);
        assert!(oracle["targets"][0]["path"]
            .as_str()
            .unwrap()
            .starts_with("documents/"));
    }
    let at_kill = disk(&base);
    assert_boundary_disk(&base, &oracle, phase);
    completed(spawn(&base, scenario, "reopen", "first", new));
    observe(
        sequence,
        "first_open",
        json!({"store_pid":pid,"result":read_json(&base.join("reopen-first.json"))}),
    );
    assert_generation(&base, &oracle, new);
    let first = disk(&base);
    assert!(
        fs::read_dir(base.join("project/.worldbuild/transactions"))
            .unwrap()
            .next()
            .is_none(),
        "first official recovery cleans owned transaction"
    );
    completed(spawn(&base, scenario, "reopen", "second", new));
    observe(
        sequence,
        "second_open",
        json!({"store_pid":pid,"result":read_json(&base.join("reopen-second.json"))}),
    );
    assert_generation(&base, &oracle, new);
    let second = disk(&base);
    assert!(first == second, "second cold recovery must be idempotent");
    for unchanged in before.as_array().unwrap() {
        if !oracle["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["path"] == unchanged["path"])
        {
            assert!(
                second.as_array().unwrap().contains(unchanged),
                "unrelated artifact preserved"
            );
        }
    }
    let record = json!({"scenario":scenario,"phase":phase,"expected_new":new,"killed_pid":if killed {Some(pid)} else {None},"handshake":handshake,"before":before,"at_kill":at_kill,"first":read_json(&base.join("reopen-first.json")),"second":read_json(&base.join("reopen-second.json"))});
    self::record("CASE", &record);
    // 각 child 종료 뒤 자기 fixture만 정리한다. 실패는 이 지점에 도달하지 않아 원 증거가 남는다.
    assert!(base.parent() == Some(std::env::temp_dir().as_path()));
    fs::remove_dir_all(base).unwrap();
}
fn run_case(scenario: &str, phase: &str, new: bool) {
    let mut case = start_case(scenario, phase, new);
    let mut sequence = vec![];
    let ready = if phase == "normal" {
        Value::Null
    } else {
        wait_ready(&mut case, phase, &mut sequence)
    };
    finish_case(case, scenario, phase, new, ready, &mut sequence);
}

#[test]
fn g14_representative_guarded_create_normal_and_prepared_kill_cold_reopen() {
    run_case("create", "normal", true);
    run_case("create", "prepared", false);
}
#[test]
fn g14_representative_internal_g9_normal_pair_control() {
    run_case("pair", "normal", true);
}

#[test]
fn g14_actual_pair_six_crash_boundaries_and_guarded_create_tombstone_recovery() {
    for (phase, new) in [
        ("prepared", false),
        ("partial", false),
        ("committed", true),
        ("rollback", false),
        ("committed_cleanup", true),
        ("rollback_cleanup", false),
    ] {
        run_case("pair", phase, new);
    }
    for scenario in ["create", "tombstone"] {
        run_case(scenario, "applied", false);
        run_case(scenario, "committed", true);
    }
}

#[test]
fn g14_parallel_children_keep_observers_roots_and_generations_isolated() {
    let mut sequence = vec![];
    let mut first = start_case("create", "prepared", false);
    let first_ready = wait_ready(&mut first, "prepared", &mut sequence);
    // 첫 store는 observer 안에 머문 채 두 번째를 준비한다. 실패 시 두 소유 guard가 자기 child를 회수한다.
    let mut second = start_case("tombstone", "committed", true);
    let second_ready = wait_ready(&mut second, "committed", &mut sequence);
    assert_ne!(first.base, second.base);
    for identity in ["fixture", "root", "pid", "transaction"] {
        assert_ne!(first_ready[identity], second_ready[identity]);
    }
    let alive = [
        first.child.0.try_wait().unwrap().is_none(),
        second.child.0.try_wait().unwrap().is_none(),
    ];
    assert!(
        alive.iter().all(|value| *value),
        "both observers must still be active before either kill"
    );
    observe(
        &mut sequence,
        "both_ready",
        json!({"handshakes":[first_ready,second_ready],"alive":alive}),
    );
    record("BOTH_READY", &json!(sequence));
    finish_case(
        first,
        "create",
        "prepared",
        false,
        first_ready,
        &mut sequence,
    );
    finish_case(
        second,
        "tombstone",
        "committed",
        true,
        second_ready,
        &mut sequence,
    );
    record("PARALLEL", &json!(sequence));
}
#[test]
fn g14_cold_blocked_open_keeps_corrupt_recovery_evidence_and_denies_artifacts() {
    let base = std::env::temp_dir().join(format!("worldbuild-g14-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(base.join("project/templates")).unwrap();
    let (template, _) = crate::data::application::composite::tests::guarded_fixture_bytes();
    let id = artifact::decode_template(&template).unwrap().template_id();
    fs::write(base.join(format!("project/templates/{id}.json")), template).unwrap();
    // 기존 G11 실패 fixture와 같은 실제 경로 종류 충돌이다. journal/authority를 합성하지 않는다.
    fs::write(
        base.join("project/.worldbuild"),
        b"G14_PRIVATE_RECOVERY_CANARY",
    )
    .unwrap();
    completed(spawn(&base, "blocked", "blocked", "first", false));
    completed(spawn(&base, "blocked", "blocked", "second", false));
    let first = read_json(&base.join("blocked-first.json"));
    let second = read_json(&base.join("blocked-second.json"));
    assert!(first["pid"] != second["pid"]);
    assert!(first["disk"] == second["disk"]);
    assert!(first["evidence"] == second["evidence"]);
    record("BLOCKED", &json!({"first":first,"second":second}));
    assert!(base.parent() == Some(std::env::temp_dir().as_path()));
    fs::remove_dir_all(base).unwrap();
}
mod authority;
mod live;
