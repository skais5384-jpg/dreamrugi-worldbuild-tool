use super::*;
mod m523;
mod m545;
mod preview;
mod workspace;
use crate::commands::{self, dto};
use crate::data::artifact;
use serde_json::{json, Value};
use std::{fs, sync::mpsc, thread, time::Instant};
use tauri::{
    ipc::{CallbackFn, InvokeBody},
    test::{mock_builder, MockRuntime},
    webview::InvokeRequest,
};

const LIMIT: Duration = Duration::from_secs(20);
fn operation_limit() -> Duration {
    // 일반 회귀의 빠른 hang 검출은 유지하고, 명시적 규모 측정만 사전 선언한
    // 35초 단계 한도를 끝까지 관측할 수 있게 더 긴 하네스 대기 창을 사용한다.
    if std::env::var_os("M39_SCALE_BASE").is_some() || std::env::var_os("M41_SCALE_BASE").is_some()
    {
        Duration::from_secs(120)
    } else {
        LIMIT
    }
}
use crate::data::{
    application::worker::bridge_test_support as gates,
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockProviderInfo, NoLockService,
    },
};
struct Provider {
    original: NoLockService,
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    validations: std::sync::atomic::AtomicUsize,
    threads: Mutex<Vec<thread::ThreadId>>,
    lose: std::sync::atomic::AtomicBool,
}
impl Provider {
    fn new() -> Self {
        Self {
            original: NoLockService::new(),
            gate: Mutex::new(None),
            validations: std::sync::atomic::AtomicUsize::new(0),
            threads: Mutex::new(Vec::new()),
            lose: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn hold(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (tx, entered) = mpsc::channel();
        let (release, rx) = mpsc::channel();
        *self.gate.lock().unwrap() = Some((tx, rx));
        (entered, release)
    }
}
impl LockService for Provider {
    fn provider_info(&self) -> LockProviderInfo {
        self.original.provider_info()
    }
    fn acquire(&self, r: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        self.threads.lock().unwrap().push(thread::current().id());
        self.original.acquire(r)
    }
    fn validate(&self, h: &mut dyn HeldLock) -> Result<(), LockError> {
        self.threads.lock().unwrap().push(thread::current().id());
        self.validations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some((tx, rx)) = gate {
            tx.send(()).unwrap();
            rx.recv_timeout(LIMIT).unwrap();
        }
        if self.lose.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(LockError::for_held(
                crate::data::collaboration_lock::LockErrorCategory::LockLost,
                h.provider_kind(),
                crate::data::collaboration_lock::LockOperation::Validate,
                h.project_fingerprint(),
                h.session_id(),
                h.target(),
            ));
        }
        self.original.validate(h)
    }
    fn release(&self, h: &mut dyn HeldLock) -> Result<(), LockError> {
        self.threads.lock().unwrap().push(thread::current().id());
        self.original.release(h)
    }
}
struct Harness {
    remove_on_drop: bool,
    base: PathBuf,
    root: PathBuf,
    state: Arc<AppState>,
    app: tauri::App<MockRuntime>,
    main: tauri::WebviewWindow<MockRuntime>,
    wake: mpsc::Receiver<()>,
}
impl Harness {
    fn new() -> Self {
        Self::with_provider(backend::provider())
    }
    fn with_provider(provider: Arc<dyn LockService>) -> Self {
        let base = std::env::temp_dir().join(format!("worldbuild-g13-{}", uuid::Uuid::new_v4()));
        Self::at(base, provider, true)
    }
    fn at(base: PathBuf, provider: Arc<dyn LockService>, remove_on_drop: bool) -> Self {
        Self::at_recovery(base, provider, remove_on_drop, None)
    }
    fn at_recovery(
        base: PathBuf,
        provider: Arc<dyn LockService>,
        remove_on_drop: bool,
        recovery_root: Option<PathBuf>,
    ) -> Self {
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        let mut state = AppState::with_provider(base.join("locks"), provider);
        if let Some(recovery_root) = recovery_root {
            state.recovery = Arc::new(crate::data::edit_recovery::Owner::new(recovery_root));
        }
        let state = Arc::new(state);
        let (tx, wake) = mpsc::channel();
        state.set_wake(Arc::new(move || {
            let _ = tx.send(());
        }));
        // 제품 generated app manifest/capability를 유지하고 창 생성은 mock runtime에 맡긴다.
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        let app = commands::register(mock_builder().plugin(tauri_plugin_dialog::init()))
            .manage(state.clone())
            .build(context)
            .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        Self {
            remove_on_drop,
            base,
            root,
            state,
            app,
            main,
            wake,
        }
    }
    fn owned_svn(base: PathBuf, manager: crate::svn::Manager) -> Self {
        let root = base.join("project");
        let state = Arc::new(
            AppState::with_provider(base.join("codex-owned-locks"), backend::provider())
                .with_svn_manager(manager),
        );
        let (tx, wake) = mpsc::channel();
        state.set_wake(Arc::new(move || {
            let _ = tx.send(());
        }));
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        let app = commands::register(mock_builder().plugin(tauri_plugin_dialog::init()))
            .manage(state.clone())
            .build(context)
            .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        Self {
            remove_on_drop: false,
            base,
            root,
            state,
            app,
            main,
            wake,
        }
    }
    fn request(command: &str, body: Vec<u8>, url: &str) -> InvokeRequest {
        InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.parse().unwrap(),
            body: InvokeBody::Raw(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        }
    }
    fn ipc(&self, body: Value) -> Result<Value, Value> {
        tauri::test::get_ipc_response(
            &self.main,
            Self::request(
                "guarded",
                serde_json::to_vec(&body).unwrap(),
                "http://tauri.localhost",
            ),
        )
        .map(|r| r.deserialize().unwrap())
    }
    fn call(&self, body: Value) -> Value {
        self.ipc(body).unwrap()
    }
    fn reserve(&self, lane: &str) -> String {
        self.call(json!({"action":"reserve","lane":lane}))["operation"]
            .as_str()
            .unwrap()
            .into()
    }
    fn submit(&self, input: Value) -> String {
        let lane = if matches!(
            input["kind"].as_str(),
            Some(
                "recover"
                    | "close"
                    | "session_control"
                    | "release_template_draft"
                    | "recovery_close_cursor"
                    | "recovery_release_selection"
                    | "recovery_revalidate"
                    | "recovery_discard"
            )
        ) || (input["kind"] == "template_draft" && input["action"] == "deposit")
        {
            "control"
        } else {
            "ordinary"
        };
        let operation = self.reserve(lane);
        self.call(json!({"action":"submit","operation":operation,"input":input}));
        operation
    }
    fn result(&self, operation: &str) -> Value {
        let end = Instant::now() + operation_limit();
        loop {
            let r = self.call(json!({"action":"operation","operation":operation}));
            if r["state"] == "complete" || r["state"] == "rejected" {
                return r["result"].clone();
            }
            let left = end.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "operation completion deadline");
            self.wake.recv_timeout(left).unwrap();
        }
    }
    fn ack(&self, operation: &str) {
        self.call(json!({"action":"acknowledge_transport","operation":operation}));
    }
    fn work(&self, input: Value) -> Value {
        let id = self.submit(input);
        let result = self.result(&id);
        self.ack(&id);
        result
    }
    fn control(&self, input: Value) -> Value {
        let (id, result) = self.control_result(input);
        self.ack(&id);
        result
    }
    fn control_result(&self, input: Value) -> (String, Value) {
        let end = Instant::now() + LIMIT;
        loop {
            let id = self.submit(input.clone());
            let result = self.result(&id);
            if result["kind"] != "rejected" || result["error"]["code"] != "full" {
                return (id, result);
            }
            self.ack(&id);
            // Full은 미수락이다. 종료 pass의 완료 통지를 받은 뒤 새 ID로 명시적 후속 호출한다.
            let left = end.saturating_duration_since(Instant::now());
            assert!(!left.is_zero());
            self.wake.recv_timeout(left).unwrap();
        }
    }
    fn open(&self) -> String {
        let r = self.work(json!({"kind":"open","root":self.root}));
        let p = r["project"].as_str().unwrap().to_owned();
        let status = self.call(json!({"action":"project_status","project":p}));
        assert_eq!(status["status"], "Ready");
        assert_eq!(status["runtime"], "Ready");
        p
    }
    fn template(&self, p: &str) -> (String, String) {
        let r = self.work(
            json!({"kind":"create_template","project":p,"name":"G13 template","presentation":null}),
        );
        assert_eq!(r["disk"], "committed");
        (
            r["artifact"].as_str().unwrap().into(),
            r["session"].as_str().unwrap().into(),
        )
    }
    fn read_template(&self, p: &str, id: &str) -> Value {
        self.work(json!({"kind":"read_template","project":p,"template":id}))
    }
    fn session(&self, p: &str, views: Vec<Value>, purpose: &str) -> String {
        let r =
            self.work(json!({"kind":"begin_session","project":p,"views":views,"purpose":purpose}));
        assert_eq!(r["state"], "Editing");
        r["session"].as_str().unwrap().into()
    }
    fn close_clean(&self) {
        self.call(json!({"action":"app_shutdown"}));
        let end = Instant::now() + LIMIT;
        loop {
            self.state.poll();
            let projects = self
                .state
                .lock()
                .projects
                .keys()
                .copied()
                .collect::<Vec<_>>();
            for p in &projects {
                self.call(json!({"action":"acknowledge_shutdown","project":String::from(*p)}));
            }
            if self.state.poll() {
                break;
            }
            if Instant::now() >= end {
                let statuses = projects
                    .iter()
                    .map(|p| {
                        self.call(json!({
                            "action":"project_status",
                            "project":String::from(*p)
                        }))
                    })
                    .collect::<Vec<_>>();
                panic!("normal exit deadline: {statuses:?}");
            }
            thread::yield_now();
        }
        assert!(self.state.lock().projects.values().all(|p| p.joined));
        self.state.clear_wake();
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        // 테스트 fixture만 정리한다. 남은 owner를 다루는 검사는 명시적 회수 후 여기에 도달한다.
        self.state.clear_wake();
        self.state.recovery.release_fixture();
        if self.remove_on_drop {
            assert!(self.base.parent() == Some(std::env::temp_dir().as_path()));
        }
        if self.remove_on_drop && self.base.exists() {
            if let Err(error) = fs::remove_dir_all(&self.base) {
                if thread::panicking() {
                    eprintln!("fixture cleanup also failed: {:?}", error.kind());
                } else {
                    panic!("fixture cleanup failed: {:?}", error.kind());
                }
            }
        }
    }
}

#[test]
fn m7_svn_working_copy_is_readable_but_native_mutation_is_rejected() {
    let h = Harness::new();
    fs::create_dir(h.root.join(".svn")).unwrap();
    let project = h.open();
    let status = h.call(json!({"action":"project_status","project":project}));
    assert_eq!(status["collaborative"], true);
    let canonical = h.root.canonicalize().unwrap();
    assert_eq!(h.state.svn_update_admission(&canonical), Ok(()));
    let listing = h.work(json!({"kind":"list_templates","project":project}));
    assert_eq!(listing["kind"], "templates");
    let operation = h.reserve("ordinary");
    let rejected = h
        .ipc(json!({
            "action":"submit",
            "operation":operation,
            "input":{"kind":"create_template","project":project,"name":"blocked","presentation":null}
        }))
        .unwrap_err();
    assert_eq!(rejected["code"], "collaboration_read_only");
    h.close_clean();
}

#[test]
fn m76_fix001_force_permit_and_project_close_share_native_lifetime() {
    let base =
        std::env::temp_dir().join(format!("worldbuild-m76-fix-force-{}", uuid::Uuid::new_v4()));
    let root = base.join("project");
    fs::create_dir_all(root.join(".svn")).unwrap();
    let manager = crate::svn::Manager::new(base.join("config"));
    let h = Harness::owned_svn(base, manager);
    let project = h.open();
    let document = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let mut permit = h.state.begin_force(&root, document.into()).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let force = thread::spawn(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(LIMIT).unwrap();
        let result = Err("svn_lock_unverified".to_owned());
        permit.finish(&result);
    });
    entered_rx.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.state.begin_force(&root, document.into()).err(),
        Some("svn_busy")
    );
    let close = h.work(json!({"kind":"close","project":project}));
    assert_eq!(close["kind"], "control");
    assert_eq!(
        h.state.begin_force(&root, document.into()).err(),
        Some("svn_close_project_first")
    );
    let during = h.call(json!({"action":"project_status","project":project}));
    assert_eq!(during["shutdown"]["forceActive"], true);
    assert_eq!(during["shutdown"]["normalExitAllowed"], false);
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    release_tx.send(()).unwrap();
    force.join().unwrap();
    let after = h.call(json!({"action":"project_status","project":project}));
    assert_eq!(after["shutdown"]["forceActive"], false);
    assert_eq!(after["shutdown"]["forceResults"][0]["outcome"], "unknown");
    assert_eq!(after["shutdown"]["forceResults"][0]["document"], document);
    assert!(!h.state.poll());
    h.call(json!({"action":"acknowledge_shutdown","project":project}));
    h.close_clean();
}

#[test]
#[ignore = "run explicitly with installed SVN CLI and an owned FSFS fixture"]
fn m76_fix001_force_close_barrier_preserves_real_server_outcome() {
    use std::process::Command;
    let cli = PathBuf::from(
        std::env::var("M7_SVN_TEST_CLI")
            .unwrap_or_else(|_| r"C:\Program Files\TortoiseSVN\bin\svn.exe".into()),
    );
    let admin = cli.with_file_name("svnadmin.exe");
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("logs")
        .join("M7-6-FIX-001")
        .join("fixtures")
        .join(format!("force-close-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let base = PathBuf::from(
        base.canonicalize()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    );
    let repo = base.join("repo");
    let first = base.join("first");
    let second = base.join("project");
    let execute = |binary: &Path, args: &[&str]| {
        let output = Command::new(binary).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "owned FSFS command {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    execute(&admin, &["create", repo.to_str().unwrap()]);
    let url = url::Url::from_directory_path(&repo).unwrap().to_string();
    execute(&cli, &["checkout", &url, first.to_str().unwrap()]);
    let document = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let file = first.join(format!("documents/{document}.json"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, br#"{"name":"owned force close"}"#).unwrap();
    execute(&cli, &["add", first.join("documents").to_str().unwrap()]);
    execute(
        &cli,
        &["propset", "svn:needs-lock", "yes", file.to_str().unwrap()],
    );
    execute(
        &cli,
        &["commit", "-m", "owned fixture", first.to_str().unwrap()],
    );
    execute(&cli, &["checkout", &url, second.to_str().unwrap()]);
    execute(&cli, &["lock", "-m", "first owner", file.to_str().unwrap()]);
    let manager = crate::svn::Manager::new(base.join("config"));
    assert!(crate::svn::probe(&manager, Some(cli.to_string_lossy().into_owned()), None).installed);
    manager.set_owned_fsfs_identity(url.clone(), "second-owner");
    let h = Harness::owned_svn(base, manager);
    let project = h.open();
    let provider = h.state.svn_commit_provider(&second).unwrap();
    let before = provider.document_lock_owner(document).unwrap();
    let owner = before.owner.unwrap();
    let original_owner = owner.clone();
    let observation = before.observation.unwrap();
    let mut permit = h.state.begin_force(&second, document.into()).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let force = thread::spawn(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(LIMIT).unwrap();
        let result = permit.provider.force_document_lock(
            document,
            &owner,
            &observation,
            "second-owner",
            "owned close barrier",
        );
        permit.finish(&result);
        result
    });
    entered_rx.recv_timeout(LIMIT).unwrap();
    h.work(json!({"kind":"close","project":project}));
    assert_eq!(
        h.state.begin_force(&second, document.into()).err(),
        Some("svn_close_project_first")
    );
    assert_eq!(
        h.call(json!({"action":"project_status","project":project}))["shutdown"]["forceActive"],
        true
    );
    assert_eq!(
        provider
            .document_lock_owner(document)
            .unwrap()
            .owner
            .as_deref(),
        Some(original_owner.as_str())
    );
    release_tx.send(()).unwrap();
    assert_eq!(force.join().unwrap(), Ok(()));
    let after = h.call(json!({"action":"project_status","project":project}));
    assert_eq!(after["shutdown"]["forceResults"][0]["outcome"], "verified");
    assert_eq!(
        provider
            .document_lock_owner(document)
            .unwrap()
            .owner
            .as_deref(),
        Some("second-owner")
    );
    h.call(json!({"action":"acknowledge_shutdown","project":project}));
    h.close_clean();
}

#[test]
#[ignore = "run with owned M7_B_NESTED_WC and installed SVN CLI"]
fn m7_owned_nested_svn_document_edit_begin_uses_real_lock() {
    let base = PathBuf::from(std::env::var("M7_B_NESTED_WC").unwrap());
    let manager = crate::svn::Manager::new(
        std::env::temp_dir().join(format!("worldbuild-m7-b-app-{}", uuid::Uuid::new_v4())),
    );
    let h = Harness::owned_svn(base, manager);
    let project = h.open();
    let result = h.work(json!({
        "kind":"document_workspace",
        "project":project,
        "request":{"action":"edit_begin","document":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}
    }));
    let editing = result["value"].clone();
    if editing["kind"] == "editing" {
        let _ = h.work(json!({
            "kind":"document_workspace",
            "project":project,
            "request":{"action":"edit_release","owner":editing["owner"],"generation":editing["generation"]}
        }));
    }
    h.close_clean();
    assert_eq!(editing["kind"], "editing", "{result}");
    assert!(editing["problem"].is_null(), "{result}");
}

#[test]
#[ignore = "run with owned M7_FIX_STATE_WC, M7_FIX_LOCK_WC and installed SVN CLI"]
fn m7_other_working_copy_lock_keeps_document_in_read_mode() {
    use crate::data::{
        collaboration_lock::{LockService, LockSessionId},
        project_relative_path::ProjectRelativePath,
    };
    let base = PathBuf::from(std::env::var("M7_FIX_STATE_WC").unwrap());
    let lock_copy = PathBuf::from(std::env::var("M7_FIX_LOCK_WC").unwrap());
    let manager = crate::svn::Manager::new(std::env::temp_dir().join(format!(
        "worldbuild-m7-fix-app-lock-{}",
        uuid::Uuid::new_v4()
    )));
    assert!(
        crate::svn::probe(
            &manager,
            Some(std::env::var("M7_SVN_TEST_CLI").unwrap()),
            None,
        )
        .installed
    );
    let target =
        ProjectRelativePath::parse("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json").unwrap();
    let locker = crate::svn::SvnLockService::new(manager.clone(), lock_copy);
    let mut held = locker
        .acquire(
            LockAcquireRequest::new(
                &"a".repeat(64),
                &LockSessionId::generate().unwrap(),
                &target,
            )
            .unwrap(),
        )
        .unwrap();
    let h = Harness::owned_svn(base, manager);
    let project = h.open();
    let result = h.work(json!({
        "kind":"document_workspace",
        "project":project,
        "request":{"action":"edit_begin","document":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}
    }));
    h.close_clean();
    locker.release(held.as_mut()).unwrap();
    assert_eq!(result["kind"], "rejected", "{result}");
    assert_eq!(result["error"]["code"], "session_rejected", "{result}");
}

#[test]
fn m7_personal_project_is_not_admitted_to_live_svn_update() {
    let h = Harness::new();
    h.open();
    assert_eq!(
        h.state
            .svn_update_admission(&h.root.canonicalize().unwrap()),
        Err("svn_close_project_first")
    );
    h.close_clean();
}

#[test]
fn g13_i1_production_registry_acl_and_safe_raw_decoder() {
    fn require<T: Send + Sync>() {}
    require::<AppState>();
    require::<Arc<AppState>>();
    let h = Harness::new();
    assert_eq!(h.call(json!({"action":"app_status"}))["kind"], "app");
    let other = tauri::WebviewWindowBuilder::new(&h.app, "foreign", Default::default())
        .build()
        .unwrap();
    let body = serde_json::to_vec(&json!({"action":"app_status"})).unwrap();
    assert!(tauri::test::get_ipc_response(
        &other,
        Harness::request("guarded", body.clone(), "http://tauri.localhost")
    )
    .is_err());
    assert!(tauri::test::get_ipc_response(
        &h.main,
        Harness::request("guarded", body.clone(), "https://untrusted.example")
    )
    .is_err());
    assert!(tauri::test::get_ipc_response(
        &h.main,
        Harness::request("unknown_command", body, "http://tauri.localhost")
    )
    .is_err());
    for body in [
        json!({"action":"G13_PRIVATE_SENTINEL"}),
        json!({"action":"operation","operation":"G13_PRIVATE_SENTINEL"}),
        json!({"action":"app_status","extra":"G13_PRIVATE_SENTINEL"}),
    ] {
        let e = h.ipc(body).unwrap_err();
        assert_eq!(e["code"], "invalid_input");
        assert!(!e.to_string().contains("G13_PRIVATE_SENTINEL"));
    }
    let e = tauri::test::get_ipc_response(
        &h.main,
        Harness::request(
            "guarded",
            b"{invalid G13_PRIVATE_SENTINEL".to_vec(),
            "http://tauri.localhost",
        ),
    )
    .err()
    .unwrap();
    assert_eq!(e["code"], "invalid_input");
    assert!(!e.to_string().contains("PRIVATE"));
    h.close_clean();
}

#[test]
fn m39_fix_support_diagnostic_changes_wake_once_and_stop_after_close() {
    const FEATURE: &str = "youtube_handler_state_test";
    crate::support_diagnostics::clear(FEATURE);
    let h = Harness::new();
    let start: u64 = h.call(json!({"action":"app_status"}))["generation"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    assert!(crate::support_diagnostics::record(
        FEATURE,
        "request_filter",
        "webview2_com",
        Some("0x80070005".into()),
    ));
    assert!(h.state.support_diagnostics_changed());
    h.wake.recv_timeout(LIMIT).unwrap();
    let updated = h.call(json!({"action":"app_status"}));
    assert_eq!(
        updated["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        start + 1
    );
    assert!(updated["support_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["feature"] == FEATURE));

    assert!(crate::support_diagnostics::clear(FEATURE));
    assert!(h.state.support_diagnostics_changed());
    h.wake.recv_timeout(LIMIT).unwrap();
    let cleared = h.call(json!({"action":"app_status"}));
    assert!(cleared["support_diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["feature"] != FEATURE));

    h.state.lock().closed = true;
    let before_late = h.state.generation();
    crate::support_diagnostics::record(FEATURE, "handler_registration", "webview2_com", None);
    assert!(!h.state.support_diagnostics_changed());
    assert_eq!(h.state.generation(), before_late);
    assert!(h.wake.try_recv().is_err());
    crate::support_diagnostics::clear(FEATURE);
}

#[test]
fn m28_framework_command_reflection_stays_outside_app_diagnostics() {
    let h = Harness::new();
    let before = h.call(json!({"action":"app_status"}));
    let sentinel = "M28_CALLER_OWNED_COMMAND_SENTINEL";
    let foreign = tauri::WebviewWindowBuilder::new(&h.app, "foreign", Default::default())
        .build()
        .unwrap();
    for (window, origin) in [
        (&h.main, "http://tauri.localhost"),
        (&foreign, "http://tauri.localhost"),
        (&h.main, "https://untrusted.example"),
    ] {
        let error = tauri::test::get_ipc_response(
            window,
            Harness::request(sentinel, br#"{"action":"app_shutdown"}"#.to_vec(), origin),
        )
        .unwrap_err();
        // 의존성의 원 반사를 사실대로 고정한다. 이 통과는 반사 해결의 증거가 아니다.
        assert!(error.to_string().contains(sentinel));
        if origin != "http://tauri.localhost" || window.label() == "foreign" {
            assert!(tauri::test::get_ipc_response(
                window,
                Harness::request("guarded", br#"{"action":"app_shutdown"}"#.to_vec(), origin)
            )
            .is_err());
        }
    }
    assert!(h.call(json!({"action":"app_status"})) == before);
    let error = h.ipc(json!({"action":sentinel})).unwrap_err();
    assert_eq!(error["code"], "invalid_input");
    assert!(!error.to_string().contains(sentinel));
    let rejected = dto::decode(&serde_json::to_vec(&json!({"action":sentinel})).unwrap())
        .err()
        .unwrap();
    assert!(!format!("{rejected:?}").contains(sentinel));
    h.close_clean();
}

#[test]
fn g13_i2_real_project_view_session_and_operation_bindings() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let read = h.read_template(&p, &template);
    let session = h.session(&p, vec![read["view"].clone()], "template");
    let second = h.base.join("second");
    fs::create_dir(&second).unwrap();
    let p2 = h.work(json!({"kind":"open","root":second}))["project"]
        .as_str()
        .unwrap()
        .to_owned();
    let op = h.reserve("ordinary");
    let error=h.ipc(json!({"action":"submit","operation":op,"input":{"kind":"update_template","project":p2,"session":session,"view":read["view"],"revision":"1","edit":{"kind":"name","name":"wrong"}}})).unwrap_err();
    assert_eq!(error["code"], "unknown_id");
    let error = h
        .ipc(json!({"action":"release_view","project":p,"view":read["view"]}))
        .unwrap_err();
    assert_eq!(error["code"], "owners_remain");
    let caller = h.state.caller("main", "main").unwrap();
    assert!(h
        .state
        .dispatch(Caller(Id::new()), Command::AppStatus {})
        .is_err());
    assert!(h.state.caller("foreign", "main").is_err());
    assert!(h.state.dispatch(caller, Command::AppStatus {}).is_ok());
    h.close_clean();
}

#[test]
fn g13_i3_g6_create_update_duplicate_tombstone_and_reference_block() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let read = h.read_template(&p, &template);
    let session = h.session(&p, vec![read["view"].clone()], "template");
    let r=h.work(json!({"kind":"update_template","project":p,"session":session,"view":read["view"],"revision":"1","edit":{"kind":"name","name":"renamed"}}));
    assert_eq!(r["disk"], "committed");
    let bytes = fs::read(h.root.join(format!("templates/{template}.json"))).unwrap();
    let disk = artifact::decode_template(&bytes).unwrap();
    assert_eq!(disk.name(), "renamed");
    assert_eq!(disk.revision().get(), 2);
    let read = h.read_template(&p, &template);
    let duplicate = h.work(json!({"kind":"duplicate_template","project":p,"view":read["view"]}));
    assert_eq!(duplicate["disk"], "committed");
    assert_ne!(duplicate["artifact"], template);
    let copy = h.read_template(&p, duplicate["artifact"].as_str().unwrap());
    let copy_session = h.session(&p, vec![copy["view"].clone()], "template");
    let tombstone=h.work(json!({"kind":"tombstone_template","project":p,"session":copy_session,"view":copy["view"],"revision":"1"}));
    assert_eq!(tombstone["disk"], "committed");
    let document =
        h.work(json!({"kind":"create_document","project":p,"view":read["view"],"name":"created"}));
    assert_eq!(document["disk"], "committed");
    let active_session = h.session(&p, vec![read["view"].clone()], "template");
    let blocked_id=h.submit(json!({"kind":"tombstone_template","project":p,"session":active_session,"view":read["view"],"revision":"2"}));
    let blocked = h.result(&blocked_id);
    assert_eq!(blocked["disk"], "not_attempted");
    assert_eq!(blocked["error"]["code"], "save_rejected");
    let listing = h.work(json!({"kind":"list_templates","project":p}));
    assert_eq!(listing["templates"].as_array().unwrap().len(), 2);
    let retained = h.call(json!({"action":"operation","operation":blocked_id}))["retained"].clone();
    h.ack(&blocked_id);
    // 보존된 tombstone 의도도 원 사용자의 명시적 선택으로만 정리한다.
    let abandon = h.reserve("control");
    h.call(json!({"action":"submit","operation":abandon,"input":{"kind":"abandon_retained","retained":retained}}));
    assert_eq!(h.result(&abandon)["action"], "abandoned");
    h.ack(&abandon);
    h.close_clean();
}

#[test]
fn g13_i3_document_materialize_and_dirty_capability_retain_input() {
    let h = Harness::new();
    recovery::unavailable(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let t = h.read_template(&p, &template);
    let created =
        h.work(json!({"kind":"create_document","project":p,"view":t["view"],"name":"document"}));
    let document = created["artifact"].as_str().unwrap();
    let d = h.work(json!({"kind":"read_document","project":p,"document":document}));
    let s = h.session(&p, vec![d["view"].clone(), t["view"].clone()], "document");
    let before = fs::read(h.root.join(format!("documents/{document}.json"))).unwrap();
    let materialized=h.work(json!({"kind":"materialize_document","project":p,"session":s,"document":d["view"],"template":t["view"],"revision":"1"}));
    assert_eq!(materialized["disk"], "no_write");
    assert_eq!(materialized["changed"], false);
    let input = json!({"kind":"save_document","project":p,"session":s,"document":d["view"],"template":t["view"],"revision":"1","edits":[{"kind":"rename","name":"G13_PRIVATE_TYPED_INPUT"}]});
    let operation = h.submit(input);
    let result = h.result(&operation);
    assert_eq!(result["error"]["code"], "sink_unavailable");
    h.ack(&operation);
    assert_eq!(h.state.lock().retained.len(), 1);
    assert_eq!(
        before,
        fs::read(h.root.join(format!("documents/{document}.json"))).unwrap()
    );
    let status = h.call(json!({"action":"session_status","project":p,"session":s}));
    assert_eq!(status["active_dirty"], true);
    assert!(!status.to_string().contains("PRIVATE"));
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    let returned = h.control(
        json!({"kind":"session_control","project":p,"session":s,"control":"return_active"}),
    );
    assert!(returned["error"].is_null());
    assert!(!h.state.poll());
    // 앱 owner를 테스트가 인수해 검사한다. 운영에 편집 폐기/가짜 durable 완료 API를 노출하지 않는다.
    h.state.lock().retained.clear();
    let end = Instant::now() + LIMIT;
    loop {
        h.state.poll();
        let mut st = h.state.lock();
        let project = st.projects.values_mut().next().unwrap();
        project.reports.clear();
        if project.joined {
            assert!(!project.control.shutdown_snapshot().normal_exit_allowed);
            break;
        }
        drop(st);
        assert!(Instant::now() < end);
        thread::yield_now();
    }
}

#[test]
fn g13_i4_lossless_dto_validation_and_set_unset_conflict() {
    for value in [
        "9007199254740993",
        "-0",
        "0.12345678901234567890123456789",
        "1E100",
    ] {
        let raw = json!({"action":"submit","operation":String::from(Id::new()),"input":{"kind":"save_document","project":String::from(Id::new()),"session":String::from(Id::new()),"document":String::from(Id::new()),"template":String::from(Id::new()),"revision":"4294967295","edits":[{"kind":"set","field":"99999999-9999-4999-8999-000000000001","value":{"kind":"number","value":value}}]}});
        let Command::Submit { input, .. } =
            dto::decode(&serde_json::to_vec(&raw).unwrap()).unwrap()
        else {
            panic!("typed input")
        };
        let Work::SaveDocument {
            edits, revision, ..
        } = *input
        else {
            panic!("document input")
        };
        assert_eq!(revision, "4294967295");
        let dto::DocumentEdit::Set {
            value: dto::ValueDto::Number { value: actual },
            ..
        } = &edits[0]
        else {
            panic!("number string")
        };
        assert_eq!(actual, value);
    }
    let field = "99999999-9999-4999-8999-000000000001".to_owned();
    let edits = vec![
        dto::DocumentEdit::Unset {
            field: field.clone(),
        },
        dto::DocumentEdit::Set {
            field,
            value: dto::ValueDto::Number { value: "42".into() },
        },
    ];
    assert!(commands::test_edits(&edits).is_err());
}

#[test]
fn g13_i5_ipc_response_loss_requery_and_duplicate_do_not_repeat_write() {
    let h = Harness::new();
    let p = h.open();
    let operation = h.reserve("ordinary");
    let input =
        json!({"kind":"create_template","project":p,"name":"only once","presentation":null});
    let submit = json!({"action":"submit","operation":operation,"input":input});
    let (tx, rx) = mpsc::channel();
    drop(rx);
    h.main.as_ref().clone().on_message(
        Harness::request(
            "guarded",
            serde_json::to_vec(&submit).unwrap(),
            "http://tauri.localhost",
        ),
        Box::new(move |_, _, response, _, _| {
            let _ = tx.send(response);
        }),
    );
    let original = h.result(&operation);
    assert_eq!(original["disk"], "committed");
    h.call(submit);
    assert_eq!(h.result(&operation), original);
    let conflict=h.ipc(json!({"action":"submit","operation":operation,"input":{"kind":"create_template","project":p,"name":"different","presentation":null}})).unwrap_err();
    assert_eq!(conflict["code"], "duplicate_conflict");
    h.main.as_ref().clone().on_message(
        Harness::request(
            "guarded",
            serde_json::to_vec(&json!({"action":"operation","operation":operation})).unwrap(),
            "http://tauri.localhost",
        ),
        Box::new(|_, _, _, _, _| {}),
    );
    assert_eq!(h.result(&operation), original);
    assert_eq!(fs::read_dir(h.root.join("templates")).unwrap().count(), 1);
    h.ack(&operation);
    assert!(h
        .ipc(json!({"action":"operation","operation":operation}))
        .is_err());
    h.close_clean();
}

#[test]
fn g13_i6_completed_results_bound_admission_but_control_progresses() {
    let h = Harness::new();
    let p = h.open();
    let mut ids = Vec::new();
    for _ in 0..ORDINARY {
        let id = h.submit(json!({"kind":"list_templates","project":p}));
        h.result(&id);
        ids.push(id);
    }
    assert_eq!(
        h.ipc(json!({"action":"reserve","lane":"ordinary"}))
            .unwrap_err()["code"],
        "full"
    );
    assert_eq!(
        h.call(json!({"action":"project_status","project":p}))["status"],
        "Ready"
    );
    let close = h.submit(json!({"kind":"close","project":p}));
    h.result(&close);
    assert!(!h.state.poll());
    h.state.event_failure(Code::EventDelivery);
    for id in ids {
        assert_eq!(h.result(&id)["kind"], "templates");
        h.ack(&id);
    }
    h.ack(&close);
    h.close_clean();
}

#[test]
fn g13_i8_pending_open_and_multi_project_shutdown_own_every_worker() {
    let h = Harness::new();
    let first = h.submit(json!({"kind":"open","root":h.root}));
    let root2 = h.base.join("second");
    fs::create_dir(&root2).unwrap();
    let second = h.submit(json!({"kind":"open","root":root2}));
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    assert_eq!(h.state.lock().projects.len(), 2);
    assert_eq!(
        h.ipc(json!({"action":"reserve","lane":"ordinary"}))
            .unwrap_err()["code"],
        "closed"
    );
    h.result(&first);
    h.result(&second);
    h.ack(&first);
    h.ack(&second);
    h.close_clean();
    assert!(h.state.take_exit_approval());
    assert!(!h.state.take_exit_approval());
}

#[test]
fn g13_i10_empty_app_close_revokes_late_admission() {
    let h = Harness::new();
    let blank = h.reserve("ordinary");
    h.close_clean();
    assert!(h.state.exit_approved());
    assert_eq!(
        h.ipc(json!({"action":"submit","operation":blank,"input":{"kind":"open","root":h.root}}))
            .unwrap_err()["code"],
        "closed"
    );
}

#[test]
fn g13_i5_actual_submit_cancellation_before_worker_completion() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (entered, release) = provider.hold();
    let operation = h.reserve("ordinary");
    let input = json!({"action":"submit","operation":operation,"input":{"kind":"create_template","project":p,"name":"held","presentation":null}});
    h.main.as_ref().clone().on_message(
        Harness::request(
            "guarded",
            serde_json::to_vec(&input).unwrap(),
            "http://tauri.localhost",
        ),
        Box::new(|_, _, _, _, _| {}),
    );
    entered.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.call(json!({"action":"operation","operation":operation}))["state"],
        "pending"
    );
    assert_eq!(
        h.ipc(json!({"action":"acknowledge_transport","operation":operation}))
            .unwrap_err()["code"],
        "not_terminal"
    );
    release.send(()).unwrap();
    let result = h.result(&operation);
    assert_eq!(result["disk"], "committed");
    h.ack(&operation);
    h.close_clean();
    assert!(provider
        .threads
        .lock()
        .unwrap()
        .iter()
        .all(|id| *id != thread::current().id()));
}

#[test]
fn g13_i7_foreground_native_resume_coalesce_running_followup() {
    use std::sync::atomic::Ordering;
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (_, session) = h.template(&p);
    let before = provider.validations.load(Ordering::SeqCst);
    let (entered, release) = provider.hold();
    let event = tauri::WindowEvent::Focused(true);
    crate::events::window_event(&h.state, "main", &event);
    entered.recv_timeout(LIMIT).unwrap();
    for _ in 0..100 {
        assert!(crate::platform::dispatch_power(&h.state, 0x218, 0x12));
        crate::events::window_event(&h.state, "main", &event);
    }
    assert!(!crate::platform::dispatch_power(&h.state, 0x218, 0x4));
    release.send(()).unwrap();
    let end = Instant::now() + LIMIT;
    loop {
        let state = h.state.lock();
        let project = state.projects.get(&p.clone().try_into().unwrap()).unwrap();
        let key = project
            .sessions
            .get(&session.clone().try_into().unwrap())
            .unwrap()
            .key
            .as_ref()
            .unwrap();
        if project.control.revalidation(key).unwrap().completed == 2 {
            break;
        }
        drop(state);
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    assert_eq!(provider.validations.load(Ordering::SeqCst), before + 2);
    h.close_clean();
    crate::platform::dispatch_power(&h.state, 0x218, 0x12);
    assert_eq!(provider.validations.load(Ordering::SeqCst), before + 2);
}

#[test]
fn g13_i2_pending_init_is_owned_before_close() {
    let h = Harness::new();
    let (gate, owner) = gates::gate();
    gates::hold_init(h.root.clone(), gate);
    let operation = h.submit(json!({"kind":"open","root":h.root}));
    owner.entered();
    let project = *h.state.lock().projects.keys().next().unwrap();
    assert_eq!(
        h.call(json!({"action":"project_status","project":String::from(project)}))["status"],
        "Starting"
    );
    let denied = h.work(json!({"kind":"list_templates","project":String::from(project)}));
    assert_eq!(denied["error"]["code"], "starting");
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    owner.release();
    h.result(&operation);
    h.ack(&operation);
    h.close_clean();
}

#[test]
fn g13_i9_close_result_owner_precedes_pending_refresh_and_join() {
    let h = Harness::new();
    let p = h.open();
    let (gate, owner) = gates::gate();
    let client = h
        .state
        .lock()
        .projects
        .values()
        .next()
        .unwrap()
        .client
        .clone();
    let ticket = client
        .try_submit(gate, |_, gate| gates::hold_after_publish(1, gate))
        .unwrap();
    let end = Instant::now() + LIMIT;
    while client.try_take(&ticket).unwrap().is_none() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    let close = h.submit(json!({"kind":"close","project":p}));
    h.result(&close);
    h.ack(&close);
    owner.entered();
    h.state.poll();
    {
        let state = h.state.lock();
        let project = state.projects.values().next().unwrap();
        assert!(project.reports.iter().any(|r| r.close.is_some()));
        assert!(!project.control.shutdown_snapshot().stop_requested);
        assert!(!project.joined);
        assert!(!state.approved);
    }
    owner.release();
    h.close_clean();
}

#[test]
fn g13_i7_native_hook_registration_power_message_and_removal() {
    use windows::{
        core::w,
        Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE,
            },
        },
    };
    let h = Harness::new();
    assert_eq!(
        crate::platform::install(HWND::default(), h.state.clone())
            .unwrap_err()
            .code,
        Code::PlatformRegistration
    );
    // 테스트 소유의 숨겨진 Win32 창에 실제 subclass를 등록한다. OS suspend는 실행하지 않는다.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("G13 power hook fixture"),
            WINDOW_STYLE::default(),
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    crate::platform::install(hwnd, h.state.clone()).unwrap();
    assert!(crate::platform::install(hwnd, h.state.clone()).is_err());
    let before = h.state.generation();
    unsafe {
        SendMessageW(hwnd, 0x218, Some(WPARAM(0x12)), Some(LPARAM(0)));
    }
    assert_ne!(h.state.generation(), before);
    crate::platform::remove().unwrap();
    let before = h.state.generation();
    unsafe {
        SendMessageW(hwnd, 0x218, Some(WPARAM(0x12)), Some(LPARAM(0)));
    }
    assert_eq!(h.state.generation(), before);
    unsafe { DestroyWindow(hwnd) }.unwrap();
    h.close_clean();
}

fn seeded(h: &Harness) -> (String, String) {
    let (t, d) = crate::data::application::composite::tests::guarded_fixture_bytes();
    let template = artifact::decode_template(&t)
        .unwrap()
        .template_id()
        .to_string();
    let document = artifact::decode_document(&d)
        .unwrap()
        .document_id()
        .to_string();
    fs::create_dir_all(h.root.join("templates")).unwrap();
    fs::create_dir_all(h.root.join("documents")).unwrap();
    fs::write(h.root.join(format!("templates/{template}.json")), t).unwrap();
    fs::write(h.root.join(format!("documents/{document}.json")), d).unwrap();
    (template, document)
}
#[test]
fn g13_i3_i4_historical_materialize_changed_unchanged_warnings_and_unknown_bytes() {
    let h = Harness::new();
    let (template, document) = seeded(&h);
    let p = h.open();
    let old = h.read_template(&p, &template);
    let read = h.work(json!({"kind":"read_document","project":p,"document":document}));
    assert_eq!(read["content"]["templateRevision"], "3");
    assert!(!read.to_string().contains("__snapshot__"));
    let template_session = h.session(&p, vec![old["view"].clone()], "template");
    assert_eq!(h.work(json!({"kind":"update_template","project":p,"session":template_session,"view":old["view"],"revision":"3","edit":{"kind":"name","name":"revision four"}}))["disk"],"committed");
    let stale_session = h.session(
        &p,
        vec![read["view"].clone(), old["view"].clone()],
        "document",
    );
    let before = fs::read(h.root.join(format!("documents/{document}.json"))).unwrap();
    let stale=h.work(json!({"kind":"materialize_document","project":p,"session":stale_session,"document":read["view"],"template":old["view"],"revision":"3"}));
    assert_ne!(stale["disk"], "committed");
    assert_eq!(
        before,
        fs::read(h.root.join(format!("documents/{document}.json"))).unwrap()
    );
    let current = h.read_template(&p, &template);
    let session = h.session(
        &p,
        vec![read["view"].clone(), current["view"].clone()],
        "document",
    );
    let changed=h.work(json!({"kind":"materialize_document","project":p,"session":session,"document":read["view"],"template":current["view"],"revision":"4"}));
    assert_eq!(changed["disk"], "committed");
    assert_eq!(changed["changed"], true);
    assert!(!changed["warnings"].as_array().unwrap().is_empty());
    let bytes = fs::read(h.root.join(format!("documents/{document}.json"))).unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    for token in ["1E100", "1e100", "-0", "0.12345678901234567890123456789"] {
        assert!(raw.contains(token));
    }
    assert_eq!(
        artifact::decode_document(&bytes)
            .unwrap()
            .template_revision()
            .get(),
        4
    );
    let read = h.work(json!({"kind":"read_document","project":p,"document":document}));
    let session = h.session(
        &p,
        vec![read["view"].clone(), current["view"].clone()],
        "document",
    );
    let unchanged=h.work(json!({"kind":"materialize_document","project":p,"session":session,"document":read["view"],"template":current["view"],"revision":"4"}));
    assert_eq!(unchanged["disk"], "no_write");
    assert_eq!(unchanged["changed"], false);
    assert!(!unchanged["warnings"].as_array().unwrap().is_empty());
    assert_eq!(
        bytes,
        fs::read(h.root.join(format!("documents/{document}.json"))).unwrap()
    );
    h.close_clean();
}

struct LimitedSink {
    owner: Arc<Mutex<Option<PendingEdit>>>,
}
impl crate::data::edit_session::DurableRecoverySink<PendingEdit> for LimitedSink {
    type Receipt = Receipt;
    fn accept_durably(
        &mut self,
        _: &crate::data::edit_session::RecoveryEnvelope,
        payload: PendingEdit,
    ) -> Result<Receipt, crate::data::edit_session::RecoverySinkFailure<PendingEdit>> {
        // 소유권 전이만 검사하는 제한된 test adapter. 운영 파일 형식/내구성 구현을 뜻하지 않는다.
        let mut owner = self.owner.lock().unwrap();
        assert!(owner.is_none());
        *owner = Some(payload);
        Ok(Receipt::from_test_sink(Box::new(self.owner.clone())))
    }
}
fn r1_handoff(preserve_first: bool) {
    let h = Harness::new();
    recovery::unavailable(&h);
    let (template, document) = seeded(&h);
    let p = h.open();
    let t = h.read_template(&p, &template);
    let d = h.work(json!({"kind":"read_document","project":p,"document":document}));
    let s = h.session(&p, vec![d["view"].clone(), t["view"].clone()], "document");
    let rejected=h.work(json!({"kind":"save_document","project":p,"session":s,"document":d["view"],"template":t["view"],"revision":"3","edits":[{"kind":"rename","name":"G13_R1_ORIGINAL"}]}));
    assert_eq!(rejected["error"]["code"], "sink_unavailable");
    let input = h.state.lock().retained[0].0.clone();
    let sink = Arc::new(Mutex::new(None));
    let (client, key) = {
        let st = h.state.lock();
        let project = st.projects.values().next().unwrap();
        (
            project.client.clone(),
            project
                .sessions
                .get(&s.clone().try_into().unwrap())
                .unwrap()
                .key
                .clone()
                .unwrap(),
        )
    };
    let ticket = client
        .try_submit((key, sink.clone()), |ctx, (key, sink)| {
            assert!(ctx
                .session(&key)
                .unwrap()
                .connect_backend(
                    crate::data::application::recovery_handoff::RecoveryBackend::connected(
                        LimitedSink { owner: sink }
                    )
                )
                .is_ok());
        })
        .unwrap();
    let end = Instant::now() + LIMIT;
    while client.try_take(&ticket).unwrap().is_none() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    let obstacle = h.root.join(".worldbuild");
    assert!(!obstacle.exists());
    fs::write(&obstacle, b"test recovery obstacle").unwrap();
    let recovery = h.control(json!({"kind":"recover","project":p}));
    assert_eq!(recovery["error"]["code"], "recovery_rejected");
    if preserve_first {
        assert!(h.control(
            json!({"kind":"session_control","project":p,"session":s,"control":"preserve"})
        )["error"]
            .is_null());
    }
    h.call(json!({"action":"app_shutdown"}));
    if !preserve_first {
        let end = Instant::now() + LIMIT;
        loop {
            h.state.poll();
            let st = h.state.lock();
            let project = st.projects.values().next().unwrap();
            let view = project.control.shutdown_snapshot();
            if view.sessions.iter().any(|s| s.awaiting_custody) {
                assert!(!view.stop_requested);
                assert!(project
                    .control
                    .session_observation(&project.sessions.values().next().unwrap().registration)
                    .unwrap()
                    .snapshot
                    .session_id()
                    .is_some());
                break;
            }
            drop(st);
            assert!(Instant::now() < end);
            thread::yield_now();
        }
        assert!(h.control(
            json!({"kind":"session_control","project":p,"session":s,"control":"preserve"})
        )["error"]
            .is_null());
    }
    let status = h.call(json!({"action":"session_status","project":p,"session":s}));
    assert_eq!(status["custody"], "Pending");
    assert!(!h.state.poll());
    assert!(sink.lock().unwrap().is_none());
    assert!(h
        .control(json!({"kind":"session_control","project":p,"session":s,"control":"accept"}))
        ["error"]
        .is_null());
    assert!(Arc::ptr_eq(
        &sink.lock().unwrap().as_ref().unwrap().input,
        &input
    ));
    let (ack, result) = h.control_result(
        json!({"kind":"session_control","project":p,"session":s,"control":"acknowledge_recovery"}),
    );
    assert!(result["error"].is_null());
    assert!(h.state.lock().retained.is_empty());
    assert!(!h.state.poll());
    h.ack(&ack);
    fs::remove_file(&obstacle).unwrap();
    assert!(h.control(json!({"kind":"recover","project":p}))["error"].is_null());
    h.close_clean();
}
#[test]
fn g13_i9_r1_preserve_before_app_close_keeps_real_payload_and_receipt() {
    r1_handoff(true);
}
#[test]
fn g13_i9_r1_app_close_before_preserve_keeps_real_payload_and_receipt() {
    r1_handoff(false);
}

#[test]
fn g13_i2_alias_initialization_failure_retains_diagnostic_and_is_never_ready() {
    let h = Harness::new();
    let p = h.open();
    let alias = h.root.join(".");
    let result = h.work(json!({"kind":"open","root":alias}));
    assert_eq!(result["status"], "InitializationFailed");
    assert!(result["runtime"].is_null());
    assert_eq!(result["error"]["code"], "initialization_failed");
    let failed = result["project"].as_str().unwrap();
    assert_ne!(failed, p);
    let denied = h.work(json!({"kind":"list_templates","project":failed}));
    assert_eq!(denied["error"]["code"], "initialization_failed");
    h.call(json!({"action":"app_shutdown"}));
    let end = Instant::now() + LIMIT;
    loop {
        h.state.poll();
        if h.state.lock().projects.values().all(|p| p.joined) {
            break;
        }
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    let status = h.call(json!({"action":"project_status","project":failed}));
    assert_eq!(status["error"]["code"], "initialization_failed");
    assert!(status["shutdown"]["reports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["initializationFailed"] == true));
    assert!(!h.state.poll());
    assert!(!status
        .to_string()
        .contains(&h.base.to_string_lossy().to_string()));
    // G12의 init 실패 정상 종료 금지를 그대로 보존한다. 실제 thread는 join되었다.
}

#[test]
fn g13_i7_native_resume_lock_loss_keeps_original_and_never_restores_editing() {
    use std::sync::atomic::Ordering;
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (_, session) = h.template(&p);
    provider.lose.store(true, Ordering::SeqCst);
    crate::platform::dispatch_power(&h.state, 0x218, 0x12);
    let end = Instant::now() + LIMIT;
    let status = loop {
        let status = h.call(json!({"action":"project_status","project":p}));
        if !status["validation_failures"].as_array().unwrap().is_empty() {
            break status;
        }
        assert!(Instant::now() < end);
        thread::yield_now();
    };
    assert_eq!(status["validation_failures"][0]["lockCategory"], "LockLost");
    assert_eq!(
        h.state
            .lock()
            .projects
            .values()
            .next()
            .unwrap()
            .failures
            .len(),
        1
    );
    provider.lose.store(false, Ordering::SeqCst);
    crate::events::window_event(&h.state, "main", &tauri::WindowEvent::Focused(true));
    assert_eq!(
        h.call(json!({"action":"session_status","project":p,"session":session}))["state"],
        "LockLost"
    );
    h.call(json!({"action":"app_shutdown"}));
    let operation = h.submit(
        json!({"kind":"session_control","project":p,"session":session,"control":"revalidate"}),
    );
    let result = h.result(&operation);
    assert_eq!(result["kind"], "control");
    assert_eq!(result["error"]["code"], "closed");
    assert_eq!(h.result(&operation), result);
    h.ack(&operation);
    h.close_clean();
}

#[test]
fn g13_i3_composite_capability_rejection_owns_input_and_both_sources() {
    let h = Harness::new();
    recovery::unavailable(&h);
    let (template, document) = seeded(&h);
    let p = h.open();
    let t = h.read_template(&p, &template);
    let d = h.work(json!({"kind":"read_document","project":p,"document":document}));
    let session = h.session(&p, vec![d["view"].clone(), t["view"].clone()], "composite");
    let paths = [
        h.root.join(format!("templates/{template}.json")),
        h.root.join(format!("documents/{document}.json")),
    ];
    let before = paths
        .iter()
        .map(|p| fs::read(p).unwrap())
        .collect::<Vec<_>>();
    let result=h.work(json!({"kind":"save_composite","project":p,"session":session,"document":d["view"],"template":t["view"],"revision":"3","edit":{"kind":"name","name":"G13_COMPOSITE_ORIGINAL"},"edits":[{"kind":"rename","name":"G13_DOCUMENT_ORIGINAL"}]}));
    assert_eq!(result["error"]["code"], "sink_unavailable");
    assert_eq!(h.state.lock().retained.len(), 1);
    assert_eq!(
        paths
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    assert!(h.control(
        json!({"kind":"session_control","project":p,"session":session,"control":"return_active"})
    )["error"]
        .is_null());
    h.state.lock().retained.clear();
    let end = Instant::now() + LIMIT;
    loop {
        h.state.poll();
        let st = h.state.lock();
        if st.projects.values().all(|p| p.joined) {
            assert!(!st.approved);
            break;
        }
        drop(st);
        assert!(Instant::now() < end);
        thread::yield_now();
    }
}

#[test]
fn g13_i8_close_during_real_write_drains_late_result_and_prevents_early_exit() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (entered, release) = provider.hold();
    let operation = h.submit(
        json!({"kind":"create_template","project":p,"name":"drained create","presentation":null}),
    );
    entered.recv_timeout(LIMIT).unwrap();
    h.call(json!({"action":"app_shutdown"}));
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    assert_eq!(
        h.call(json!({"action":"operation","operation":operation}))["state"],
        "pending"
    );
    release.send(()).unwrap();
    let result = h.result(&operation);
    assert_eq!(result["disk"], "committed");
    assert!(!h.state.poll());
    assert_eq!(fs::read_dir(h.root.join("templates")).unwrap().count(), 1);
    h.ack(&operation);
    h.close_clean();
    assert!(h
        .state
        .lock()
        .projects
        .values()
        .all(|p| p.control.shutdown_snapshot().round == 1));
}

#[test]
fn g13_i4_typed_field_number_default_survives_actual_write_and_read() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let read = h.read_template(&p, &template);
    let session = h.session(&p, vec![read["view"].clone()], "template");
    let lexeme = "9007199254740993.1234567890123456789";
    let result=h.work(json!({"kind":"update_template","project":p,"session":session,"view":read["view"],"revision":"1","edit":{"kind":"create_field","field":"99999999-9999-4999-8999-000000000001","label":"exact number","configuration":{"kind":"number"},"required":false,"presentation":null,"default":{"kind":"number","value":lexeme},"index":null}}));
    assert_eq!(result["disk"], "committed");
    let read = h.read_template(&p, &template);
    assert_eq!(read["content"]["fields"][0]["default"]["value"], lexeme);
    assert_eq!(
        read["content"]["fields"][0]["initialDefault"]["value"],
        lexeme
    );
    let document = h.work(
        json!({"kind":"create_document","project":p,"view":read["view"],"name":"exact number doc"}),
    );
    let read = h.work(json!({"kind":"read_document","project":p,"document":document["artifact"]}));
    assert_eq!(read["content"]["values"][0]["value"]["kind"], "unset");
    h.state.lock().generation = 9_007_199_254_740_993;
    assert_eq!(
        h.call(json!({"action":"app_status"}))["generation"],
        "9007199254740993"
    );
    h.close_clean();
}

mod client_capacity;
mod fix;
mod g14;
mod m271;
mod m272;
mod m274;
mod m51;

mod recovery;

mod document_workspace;
