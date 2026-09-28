//! FIX-002: 실제 generic writer의 큰 v1 journal을 제품 회귀로 고정한다.
use super::*;
use crate::data::{project_runtime::ProjectRuntime, repository::ArtifactRepository};
use std::collections::BTreeMap;
use std::mem::ManuallyDrop;

#[test]
fn fix004_wrong_string_eof_blocks_g2_and_preserves_twice() -> TestResult {
    let temp = fixture("fix004-wrong-string-eof")?;
    let (root, directory) = fix003_pending_child(&temp)?;
    let manifest_bytes = fs::read(directory.join("manifest.json"))?;
    let manifest: TransactionManifest = serde_json::from_slice(&manifest_bytes)?;
    manifest.validate()?;
    assert_eq!(manifest.schema_version.get(), 1);
    assert_eq!(manifest.operations.len(), 256);
    assert!(manifest_bytes.len() > 65536);
    let path = directory.join("state.json");
    fs::copy(&path, temp.0.join("original-state.json"))?;
    let input = br#"{"schemaVersion":"2"#;
    assert_eq!(input.len(), 19);
    assert_eq!(
        super::super::prepare::sha256(input),
        "791f8e7bc2e64e5ec0b06968ea88e91d67d6fcf8a77b020542961bc643d5661b"
    );
    fs::write(&path, input)?;
    fs::write(temp.0.join("injected-state.json"), input)?;
    // snapshot은 값과 File ID만 소유한다. 복구를 방해하는 controller handle은 남기지 않는다.
    let before = fix003_save_native(&temp, &root, "injected")?;
    let mut outcomes = Vec::new();
    for run in 1..=2 {
        let mut runtime = ProjectRuntime::acquire(&root, &temp.0.join("locks"))?;
        let result = runtime.recover();
        let blocked =
            runtime.snapshot().state == crate::data::project_runtime::RuntimeState::Blocked;
        let ready_denied = runtime.ready().is_err();
        let after = fix003_save_native(&temp, &root, &format!("after-{run}"))?;
        let unchanged = after == before;
        let outcome = serde_json::json!({"run":run,"recovery_error":result.is_err(),"blocked":blocked,"ready_denied":ready_denied,"all_bytes_ids_entries_preserved":unchanged,"entries_before":before.len(),"entries_after":after.len()});
        fs::write(
            temp.0.join(format!("outcome-{run}.json")),
            serde_json::to_vec_pretty(&outcome)?,
        )?;
        outcomes.push(result.is_err() && blocked && ready_denied && unchanged);
        runtime.close()?;
    }
    println!(
        "FIX004_G2 {}",
        serde_json::json!({"fixture":temp.0,"outcomes":outcomes})
    );
    assert!(
        outcomes == [true, true],
        "F06 must block both recoveries and preserve the injected snapshot"
    );
    Ok(())
}

#[test]
fn fix004_eof_before_value_keeps_g2_legacy_control() -> TestResult {
    let temp = fixture("fix004-before-value-control")?;
    let (root, directory) = fix003_pending_child(&temp)?;
    let path = directory.join("state.json");
    fs::copy(&path, temp.0.join("original-state.json"))?;
    // F06과 동일한 실제 Prepared writer에서 값의 첫 따옴표만 아직 오지 않은 경계다.
    let input = br#"{"schemaVersion":"#;
    fs::write(&path, input)?;
    fs::write(temp.0.join("injected-state.json"), input)?;
    fix003_save_native(&temp, &root, "injected")?;
    ready_twice(&temp, &root, &directory, false, 256)?;
    fs::write(
        temp.0.join("fix004-control.json"),
        b"{\"recoveries\":2,\"ready\":true,\"strict_scan\":true,\"actual_writer_operations\":256}",
    )?;
    println!(
        "FIX004_G2_CONTROL {}",
        serde_json::json!({"fixture":temp.0,"case":"eof-before-value","ready":true})
    );
    Ok(())
}

fn fixture(label: &str) -> TestResultWith<ManuallyDrop<Temp>> {
    let base = std::env::var_os("WORLDBUILD_FIX002_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("fix002-{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base)?;
    Ok(ManuallyDrop::new(Temp(base)))
}
type TestResultWith<T> = Result<T, Box<dyn std::error::Error>>;

type NativeInventory = BTreeMap<PathBuf, (Option<Vec<u8>>, (u64, [u8; 16]))>;
fn native_id(path: &Path) -> io::Result<(u64, [u8; 16])> {
    use std::{
        ffi::c_void,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    #[repr(C)]
    struct Id {
        volume: u64,
        index: [u8; 16],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(h: *mut c_void, c: i32, b: *mut Id, n: u32) -> i32;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(7)
        .custom_flags(0x02200000)
        .open(path)?;
    let mut id = Id {
        volume: 0,
        index: [0; 16],
    };
    if unsafe { GetFileInformationByHandleEx(file.as_raw_handle(), 18, &mut id, 24) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((id.volume, id.index))
}
fn inventory(root: &Path) -> io::Result<NativeInventory> {
    fn walk(root: &Path, at: &Path, out: &mut NativeInventory) -> io::Result<()> {
        out.insert(
            at.strip_prefix(root).unwrap().to_path_buf(),
            (None, native_id(at)?),
        );
        for entry in fs::read_dir(at)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    (Some(fs::read(&path)?), native_id(&path)?),
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

#[test]
fn fix002_large_v1_actual_writer_recovers_and_repeats() -> TestResult {
    let temp = fixture("reported-writer")?;
    let root = setup(&temp)?;
    fs::create_dir(root.join("templates"))?;
    fs::create_dir(root.join("documents"))?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("locks"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let mut plan = TransactionPlan::new();
    for index in 0..256 {
        plan.add_json(
            &format!("data/item-{index:04}.json"),
            &Document {
                schema_version: 1,
                title: "legacy",
            },
        )?;
    }
    let prepared = plan.prepare_for_test(&project)?;
    let directory = prepared.transaction_directory().to_path_buf();
    let manifest = fs::read(directory.join("manifest.json"))?;
    let model: TransactionManifest = serde_json::from_slice(&manifest)?;
    assert_eq!(model.schema_version.get(), 1);
    assert_eq!(model.operations.len(), 256);
    assert!(manifest.len() > 65536);
    assert!(
        manifest
            .windows(15)
            .position(|b| b == b"\"schemaVersion\"")
            .unwrap()
            > 65536
    );
    fs::write(temp.0.join("manifest-preserved.json"), &manifest)?;
    fs::write(
        temp.0.join("writer-result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "actual_generic_writer": true, "schema_version": 1, "operations": 256,
            "manifest_bytes": manifest.len(), "version_offset": manifest.windows(15).position(|b| b == b"\"schemaVersion\"").unwrap(),
        }))?,
    )?;
    drop(prepared);
    drop(project);
    lock.release()?;
    let mut runtime = ProjectRuntime::acquire(&root, &temp.0.join("locks"))?;
    assert!(runtime.ready().is_err());
    let recovered = runtime.recover();
    assert!(
        recovered.is_ok(),
        "normal large v1 journal must recover: {recovered:?}"
    );
    assert_eq!(recovered?.rolled_back_transactions, 1);
    assert!(!directory.exists());
    assert_eq!(fs::read_dir(root.join("data"))?.count(), 0);
    {
        let ready = runtime.ready()?;
        let repository = ArtifactRepository::new(&ready)?;
        repository.scan_documents()?;
        repository.scan_templates()?;
    }
    let after = inventory(&root)?;
    assert_eq!(runtime.recover()?.discovered_transactions, 0);
    assert_eq!(inventory(&root)?, after);
    assert!(runtime.ready().is_ok());
    runtime.close()?;
    fs::write(
        temp.0.join("result.json"),
        b"{\"recovered\":true,\"ready\":true,\"strict_scan\":true,\"second_unchanged\":true}",
    )?;
    Ok(())
}

// 실제 commit의 첫 progress 실패 뒤 rollback 시작도 실패시켜 부분 적용을 남긴다.
// committed 대조는 기존 Cleanup hook을 쓰며 marker를 합성하지 않는다.
struct Partial;
impl CommitHooks for Partial {
    fn check(&self, point: CommitFailPoint, operation: Option<u32>) -> io::Result<()> {
        if point == CommitFailPoint::ProgressState && operation == Some(0) {
            Err(io::Error::other("partial test checkpoint"))
        } else {
            Ok(())
        }
    }
    fn check_recovery(&self, point: RecoveryFailPoint, _: Option<u32>) -> io::Result<()> {
        if point == RecoveryFailPoint::RollingBackState {
            Err(io::Error::other("retain partial test checkpoint"))
        } else {
            Ok(())
        }
    }
}
fn pending(temp: &Temp, mode: &str, count: usize) -> TestResultWith<(PathBuf, PathBuf)> {
    let root = setup(temp)?;
    fs::create_dir(root.join("templates"))?;
    fs::create_dir(root.join("documents"))?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("locks"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let mut plan = TransactionPlan::new();
    for index in 0..count {
        let path = format!("data/item-{index:04}.json");
        fs::write(root.join(&path), bytes("old")?)?;
        plan.add_json(
            &path,
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;
    }
    let prepared = plan.prepare_for_test(&project)?;
    let directory = prepared.transaction_directory().to_path_buf();
    match mode {
        "fix003-child" => {
            let manifest = fs::read(directory.join("manifest.json"))?;
            fs::write(temp.0.join("manifest-preserved.json"), &manifest)?;
            fs::write(
                temp.0.join("writer-result.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "actual_generic_writer":true,"operations":count,"manifest_bytes":manifest.len(),
                    "version_offset":manifest.windows(15).position(|b|b==b"\"schemaVersion\"").unwrap(),"checkpoint":"prepared-held-before-drop"
                }))?,
            )?;
            let ready = fs::File::create(temp.0.join("legacy-child-ready"))?;
            ready.sync_all()?;
            // Prepared와 project lock을 가진 실제 writer 프로세스를 controller가 종료한다.
            loop {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        "prepared" => drop(prepared),
        "partial" => {
            let error = commit_with_hooks(prepared, &Partial)
                .expect_err("both injected boundaries must fail");
            assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
            assert_eq!(fs::read(root.join("data/item-0000.json"))?, bytes("new")?);
            assert!(!directory.join("committed.json").exists());
        }
        "committed" => assert_eq!(
            commit_with_hooks(prepared, &CommitCleanupFail)?.result_state(),
            CommitResultState::CommittedCleanupFailed
        ),
        "rolled-back" => {
            drop(prepared);
            let error = recover_pending_transactions_with_hooks(
                &project,
                &Fail(RecoveryFailPoint::Cleanup),
            )
            .expect_err("retain genuine rollback marker");
            assert_eq!(
                error.result_state(),
                RecoveryResultState::RolledBackCleanupFailed
            );
        }
        _ => return Err("unknown fixture mode".into()),
    }
    fs::write(
        temp.0.join("manifest-preserved.json"),
        fs::read(directory.join("manifest.json"))?,
    )?;
    drop(project);
    lock.release()?;
    Ok((root, directory))
}
fn ready_twice(
    temp: &Temp,
    root: &Path,
    directory: &Path,
    committed: bool,
    count: usize,
) -> TestResult {
    let mut runtime = ProjectRuntime::acquire(root, &temp.0.join("locks"))?;
    assert!(runtime.ready().is_err());
    let result = runtime.recover()?;
    assert_eq!(result.discovered_transactions, 1);
    assert!(!directory.exists());
    for index in 0..count {
        assert_eq!(
            fs::read(root.join(format!("data/item-{index:04}.json")))?,
            bytes(if committed { "new" } else { "old" })?
        );
    }
    {
        let ready = runtime.ready()?;
        let repository = ArtifactRepository::new(&ready)?;
        repository.scan_documents()?;
        repository.scan_templates()?;
    }
    let after = inventory(root)?;
    assert_eq!(runtime.recover()?.discovered_transactions, 0);
    assert_eq!(inventory(root)?, after);
    assert!(runtime.ready().is_ok());
    runtime.close()?;
    fs::write(
        temp.0.join("result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"ready":true,"strict_scans":true,"committed_new":committed,"second_bytes_ids_unchanged":true}),
        )?,
    )?;
    Ok(())
}
#[test]
fn fix002_large_v1_prepared_partial_and_committed_boundaries() -> TestResult {
    for mode in ["prepared", "partial", "committed"] {
        let temp = fixture(mode)?;
        let (root, directory) = pending(&temp, mode, 256)?;
        let raw = fs::read(directory.join("manifest.json"))?;
        let model: TransactionManifest = serde_json::from_slice(&raw)?;
        assert!(raw.len() > 65536);
        assert_eq!(model.schema_version.get(), 1);
        assert_eq!(model.operations.len(), 256);
        fs::write(
            temp.0.join("writer-result.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"mode":mode,"operations":256,"version":1,"manifest_bytes":raw.len(),"actual_commit_hooks":true}),
            )?,
        )?;
        ready_twice(&temp, &root, &directory, mode == "committed", 256)?;
    }
    Ok(())
}
#[test]
fn fix002_v1_all_main_records_accept_raw_size_boundaries() -> TestResult {
    for name in [
        "manifest.json",
        "state.json",
        "committed.json",
        "rolled-back.json",
    ] {
        for length in [65536, 65537, 131201] {
            let temp = fixture(&format!("{name}-{length}"))?;
            let mode = match name {
                "committed.json" => "committed",
                "rolled-back.json" => "rolled-back",
                _ => "prepared",
            };
            let (root, directory) = pending(&temp, mode, 1)?;
            let path = directory.join(name);
            let mut raw = fs::read(&path)?;
            let value: serde_json::Value = serde_json::from_slice(&raw)?;
            assert_eq!(value["schemaVersion"], 1);
            raw.resize(length, b' ');
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&raw)?, value);
            fs::write(&path, &raw)?;
            fs::write(temp.0.join("input-preserved.json"), &raw)?;
            ready_twice(&temp, &root, &directory, mode == "committed", 1)?;
        }
    }
    Ok(())
}
#[test]
fn fix002_late_invalid_main_records_block_twice_without_mutation() -> TestResult {
    for name in [
        "manifest.json",
        "state.json",
        "committed.json",
        "rolled-back.json",
    ] {
        for damage in [
            "garbage",
            "second-json",
            "duplicate-version",
            "unknown-version",
            "missing-version",
            "duplicate-identity",
        ] {
            let temp = fixture(&format!("{name}-{damage}"))?;
            let mode = match name {
                "committed.json" => "committed",
                "rolled-back.json" => "rolled-back",
                _ => "prepared",
            };
            let (root, directory) = pending(&temp, mode, 1)?;
            let path = directory.join(name);
            let original = fs::read(&path)?;
            let mut raw = original.clone();
            if damage == "garbage" || damage == "second-json" {
                raw.resize(131201, b' ');
                raw.extend_from_slice(if damage == "garbage" {
                    b"invalid"
                } else {
                    b"{}"
                });
            } else {
                let mut value: serde_json::Value = serde_json::from_slice(&raw)?;
                let version = value
                    .as_object_mut()
                    .unwrap()
                    .remove("schemaVersion")
                    .unwrap();
                raw = serde_json::to_vec(&value)?;
                assert_eq!(raw.pop(), Some(b'}'));
                raw.resize(131201, b' ');
                match damage {
                    "duplicate-version" => {
                        raw.extend_from_slice(b",\"schemaVersion\":1,\"schemaVersion\":1}")
                    }
                    "unknown-version" => raw.extend_from_slice(b",\"schemaVersion\":9}"),
                    "missing-version" => raw.push(b'}'),
                    "duplicate-identity" => {
                        raw.extend_from_slice(
                            format!(
                                ",\"schemaVersion\":{version},\"transactionId\":{} }}",
                                value["transactionId"]
                            )
                            .as_bytes(),
                        );
                    }
                    _ => unreachable!(),
                }
            }
            fs::write(temp.0.join("original-record.json"), original)?;
            fs::write(&path, &raw)?;
            fs::write(temp.0.join("input-preserved.json"), raw)?;
            let before = inventory(&root)?;
            let mut runtime = ProjectRuntime::acquire(&root, &temp.0.join("locks"))?;
            for _ in 0..2 {
                assert!(runtime.recover().is_err(), "{name}/{damage} must block");
                assert_eq!(
                    runtime.snapshot().state,
                    crate::data::project_runtime::RuntimeState::Blocked
                );
                assert!(runtime.ready().is_err());
                assert_eq!(inventory(&root)?, before);
            }
            runtime.close()?;
            fs::write(
                temp.0.join("result.json"),
                b"{\"blocked_twice\":true,\"all_bytes_ids_preserved\":true}",
            )?;
        }
    }
    Ok(())
}

#[test]
fn fix003_pre_late_v2_truncation() -> TestResult {
    let temp = fixture("fix003-pre-late-v2")?;
    let (root, directory) = fix003_pending_child(&temp)?;
    let state = directory.join("state.json");
    fs::copy(&state, temp.0.join("original-state.json"))?;
    let damaged = format!(
        "{{\"padding\":\"{}\",\"schemaVersion\":2,\"damaged\":",
        "x".repeat(70000)
    );
    fs::write(&state, damaged)?;
    let before = fix003_save_native(&temp, &root, "before-recovery")?;
    for run in 1..=2 {
        let mut runtime = ProjectRuntime::acquire(&root, &temp.0.join("locks"))?;
        assert!(
            runtime.recover().is_err(),
            "F05 must block observed v2 before cleanup"
        );
        assert!(runtime.ready().is_err());
        assert_eq!(
            fix003_save_native(&temp, &root, &format!("after-{run}"))?,
            before
        );
        runtime.close()?;
    }
    println!(
        "FIX003_STREAM {}",
        serde_json::json!({"fixture":temp.0,"case":"late-v2-truncation","blocked":true})
    );
    Ok(())
}

fn fix003_save_native(temp: &Temp, root: &Path, label: &str) -> TestResultWith<NativeInventory> {
    let value = inventory(root)?;
    fs::write(
        temp.0.join(format!("{label}.json")),
        serde_json::to_vec_pretty(&value)?,
    )?;
    Ok(value)
}
#[test]
fn fix003_state_partial_evidence_and_legacy_controls() -> TestResult {
    if let Some(base) = std::env::var_os("FIX003_LEGACY_CHILD_BASE") {
        let temp = ManuallyDrop::new(Temp(PathBuf::from(base)));
        pending(&temp, "fix003-child", 256)?;
        return Err("legacy checkpoint was not held".into());
    }
    for case in [
        "front-v2",
        "late-unknown",
        "late-duplicate",
        "escaped-key",
        "wrong-version-type",
        "wrong-identity",
        "bad-time",
        "duplicate-progress",
        "nonjson",
        "truncated-v1",
        "nested-and-string",
        "valid-late-v1",
    ] {
        let temp = fixture(case)?;
        let (root, directory) = fix003_pending_child(&temp)?;
        let path = directory.join("state.json");
        let original = fs::read(&path)?;
        fs::write(temp.0.join("original-state.json"), &original)?;
        let prefix = format!("{{\"padding\":\"{}\",", "x".repeat(70000));
        let (text,blocked)=match case {
            "front-v2" => ("{\"schemaVersion\":2,\"damaged\":".to_owned(),true),
            "late-unknown" => (format!("{prefix}\"schemaVersion\":7,\"damaged\":"),true),
            "late-duplicate" => (format!("{prefix}\"schemaVersion\":1,\"schemaVersion\":1,\"damaged\":"),true),
            "escaped-key" => (format!(r#"{prefix}"schema\u0056ersion":2,"damaged":"#),true),
            "wrong-version-type" => (format!("{prefix}\"schemaVersion\":\"2\",\"damaged\":"),true),
            "wrong-identity" => (format!("{prefix}\"schemaVersion\":1,\"projectFingerprint\":\"{}\",\"damaged\":", "e".repeat(64)),true),
            "bad-time" => (format!("{prefix}\"schemaVersion\":1,\"updatedAtUtc\":\"private-bad-time\",\"damaged\":"),true),
            "duplicate-progress" => (format!("{prefix}\"schemaVersion\":1,\"appliedOperations\":[],\"appliedOperations\":[],\"damaged\":"),true),
            "nonjson" => ("advisory interrupted state".to_owned(),false),
            "truncated-v1" => ("{\"schemaVersion\":1,\"state\":".to_owned(),false),
            "nested-and-string" => (r#"{"nested":{"schemaVersion":2},"text":"schemaVersion:2","schemaVersion":1,"damaged":"#.to_owned(),false),
            _ => (format!("{prefix}{}",String::from_utf8(original)?[1..].to_owned()),false),
        };
        fs::write(&path, text.as_bytes())?;
        let before = fix003_save_native(&temp, &root, "before-recovery")?;
        if blocked {
            for run in 1..=2 {
                let mut runtime = ProjectRuntime::acquire(&root, &temp.0.join("locks"))?;
                let error = runtime
                    .recover()
                    .expect_err("partial mandatory evidence must block");
                assert!(runtime.ready().is_err());
                assert_eq!(
                    runtime.snapshot().state,
                    crate::data::project_runtime::RuntimeState::Blocked
                );
                assert!(!format!("{error:?}").contains("private-bad-time"));
                assert_eq!(
                    fix003_save_native(&temp, &root, &format!("after-{run}"))?,
                    before
                );
                runtime.close()?;
            }
        } else {
            ready_twice(&temp, &root, &directory, false, 256)?;
        }
        fs::write(
            temp.0.join("fix003-result.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"case":case,"blocked":blocked,"recoveries":2,"actual_writer":true,"operations":256,"state_bytes":text.len()}),
            )?,
        )?;
        println!(
            "FIX003_STREAM {}",
            serde_json::json!({"fixture":temp.0,"case":case,"blocked":blocked})
        );
    }
    Ok(())
}

fn fix003_pending_child(temp: &Temp) -> TestResultWith<(PathBuf, PathBuf)> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let executable = std::env::current_exe()?;
    let name="data::transaction::recovery_tests::legacy_stream::fix003_state_partial_evidence_and_legacy_controls";
    let started = crate::data::utc_time::now_utc_milliseconds()?;
    let mut child = Command::new(&executable)
        .args(["--exact", name, "--nocapture"])
        .env("FIX003_LEGACY_CHILD_BASE", &temp.0)
        .stdin(Stdio::null())
        .stdout(fs::File::create(temp.0.join("legacy-child.stdout.log"))?)
        .stderr(fs::File::create(temp.0.join("legacy-child.stderr.log"))?)
        .spawn()?;
    let timer = Instant::now();
    while !temp.0.join("legacy-child-ready").exists() {
        if let Some(exit) = child.try_wait()? {
            return Err(format!("legacy child exited before checkpoint: {exit}").into());
        }
        if timer.elapsed() > Duration::from_secs(30) {
            child.kill()?;
            child.wait()?;
            return Err("legacy writer checkpoint timeout".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill()?;
    let exit = child.wait()?;
    assert!(!exit.success());
    fs::write(
        temp.0.join("legacy-child.execution.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "cwd":std::env::current_dir()?,"argv":[executable.to_string_lossy(),"--exact",name,"--nocapture"],
            "started":started,"ended":crate::data::utc_time::now_utc_milliseconds()?,"pid":child.id(),"exit":exit.code(),
            "checkpoint":"prepared-held-before-drop","kill_wait":true,"fixture":temp.0
        }))?,
    )?;
    let root = temp.0.join("project");
    let directory = fs::read_dir(root.join(".worldbuild/transactions"))?
        .next()
        .ok_or("legacy journal missing")??
        .path();
    println!(
        "FIX003_LEGACY_KILL {}",
        serde_json::json!({"fixture":temp.0,"pid":child.id(),"exit":exit.code(),"kill_wait":true})
    );
    Ok((root, directory))
}

// FIX-005는 Create의 기존 대상 부재와 실제 Prepared child 종료를 함께 고정한다.
fn fix005_create_writer(temp: &Temp) -> TestResult {
    let root = setup(temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("locks"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let mut plan = TransactionPlan::new();
    for index in 0..256 {
        plan.add_json(
            &format!("data/fix005-create-{index:04}.json"),
            &serde_json::json!({"schemaVersion":1,"ordinal":index}),
        )?;
    }
    let prepared = plan.prepare_for_test(&project)?;
    let journal = prepared.transaction_directory();
    let raw = fs::read(journal.join("manifest.json"))?;
    let manifest: TransactionManifest = serde_json::from_slice(&raw)?;
    manifest.validate()?;
    assert_eq!(manifest.schema_version.get(), 1);
    assert_eq!(manifest.operations.len(), 256);
    assert!(manifest
        .operations
        .iter()
        .all(|o| !o.original_existed && o.backup_path.is_none()));
    let state = fs::read(journal.join("state.json"))?;
    let typed: TransactionStateRecord = serde_json::from_slice(&state)?;
    typed.validate()?;
    assert_eq!(typed.state, TransactionState::Prepared);
    assert_eq!(fs::read_dir(root.join("data"))?.count(), 0);
    fs::write(temp.0.join("actual-manifest.json"), &raw)?;
    fs::write(temp.0.join("original-state.json"), &state)?;
    fs::write(
        temp.0.join("writer.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "journal":journal,"transaction_id":manifest.transaction_id,"operations":256,"all_create":true,
            "version":1,"manifest_bytes":raw.len(),"state_bytes":state.len(),"pid":std::process::id()
        }))?,
    )?;
    fs::File::create(temp.0.join("checkpoint"))?.sync_all()?;
    loop {
        std::hint::black_box(&prepared);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fix005_snapshot(temp: &Temp, root: &Path, label: &str) -> TestResultWith<NativeInventory> {
    // inventory가 반환될 때 모든 File ID 조회 handle은 이미 닫혀 있다.
    let value = fix003_save_native(temp, root, label)?;
    let hashes:BTreeMap<_,_>=value.iter().map(|(p,(bytes,id))| (p,serde_json::json!({
        "kind":if bytes.is_some(){"file"}else{"directory"},"file_id":id,
        "bytes":bytes.as_ref().map(Vec::len),"sha256":bytes.as_ref().map(|b|super::super::prepare::sha256(b))
    }))).collect();
    fs::write(
        temp.0.join(format!("{label}-metadata.json")),
        serde_json::to_vec_pretty(&hashes)?,
    )?;
    Ok(value)
}

#[test]
fn fix005_enum_unit_create_prepared_g2_pair() -> TestResult {
    use std::process::{Command, Stdio};
    if let Some(base) = std::env::var_os("FIX005_CREATE_CHILD_BASE") {
        return fix005_create_writer(&Temp(PathBuf::from(base)));
    }
    let mut all_met = Vec::new();
    for (label, input, expect_blocked) in [
        (
            "enum-unit-wrong-string",
            br#"{"state":{"prepared":"x"#.as_slice(),
            true,
        ),
        (
            "enum-unit-null-control",
            br#"{"state":{"prepared":n"#.as_slice(),
            false,
        ),
    ] {
        let temp = fixture(&format!("fix005-{label}"))?;
        let executable = std::env::current_exe()?;
        let test="data::transaction::recovery_tests::legacy_stream::fix005_enum_unit_create_prepared_g2_pair";
        let started = crate::data::utc_time::now_utc_milliseconds()?;
        let mut child = Command::new(&executable)
            .args(["--exact", test, "--nocapture"])
            .env("FIX005_CREATE_CHILD_BASE", &temp.0)
            .stdin(Stdio::null())
            .stdout(fs::File::create(temp.0.join("writer.stdout.log"))?)
            .stderr(fs::File::create(temp.0.join("writer.stderr.log"))?)
            .spawn()?;
        let timer = std::time::Instant::now();
        while !temp.0.join("checkpoint").exists() {
            if child.try_wait()?.is_some() {
                return Err("Create child exited before Prepared checkpoint".into());
            }
            if timer.elapsed() > Duration::from_secs(30) {
                child.kill()?;
                child.wait()?;
                return Err("Create checkpoint timeout".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let checkpoint = crate::data::utc_time::now_utc_milliseconds()?;
        child.kill()?;
        let killed = crate::data::utc_time::now_utc_milliseconds()?;
        let exit = child.wait()?;
        assert!(!exit.success());
        let env: BTreeMap<_, _> = std::env::vars()
            .filter(|(k, _)| {
                matches!(k.as_str(), "PATH" | "TEMP" | "TMP")
                    || k.starts_with("WORLDBUILD_")
                    || k.starts_with("CARGO_")
                    || k.starts_with("RUST_TEST_")
            })
            .chain(std::iter::once((
                "FIX005_CREATE_CHILD_BASE".into(),
                temp.0.to_string_lossy().into_owned(),
            )))
            .collect();
        fs::write(
            temp.0.join("writer.execution.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "cwd":std::env::current_dir()?,"argv":[executable.to_string_lossy(),"--exact",test,"--nocapture"],
                "environment":env,"started":started,"checkpoint_seen":checkpoint,"kill_returned":killed,
                "wait_returned":crate::data::utc_time::now_utc_milliseconds()?,"checkpoint":"Prepared-held-before-drop",
                "pid":child.id(),"exit":exit.code(),"kill_wait":true
            }))?,
        )?;
        let root = temp.0.join("project");
        let journal = fs::read_dir(root.join(".worldbuild/transactions"))?
            .next()
            .ok_or("missing journal")??
            .path();
        fix005_snapshot(&temp, &root, "prepared")?;
        fs::write(journal.join("state.json"), input)?;
        fs::write(temp.0.join("injected-state.json"), input)?;
        let before = fix005_snapshot(&temp, &root, "injected")?;
        let mut prior = None;
        let mut outcomes = Vec::new();
        for run in 1..=2 {
            let mut runtime = ProjectRuntime::acquire(&root, &temp.0.join("locks"))?;
            let pending_denied = runtime.ready().is_err();
            let result = runtime.recover();
            let blocked =
                runtime.snapshot().state == crate::data::project_runtime::RuntimeState::Blocked;
            let ready = runtime.ready().is_ok();
            let old = fs::read_dir(root.join("data"))?.count() == 0;
            if ready {
                let access = runtime.ready()?;
                let repository = ArtifactRepository::new(&access)?;
                repository.scan_documents()?;
                repository.scan_templates()?;
            }
            let after = fix005_snapshot(&temp, &root, &format!("after-{run}"))?;
            let unchanged = after == before;
            let idempotent = prior.as_ref().is_none_or(|p| p == &after);
            let counts=result.as_ref().ok().map(|s|serde_json::json!({"discovered":s.discovered_transactions,"rolled_back":s.rolled_back_transactions}));
            let met = pending_denied
                && old
                && if expect_blocked {
                    result.is_err() && blocked && !ready && unchanged
                } else {
                    result.is_ok()
                        && ready
                        && !journal.exists()
                        && idempotent
                        && counts
                            .as_ref()
                            .is_some_and(|c| c["rolled_back"] == if run == 1 { 1 } else { 0 })
                };
            let outcome = serde_json::json!({"run":run,"recovery_error":result.is_err(),"blocked":blocked,"ready":ready,"pending_ready_denied":pending_denied,"old_create_absent":old,"all_bytes_ids_entries_preserved":unchanged,"entries_before":before.len(),"entries_after":after.len(),"idempotent":idempotent,"counts":counts,"met":met});
            fs::write(
                temp.0.join(format!("outcome-{run}.json")),
                serde_json::to_vec_pretty(&outcome)?,
            )?;
            outcomes.push(outcome);
            all_met.push(met);
            prior = Some(after);
            runtime.close()?;
        }
        println!(
            "FIX005_G2 {}",
            serde_json::json!({"fixture":temp.0,"case":label,"pid":child.id(),"kill_wait":true,"outcomes":outcomes})
        );
    }
    assert!(all_met.iter().all(|v|*v),"unit payload contradiction must preserve both snapshots; null prefix must recover old Create absence");
    Ok(())
}
