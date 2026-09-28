//! 실제 writer/child를 사용해 protocol 유실과 bounded read의 실패 계약을 고정한다.
use super::*;

fn native_id(path: &Path) -> Result<(u64, [u8; 16]), Box<dyn Error>> {
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
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((id.volume, id.index))
}
fn snapshot(f: &Fixture, label: &str) -> Result<Value, Box<dyn Error>> {
    let mut records = serde_json::Map::new();
    for (path, bytes) in inventory(&f.root) {
        let relative = path.strip_prefix(&f.root).unwrap_or(&path);
        let absolute = f.root.join(relative);
        let id = native_id(&absolute)?;
        records.insert(
            relative.to_string_lossy().into_owned(),
            json!({
                "id":id,"sha256":bytes.as_ref().map(|b| hex_sha(b)),
                "bytes":bytes
            }),
        );
    }
    let value = Value::Object(records);
    fs::write(
        f.base.join(format!("{label}.json")),
        serde_json::to_vec_pretty(&value)?,
    )?;
    Ok(value)
}
fn hex_sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn journal(f: &Fixture) -> PathBuf {
    fs::read_dir(f.root.join(".worldbuild/transactions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}
fn checkpoint_fixture(stage: &str, mixed: bool) -> Result<ManuallyDrop<Fixture>, Box<dyn Error>> {
    let f = persistent(mixed);
    if stage == "restore" {
        kill(&f, "commit", "before-progress", 1)?;
        kill(&f, "recover", "cleared", 1)?;
    } else {
        kill(
            &f,
            "commit",
            match stage {
                "temp" => "payload-synced",
                "applied" => "before-progress",
                "committed" => "committed-cleanup",
                _ => return Err("unknown stage".into()),
            },
            0,
        )?;
    }
    // 변형 전 실제 writer의 필수 기록이 모두 v2이고 서로 결합하는지 먼저 확인한다.
    let tx = journal(&f);
    let manifest: Value = serde_json::from_slice(&fs::read(tx.join("manifest.json"))?)?;
    let state: Value = serde_json::from_slice(&fs::read(tx.join("state.json"))?)?;
    assert_eq!(manifest["schemaVersion"], 2);
    assert_eq!(state["schemaVersion"], 2);
    assert_eq!(manifest["transactionId"], state["transactionId"]);
    assert!(tx.join("owned-temp-v1").is_dir());
    snapshot(&f, "writer-checkpoint")?;
    Ok(f)
}
fn move_out(f: &Fixture, path: &Path, name: &str) -> TestResult {
    assert!(path.starts_with(&f.root));
    let destination = f.base.join(name);
    assert!(!destination.exists());
    fs::rename(path, destination)?;
    Ok(())
}
fn ownership_paths(
    f: &Fixture,
    stage: &str,
) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), Box<dyn Error>> {
    let phase = if stage == "restore" {
        "restore"
    } else {
        "apply"
    };
    let index = if stage == "restore" { 1 } else { 0 };
    let dir = journal(f).join("owned-temp-v1");
    let receipt = dir.join(format!("{index:06}-{phase}.acquired.json"));
    let intent = dir.join(format!("{index:06}-{phase}.intent.json"));
    let record: Value = serde_json::from_slice(&fs::read(&receipt)?)?;
    let target = f.root.join(
        record["intent"]["target"]
            .as_str()
            .ok_or("target missing")?,
    );
    let temporary = target.parent().ok_or("parent missing")?.join(
        record["intent"]["temporary"]
            .as_str()
            .ok_or("temp missing")?,
    );
    Ok((receipt, intent, target, temporary))
}
fn foreign(f: &Fixture, path: &Path) -> TestResult {
    let bytes = fs::read(path)?;
    let old_id = file_id(path)?;
    move_out(f, path, "displaced-owned-object")?;
    fs::write(path, &bytes)?;
    assert_ne!(old_id, file_id(path)?);
    assert_eq!(bytes, fs::read(path)?);
    Ok(())
}
fn blocked_twice(f: &Fixture, label: &str) -> TestResult {
    let before = snapshot(f, "before-recovery")?;
    let mut results = Vec::new();
    for run in 1..=2 {
        let mut runtime = ProjectRuntime::acquire(&f.root, &f.base.join("locks"))?;
        assert!(runtime.ready().is_err());
        let error = runtime
            .recover()
            .expect_err("corruption must block before mutation");
        assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
        // Ready가 발급되지 않으므로 이를 요구하는 repository read/write API에 접근할 수 없다.
        assert!(runtime.ready().is_err());
        no_leak(&error);
        let after = snapshot(f, &format!("after-recovery-{run}"))?;
        assert_eq!(before, after, "{label}, recovery {run}");
        results.push(json!({"run":run,"state":"Blocked","ready_denied":true,
            "read_write_capability_denied":true,"bytes_and_file_ids_unchanged":true,"error":format!("{error:?}")}));
    }
    fs::write(
        f.base.join("fix-result.json"),
        serde_json::to_vec_pretty(&json!({"case":label,"results":results}))?,
    )?;
    println!(
        "FIX001_BLOCKED {}",
        json!({"case":label,"fixture":f.base,"recoveries":2})
    );
    Ok(())
}
#[test]
fn fix001_missing_directory_six_actual_writer_boundaries() -> TestResult {
    for (stage, mixed, replace_identity) in [
        ("temp", false, false),
        ("applied", true, true),
        ("applied", false, true),
        ("restore", false, false),
        ("committed", false, false),
        ("committed", true, true),
    ] {
        let f = checkpoint_fixture(stage, mixed)?;
        if replace_identity {
            let (_, _, target, _) = ownership_paths(&f, stage)?;
            foreign(&f, &target)?;
        }
        move_out(
            &f,
            &journal(&f).join("owned-temp-v1"),
            "removed-ownership-directory",
        )?;
        blocked_twice(&f, &format!("F01-{stage}-{mixed}-{replace_identity}"))?;
    }
    Ok(())
}
#[test]
fn fix001_partial_records_and_foreign_identity_remain_blocked() -> TestResult {
    for stage in ["temp", "applied", "restore", "committed"] {
        for damage in [
            "missing-acquired",
            "corrupt-acquired",
            "missing-both",
            "missing-intent",
        ] {
            let f = checkpoint_fixture(stage, false)?;
            let (receipt, intent, _, _) = ownership_paths(&f, stage)?;
            match damage {
                "corrupt-acquired" => {
                    fs::copy(&receipt, f.base.join("original-acquired.json"))?;
                    fs::write(&receipt, b"{\"intent\":")?;
                }
                "missing-intent" => move_out(&f, &intent, "removed-intent.json")?,
                _ => {
                    move_out(&f, &receipt, "removed-acquired.json")?;
                    if damage == "missing-both" {
                        move_out(&f, &intent, "removed-intent.json")?;
                    }
                }
            }
            blocked_twice(&f, &format!("{stage}-{damage}"))?;
        }
    }
    for stage in ["temp", "applied", "restore", "committed"] {
        let f = checkpoint_fixture(stage, false)?;
        let (_, _, target, temp) = ownership_paths(&f, stage)?;
        foreign(&f, if temp.exists() { &temp } else { &target })?;
        blocked_twice(&f, &format!("{stage}-same-bytes-foreign-id"))?;
    }
    Ok(())
}
#[test]
fn fix001_mandatory_protocol_records_reject_contradictions() -> TestResult {
    for damage in [
        "unknown",
        "missing-version",
        "wrong-type",
        "zero",
        "state-version",
        "marker-version",
        "all-legacy",
        "missing-manifest",
        "missing-state",
        "corrupt-state",
        "missing-manifest-and-owned",
        "missing-manifest-preparing",
        "transaction-id",
    ] {
        let f = checkpoint_fixture("committed", false)?;
        let tx = journal(&f);
        match damage {
            "missing-manifest" | "missing-manifest-and-owned" | "missing-manifest-preparing" => {
                move_out(&f, &tx.join("manifest.json"), "removed-manifest.json")?;
                if damage == "missing-manifest-and-owned" {
                    move_out(&f, &tx.join("owned-temp-v1"), "removed-ownership-directory")?;
                } else if damage == "missing-manifest-preparing" {
                    move_out(&f, &tx.join("committed.json"), "removed-committed.json")?;
                    let p = tx.join("state.json");
                    let mut v: Value = serde_json::from_slice(&fs::read(&p)?)?;
                    v["state"] = json!("preparing");
                    v["appliedOperations"] = json!([]);
                    fs::write(p, serde_json::to_vec(&v)?)?;
                }
            }
            "missing-state" => move_out(&f, &tx.join("state.json"), "removed-state.json")?,
            "corrupt-state" => fs::write(tx.join("state.json"), b"corrupt")?,
            _ => {
                for name in ["manifest.json", "state.json", "committed.json"] {
                    let p = tx.join(name);
                    let mut v: Value = serde_json::from_slice(&fs::read(&p)?)?;
                    let modify = damage == "all-legacy"
                        || name
                            == match damage {
                                "state-version" => "state.json",
                                "marker-version" => "committed.json",
                                _ => "manifest.json",
                            };
                    if modify {
                        match damage {
                            "unknown" => v["schemaVersion"] = json!(999),
                            "missing-version" => {
                                v.as_object_mut()
                                    .ok_or("record not object")?
                                    .remove("schemaVersion");
                            }
                            "wrong-type" => v["schemaVersion"] = json!("2"),
                            "zero" => v["schemaVersion"] = json!(0),
                            "transaction-id" => v["transactionId"] = json!("invalid-transaction"),
                            _ => v["schemaVersion"] = json!(1),
                        }
                        fs::write(p, serde_json::to_vec(&v)?)?;
                    }
                }
            }
        }
        blocked_twice(&f, damage)?;
    }
    Ok(())
}
#[test]
fn fix001_record_size_boundaries_and_partial_json() -> TestResult {
    for acquired in [false, true] {
        for length in [8192_usize, 8193, 16384] {
            let f = checkpoint_fixture("applied", false)?;
            let (receipt, intent, _, _) = ownership_paths(&f, "applied")?;
            let path = if acquired { receipt } else { intent };
            let mut bytes = fs::read(&path)?;
            fs::write(f.base.join("original-record.json"), &bytes)?;
            bytes.resize(length, b' ');
            fs::write(path, bytes)?;
            if length > 8192 {
                blocked_twice(&f, &format!("F02-{acquired}-{length}"))?;
            } else {
                recover_and_assert(&f, false, false)?;
            }
            println!(
                "FIX001_SIZE {}",
                json!({"fixture":f.base,"acquired":acquired,"bytes":length,"blocked":length>8192})
            );
        }
        let f = persistent(false);
        kill(
            &f,
            "commit",
            if acquired {
                "receipt-partial"
            } else {
                "intent-partial"
            },
            0,
        )?;
        snapshot(&f, "partial-checkpoint")?;
        recover_and_assert(&f, false, false)?;
    }
    Ok(())
}
#[test]
fn fix001_protocol_creation_and_cleanup_forced_exit() -> TestResult {
    for point in [
        "protocol-preparing",
        "protocol-manifest",
        "protocol-owned-directory",
        "cleanup-certified",
        "cleanup-auxiliary",
        "cleanup-manifest",
        "cleanup-marker",
        "cleanup-anchor",
    ] {
        let f = persistent(false);
        kill(&f, "commit", point, 0)?;
        snapshot(&f, "cleanup-checkpoint")?;
        recover_and_assert(&f, false, point.starts_with("cleanup-"))?;
        println!(
            "FIX001_PROTOCOL_KILL {}",
            json!({"fixture":f.base,"point":point,"strict_scans":true,"repeated":true})
        );
    }
    for point in [
        "cleanup-certified",
        "cleanup-auxiliary",
        "cleanup-manifest",
        "cleanup-marker",
        "cleanup-anchor",
    ] {
        let f = persistent(false);
        kill(&f, "commit", "before-progress", 1)?;
        kill(&f, "recover", point, 0)?;
        snapshot(&f, "cleanup-checkpoint")?;
        recover_and_assert(&f, false, false)?;
        println!(
            "FIX001_ROLLBACK_CLEANUP_KILL {}",
            json!({"fixture":f.base,"point":point,"strict_scans":true,"repeated":true})
        );
    }
    Ok(())
}
#[test]
fn fix001_export_actual_writer_for_original_reader() -> TestResult {
    let mut rows = Vec::new();
    for stage in ["temp", "applied"] {
        for lost in [false, true] {
            let f = checkpoint_fixture(stage, false)?;
            if lost {
                move_out(
                    &f,
                    &journal(&f).join("owned-temp-v1"),
                    "removed-ownership-directory",
                )?;
            }
            snapshot(&f, "original-reader-before")?;
            rows.push(json!({"fixture":f.base,"stage":stage,"lost":lost}));
        }
    }
    fs::write(
        evidence_base().join("fix001-original-reader-fixtures.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}
#[test]
fn fix001_cleanup_failure_twice_preserves_receipts_before_retry() -> TestResult {
    let f = checkpoint_fixture("restore", false)?;
    let before = snapshot(&f, "retry-before")?;
    for run in 1..=2 {
        let output = Command::new(std::env::current_exe()?)
            .args([
                "--ignored",
                "--exact",
                "data::repository::write::tests::b003_connection::b003_m1_child",
                "--nocapture",
            ])
            .env("B003_M1_BASE", &f.base)
            .env("B003_M1_MODE", "recover")
            .env("B003_M1_POINT", "cleanup-owned")
            .env("B003_M1_INDEX", "1")
            .env("B003_M1_ACTION", "fail")
            .output()?;
        assert!(!output.status.success());
        fs::write(
            f.base.join(format!("retry-{run}.stdout.log")),
            output.stdout,
        )?;
        fs::write(
            f.base.join(format!("retry-{run}.stderr.log")),
            output.stderr,
        )?;
        assert_eq!(before, snapshot(&f, &format!("retry-after-{run}"))?);
    }
    recover_and_assert(&f, false, false)?;
    Ok(())
}

#[path = "write_recovery_evidence_tests.rs"]
mod fix003;
