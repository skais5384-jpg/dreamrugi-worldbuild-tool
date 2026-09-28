use super::*;
use std::{
    mem::ManuallyDrop,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn evidence_base() -> PathBuf {
    static BASE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let p = std::env::var_os("WORLDBUILD_B003_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("worldbuild-b003-tests-{}", uuid::Uuid::new_v4()))
            });
        fs::create_dir_all(&p).unwrap();
        p
    })
    .clone()
}
fn persistent(mixed: bool) -> ManuallyDrop<Fixture> {
    let base = evidence_base().join(format!("m1-{}", uuid::Uuid::new_v4()));
    let root = base.join("project");
    fs::create_dir_all(root.join("documents")).unwrap();
    fs::create_dir(root.join("templates")).unwrap();
    let f = ManuallyDrop::new(Fixture { base, root });
    f.write(
        ArtifactSourceId::Template(template_id()),
        &raw(&template_value()),
    );
    if !mixed {
        f.write(
            ArtifactSourceId::Document(document_id()),
            &raw(&document_value()),
        );
    }
    f
}
fn candidates() -> (TemplateArtifact, DocumentArtifact) {
    let mut t = template_value();
    t["revision"] = json!(8);
    t["name"] = json!("B003 changed template");
    let mut d = document_value();
    d["templateRevision"] = json!(8);
    d["name"] = json!("B003 changed document");
    (
        artifact::decode_template(&raw(&t)).unwrap(),
        artifact::decode_document(&raw(&d)).unwrap(),
    )
}
#[test]
#[ignore = "controller kills this process without Drop"]
fn b003_m1_child() -> TestResult {
    let Some(base) = std::env::var_os("B003_M1_BASE") else {
        return Ok(());
    };
    let base = PathBuf::from(base);
    let f = ManuallyDrop::new(Fixture {
        root: base.join("project"),
        base,
    });
    let mut runtime = ProjectRuntime::acquire(&f.root, &f.base.join("locks"))?;
    runtime.recover()?;
    if std::env::var("B003_M1_MODE")?.as_str() == "recover" {
        return Err("recovery checkpoint not reached".into());
    }
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let t = repo.load_template(template_id())?;
    let d = if f.path(ArtifactSourceId::Document(document_id())).exists() {
        Some(repo.load_document(document_id())?)
    } else {
        None
    };
    let (tc, dc) = candidates();
    let plan = CanonicalWritePlan::new().replace_template(&tc, t.source())?;
    let plan = if let Some(d) = &d {
        plan.replace_document(&dc, d.source())?
    } else {
        plan.create_document(&dc)?
    };
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        let result = prepared.commit();
        let mode = std::env::var("B003_M1_MODE")?;
        if mode == "inline" {
            let out = result?;
            assert_eq!(out.result_state(), CommitResultState::RolledBack);
            no_leak(&out);
            fs::write(
                f.base.join("inline-result.json"),
                serde_json::to_vec(&json!({"state":"RolledBack","first_operation_reverted":true}))?,
            )?;
            return Ok(());
        }
        if mode == "compound" {
            let error = result.unwrap_err();
            assert!(error.diagnostic().rollback_failed);
            error.b003_assert_compound_causes();
            no_leak(&error);
            fs::write(
                f.base.join("inline-result.json"),
                serde_json::to_vec(
                    &json!({"state":"RecoveryRequired","primary_and_secondary_preserved":true}),
                )?,
            )?;
            return Ok(());
        }
        result?;
        Err("commit checkpoint not reached".into())
    })
}
fn kill(f: &Fixture, mode: &str, point: &str, index: u32) -> TestResult {
    let ready = f.base.join("child-ready");
    if ready.exists() {
        fs::remove_file(&ready)?;
    }
    let stdout = fs::File::create(f.base.join(format!("{mode}-{point}-{index}.stdout.log")))?;
    let stderr = fs::File::create(f.base.join(format!("{mode}-{point}-{index}.stderr.log")))?;
    let started = crate::data::utc_time::now_utc_milliseconds()?;
    let executable = std::env::current_exe()?;
    let cwd = std::env::current_dir()?;
    let mut child = Command::new(&executable)
        .args([
            "--ignored",
            "--exact",
            "data::repository::write::tests::b003_connection::b003_m1_child",
            "--nocapture",
        ])
        .env("B003_M1_BASE", &f.base)
        .env("B003_M1_MODE", mode)
        .env("B003_M1_POINT", point)
        .env("B003_M1_INDEX", index.to_string())
        .env("B003_M1_READY", &ready)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;
    let start = Instant::now();
    while !ready.exists() {
        if let Some(exit) = child.try_wait()? {
            return Err(format!(
                "child exited early {mode}/{point}/{index}: {exit}, fixture {}",
                f.base.display()
            )
            .into());
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill()?;
            child.wait()?;
            return Err(format!("timeout {mode}/{point}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill()?;
    let exit = child.wait()?;
    assert!(!exit.success());
    fs::write(
        f.base
            .join(format!("{mode}-{point}-{index}.execution.json")),
        serde_json::to_vec_pretty(&json!({
            "cwd":cwd,"argv":[executable.to_string_lossy(),"--ignored","--exact","data::repository::write::tests::b003_connection::b003_m1_child","--nocapture"],
            "started":started,"ended":crate::data::utc_time::now_utc_milliseconds()?,"pid":child.id(),"exit":exit.code(),"checkpoint":point,"mode":mode,"index":index,"kill_wait":true
        }))?,
    )?;
    println!(
        "B003_M1_KILL {}",
        json!({"fixture":f.base,"mode":mode,"point":point,"index":index,"pid":child.id(),"exit":exit.code(),"forced_without_drop":true})
    );
    Ok(())
}
fn recover_and_assert(f: &Fixture, mixed: bool, committed: bool) -> TestResult {
    let mut runtime = ProjectRuntime::acquire(&f.root, &f.base.join("locks"))?;
    assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
    assert!(runtime.ready().is_err());
    runtime.recover()?;
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    let (t, d) = candidates();
    assert_eq!(
        fs::read(f.path(ArtifactSourceId::Template(template_id())))?,
        if committed {
            artifact::encode_template(&t)?
        } else {
            raw(&template_value())
        }
    );
    let path = f.path(ArtifactSourceId::Document(document_id()));
    if !committed && mixed {
        assert!(!path.exists());
    } else {
        assert_eq!(
            fs::read(path)?,
            if committed {
                artifact::encode_document(&d)?
            } else {
                raw(&document_value())
            }
        );
    }
    {
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        repo.scan_templates()?;
        repo.scan_documents()?;
    }
    let before = inventory(&f.root);
    runtime.recover()?;
    assert_eq!(before, inventory(&f.root));
    assert!(fs::read_dir(f.root.join(".worldbuild/transactions"))?
        .next()
        .is_none());
    Ok(())
}
#[test]
fn b003_m1_forced_exit_recovery() -> TestResult {
    let mut rows = Vec::new();
    for mixed in [false, true] {
        for point in [
            "intent-partial",
            "intent-before-sync",
            "intent-durable",
            "created",
            "identity",
            "receipt-partial",
            "receipt-before-sync",
            "acquired",
            "cleared",
            "payload-partial",
            "payload-synced",
            "renamed",
            "before-progress",
            "committed-cleanup",
        ] {
            let f = persistent(mixed);
            kill(&f, "commit", point, 0)?;
            recover_and_assert(&f, mixed, point == "committed-cleanup")?;
            rows.push(json!({"fixture":f.base,"mixed":mixed,"point":point,"M1_G2_recovery":true,"second_recovery_unchanged":true,"strict_scans":true,"committed":point=="committed-cleanup"}));
        }
    }
    for point in [
        "intent-partial",
        "intent-before-sync",
        "intent-durable",
        "created",
        "receipt-partial",
        "acquired",
        "cleared",
        "payload-partial",
        "payload-synced",
        "renamed",
    ] {
        let f = persistent(false);
        kill(&f, "commit", "before-progress", 1)?;
        kill(&f, "recover", point, 1)?;
        recover_and_assert(&f, false, false)?;
        rows.push(json!({"fixture":f.base,"rollback_point":point,"two_operations_applied_before_first_kill":true,"M1_G2_recovery":true,"second_recovery_unchanged":true,"strict_scans":true}));
    }
    fs::write(
        evidence_base().join("m1-crash-results.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}
fn file_id(path: &Path) -> Result<(u64, [u8; 16]), Box<dyn Error>> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    struct Id {
        volume: u64,
        index: [u8; 16],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(h: *mut c_void, c: i32, b: *mut Id, n: u32) -> i32;
    }
    let f = fs::File::open(path)?;
    let mut b = Id {
        volume: 0,
        index: [0; 16],
    };
    if unsafe { GetFileInformationByHandleEx(f.as_raw_handle(), 18, &mut b, 24) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((b.volume, b.index))
}

#[test]
fn b003_inline_rollback_and_compound_diagnostics() -> TestResult {
    let mut rows = Vec::new();
    for mode in ["inline", "compound"] {
        for mixed in [false, true] {
            let f = persistent(mixed);
            let mut command = Command::new(std::env::current_exe()?);
            command
                .args([
                    "--ignored",
                    "--exact",
                    "data::repository::write::tests::b003_connection::b003_m1_child",
                    "--nocapture",
                ])
                .env("B003_M1_BASE", &f.base)
                .env("B003_M1_MODE", mode)
                .env("B003_M1_POINT", "payload-synced")
                .env("B003_M1_INDEX", "1")
                .env("B003_M1_ACTION", "fail");
            if mode == "compound" {
                command.env("B003_M1_SECOND_POINT", "cleanup-owned");
            }
            let output = command.output()?;
            fs::write(f.base.join("inline.stdout.log"), &output.stdout)?;
            fs::write(f.base.join("inline.stderr.log"), &output.stderr)?;
            assert!(
                output.status.success(),
                "{mode} {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: Value =
                serde_json::from_slice(&fs::read(f.base.join("inline-result.json"))?)?;
            if mode == "compound" {
                assert!(fs::read_dir(f.root.join(".worldbuild/transactions"))?
                    .next()
                    .is_some());
            }
            recover_and_assert(&f, mixed, false)?;
            rows.push(json!({"fixture":f.base,"mixed":mixed,"mode":mode,"result":result,"recovery_after_fault_removed":true}));
        }
    }
    fs::write(
        evidence_base().join("m1-inline-results.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}
#[test]
fn b003_large_stage_small_backup_admission_allocates_nothing() -> TestResult {
    let mut rows = Vec::new();
    for replace in [false, true] {
        let f = persistent(true);
        if !replace {
            fs::remove_file(f.path(ArtifactSourceId::Template(template_id())))?;
        }
        let mut runtime = f.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let mut value = template_value();
        value["future"] = json!({"large":"x".repeat(2*1024*1024)});
        let candidate = artifact::decode_template(&serde_json::to_vec(&value)?)?;
        let size = artifact::encode_template(&candidate)?.len() as u64;
        let plan = if replace {
            CanonicalWritePlan::new()
                .replace_template(&candidate, repo.load_template(template_id())?.source())?
        } else {
            CanonicalWritePlan::new().create_template(&candidate)?
        };
        let before = inventory(&f.root);
        let (error, queries) = with_storage_response(
            TestStorageResponse::Available {
                available_bytes: 0,
                allocation_unit_bytes: 4096,
            },
            || {
                with_plan(plan, &repo, |p, permit| {
                    p.prepare(&repo, permit).unwrap_err()
                })
            },
        );
        assert_eq!(
            error.diagnostic().category,
            ArtifactWriteCategory::StorageRejected
        );
        assert_eq!(before, inventory(&f.root));
        assert_eq!(queries.len(), 1);
        rows.push(json!({"fixture":f.base,"replace":replace,"staged_bytes":size,"no_namespace_or_journal_allocation":true}));
    }
    fs::write(
        evidence_base().join("m1-storage-results.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}
#[test]
fn b003_prepare_compat_fixtures() -> TestResult {
    let mut rows = Vec::new();
    for point in ["acquired", "before-progress"] {
        let f = persistent(false);
        kill(&f, "commit", point, 0)?;
        rows.push(json!({"fixture":f.base,"root":f.root,"point":point}));
    }
    fs::write(
        evidence_base().join("compat-new-fixtures.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}
#[test]
fn b003_m1_collision_and_foreign_identity() -> TestResult {
    let mut rows = Vec::new();
    for payload in [Vec::new(), artifact::encode_document(&candidates().1)?] {
        let f = persistent(true);
        let mut runtime = f.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let (_, dc) = candidates();
        let plan = CanonicalWritePlan::new().create_document(&dc)?;
        with_plan(plan, &repo, |plan, permit| -> TestResult {
            let prepared = plan.prepare(&repo, permit)?;
            let inner = prepared.inner_for_test();
            let temp = f.root.join("documents").join(format!(
                ".wb-{}-000000-apply.tmp",
                inner.transaction_id().as_str()
            ));
            fs::write(&temp, &payload)?;
            let id = file_id(&temp)?;
            assert!(prepared.commit().is_err());
            assert_eq!(file_id(&temp)?, id);
            assert_eq!(fs::read(&temp)?, payload);
            assert!(!f.path(ArtifactSourceId::Document(document_id())).exists());
            rows.push(json!({"fixture":f.base,"collision_length":payload.len(),"actual_M1_create_failed":true,"foreign_preserved":true}));
            Ok(())
        })?;
    }
    for kind in [
        "same-bytes-different-id",
        "missing-receipt",
        "corrupt-receipt",
        "unknown-journal",
    ] {
        let f = persistent(false);
        kill(&f, "commit", "payload-synced", 0)?;
        let tx = fs::read_dir(f.root.join(".worldbuild/transactions"))?
            .next()
            .ok_or("missing journal")??;
        let tx = tx.path();
        let temp = fs::read_dir(f.root.join("documents"))?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().starts_with(".wb-"))
            .ok_or("missing owned temp")?
            .path();
        let receipt = tx.join("owned-temp-v1/000000-apply.acquired.json");
        match kind {
            "same-bytes-different-id" => {
                let b = fs::read(&temp)?;
                let original = file_id(&temp)?;
                fs::rename(&temp, f.base.join("displaced-owned-file"))?;
                fs::write(&temp, &b)?;
                assert_ne!(file_id(&temp)?, original);
            }
            "missing-receipt" => fs::remove_file(receipt)?,
            "corrupt-receipt" => fs::write(receipt, b"{}")?,
            _ => fs::write(tx.join("owned-temp-v1/unknown.json"), b"preserve")?,
        }
        let before = inventory(&f.root);
        let mut runtime = ProjectRuntime::acquire(&f.root, &f.base.join("locks"))?;
        assert!(runtime.recover().is_err());
        assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
        assert!(runtime.ready().is_err());
        assert_eq!(before, inventory(&f.root));
        assert!(runtime.recover().is_err());
        assert_eq!(before, inventory(&f.root));
        rows.push(json!({"fixture":f.base,"kind":kind,"blocked_before_mutation":true,"second_attempt_unchanged":true}));
    }
    fs::write(
        evidence_base().join("m1-negative-results.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}

#[path = "write_protocol_tests.rs"]
mod fix001;
