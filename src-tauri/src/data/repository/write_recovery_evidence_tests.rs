//! FIX-003: 실제 writer 기록과 손상 controller를 분리한 복구 증거 회귀.
use super::*;

fn replay(f: &Fixture) -> TestResult {
    let state = journal(f).join("state.json");
    fs::copy(&state, f.base.join("interrupted-state.json"))?;
    let earlier = fs::read(f.base.join("actual-preparing.json"))?;
    let old: Value = serde_json::from_slice(&earlier)?;
    let now: Value = serde_json::from_slice(&fs::read(&state)?)?;
    assert_eq!(old["state"], "preparing");
    assert_eq!(old["transactionId"], now["transactionId"]);
    fs::write(state, earlier)?;
    Ok(())
}
#[test]
fn fix003_pre_preparing_replay() -> TestResult {
    let f = persistent(true);
    kill(&f, "commit", "before-progress", 0)?;
    snapshot(&f, "writer-checkpoint")?;
    replay(&f)?;
    move_out(&f, &journal(&f).join("owned-temp-v1"), "removed-owned")?;
    mixed_scan_control(&f)?;
    blocked_twice(&f, "F03-actual-preparing-replay")
}
#[test]
fn fix003_pre_cleaning_no_marker() -> TestResult {
    let f = persistent(true);
    kill(&f, "commit", "before-progress", 0)?;
    snapshot(&f, "writer-checkpoint")?;
    let state = journal(&f).join("state.json");
    fs::copy(&state, f.base.join("original-state.json"))?;
    let mut value: Value = serde_json::from_slice(&fs::read(&state)?)?;
    value["state"] = json!("cleaningCommitted");
    fs::write(&state, serde_json::to_vec(&value)?)?;
    mixed_scan_control(&f)?;
    blocked_twice(&f, "F04-cleaning-without-marker")
}

fn namespace_snapshot(f: &Fixture, label: &str) -> Result<Value, Box<dyn Error>> {
    let full = snapshot(f, label)?;
    Ok(Value::Object(
        full.as_object()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.starts_with("documents") || p.starts_with("templates"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ))
}
fn cleaned(f: &Fixture, committed: bool, label: &str) -> TestResult {
    let before = namespace_snapshot(f, "before-cleanup")?;
    recover_and_assert(f, true, committed)?;
    assert_eq!(namespace_snapshot(f, "after-cleanup")?, before, "{label}");
    println!(
        "FIX003_NORMAL {}",
        json!({"fixture":f.base,"case":label,"namespace_bytes_ids_unchanged":true,"ready":true,"repeat_unchanged":true})
    );
    Ok(())
}
fn cleaning_fixture(committed: bool, point: &str) -> Result<ManuallyDrop<Fixture>, Box<dyn Error>> {
    let f = persistent(true);
    if committed {
        kill(&f, "commit", point, 0)?;
    } else {
        kill(&f, "commit", "before-progress", 0)?;
        kill(&f, "recover", point, 0)?;
    }
    snapshot(&f, "writer-checkpoint")?;
    Ok(f)
}
#[test]
fn fix003_preparing_replay_adjacent_losses_and_temps() -> TestResult {
    for (point, damage) in [
        ("payload-synced", "owned-missing"),
        ("before-progress", "owned-empty"),
        ("payload-synced", "manifest-missing"),
        ("before-progress", "manifest-missing"),
    ] {
        let f = persistent(true);
        kill(&f, "commit", point, 0)?;
        snapshot(&f, "writer-checkpoint")?;
        replay(&f)?;
        let tx = journal(&f);
        move_out(&f, &tx.join("owned-temp-v1"), "removed-owned")?;
        if damage == "owned-empty" {
            fs::create_dir(tx.join("owned-temp-v1"))?;
        }
        if damage == "manifest-missing" {
            move_out(&f, &tx.join("manifest.json"), "removed-manifest.json")?;
        }
        blocked_twice(&f, &format!("F03-{point}-{damage}"))?;
    }
    for (phase, empty) in [
        ("apply", true),
        ("apply", false),
        ("restore", true),
        ("restore", false),
    ] {
        let f = persistent(true);
        kill(&f, "commit", "protocol-manifest", 0)?;
        let tx = journal(&f);
        let id = tx.file_name().unwrap().to_str().unwrap();
        let temp = f
            .root
            .join("documents")
            .join(format!(".wb-{id}-000000-{phase}.tmp"));
        let bytes = if empty {
            vec![]
        } else {
            fs::read(tx.join("staged/000000.json"))?
        };
        fs::write(temp, bytes)?;
        blocked_twice(&f, &format!("F03-{phase}-empty-{empty}"))?;
    }
    for damage in [
        "missing-original-targets",
        "corrupt-original-targets",
        "malformed-state",
    ] {
        let f = persistent(true);
        kill(&f, "commit", "protocol-preparing", 0)?;
        let state = journal(&f).join("state.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&state)?)?;
        match damage {
            "missing-original-targets" => {
                value.as_object_mut().unwrap().remove("originalTargets");
            }
            "corrupt-original-targets" => {
                value["originalTargets"][0]["originalSize"] = json!(7);
            }
            _ => {}
        }
        fs::copy(&state, f.base.join("original-state.json"))?;
        fs::write(
            state,
            if damage == "malformed-state" {
                b"{\"schemaVersion\":2,\"state\":".to_vec()
            } else {
                serde_json::to_vec(&value)?
            },
        )?;
        blocked_twice(&f, damage)?;
    }
    Ok(())
}
#[test]
fn fix003_preparing_manifestless_control_and_partial_cleanup() -> TestResult {
    for damaged in [false, true] {
        let f = persistent(true);
        kill(&f, "commit", "protocol-empty", 0)?;
        assert!(!journal(&f).join("state.json").exists());
        if damaged {
            fs::write(
                journal(&f).join("staged/000000.json"),
                b"unattributed evidence",
            )?;
            blocked_twice(&f, "initial-records-missing-but-artifacts-present")?;
        } else {
            cleaned(&f, false, "actual-initial-empty-journal")?;
        }
    }
    // FIX-001이 기존 생성 checkpoint 3개를 모두 검사한다. 여기서는 manifest 유실과
    // 실제 정리 도중의 새 경계만 추가한다.
    let f = persistent(true);
    kill(&f, "commit", "protocol-manifest", 0)?;
    move_out(
        &f,
        &journal(&f).join("manifest.json"),
        "removed-manifest.json",
    )?;
    cleaned(&f, false, "Preparing-originals-manifest-missing")?;
    let f = persistent(true);
    kill(&f, "commit", "protocol-manifest", 0)?;
    kill(&f, "recover", "cleanup-staged-file", 0)?;
    cleaned(&f, false, "Preparing-partial-staged-cleanup")?;
    Ok(())
}
#[test]
fn fix003_cleaning_requires_markers_identity_and_reachable_topology() -> TestResult {
    let f = persistent(true);
    kill(&f, "commit", "before-progress", 0)?;
    let p = journal(&f).join("state.json");
    fs::copy(&p, f.base.join("original-state.json"))?;
    let mut v: Value = serde_json::from_slice(&fs::read(&p)?)?;
    v["state"] = json!("cleaningRolledBack");
    fs::write(p, serde_json::to_vec(&v)?)?;
    blocked_twice(&f, "F04-incomplete-cleaningRolledBack")?;
    for committed in [true, false] {
        for damage in [
            "marker-missing",
            "wrong-kind",
            "wrong-identity",
            "manifest-only-missing",
            "state-truncated",
            "aux-order",
        ] {
            let f = cleaning_fixture(committed, "cleanup-certified")?;
            let tx = journal(&f);
            let name = if committed {
                "committed.json"
            } else {
                "rolled-back.json"
            };
            match damage {
                "marker-missing" => move_out(&f, &tx.join(name), "removed-marker.json")?,
                "wrong-kind" => {
                    move_out(&f, &tx.join(name), "removed-marker.json")?;
                    fs::copy(
                        f.base.join("removed-marker.json"),
                        tx.join(if committed {
                            "rolled-back.json"
                        } else {
                            "committed.json"
                        }),
                    )?;
                }
                "wrong-identity" => {
                    let p = tx.join(name);
                    fs::copy(&p, f.base.join("original-marker.json"))?;
                    let mut v: Value = serde_json::from_slice(&fs::read(&p)?)?;
                    v["projectFingerprint"] = json!("e".repeat(64));
                    fs::write(p, serde_json::to_vec(&v)?)?;
                }
                "manifest-only-missing" => {
                    move_out(&f, &tx.join("manifest.json"), "removed-manifest.json")?
                }
                "aux-order" => move_out(&f, &tx.join("backups"), "removed-backups")?,
                _ => {
                    let p = tx.join("state.json");
                    fs::copy(&p, f.base.join("original-state.json"))?;
                    fs::write(p, b"{\"schemaVersion\":2,\"state\":")?;
                }
            }
            blocked_twice(&f, &format!("F04-{committed}-{damage}"))?;
        }
        let f = cleaning_fixture(committed, "cleanup-manifest")?;
        fs::create_dir(journal(&f).join("staged"))?;
        blocked_twice(&f, "F04-resurrected-directory-after-manifest")?;
    }
    let f = cleaning_fixture(true, "cleanup-certified")?;
    foreign(&f, &f.path(ArtifactSourceId::Document(document_id())))?;
    blocked_twice(&f, "F04-committed-target-same-bytes-different-id")?;
    Ok(())
}
#[test]
fn fix003_partial_cleanup_writer_boundaries() -> TestResult {
    for committed in [true, false] {
        for point in [
            "cleanup-staged-file",
            "cleanup-staged",
            "cleanup-backups-file",
            "cleanup-backups",
            "cleanup-owned-temp-v1-file",
        ] {
            let f = cleaning_fixture(committed, point)?;
            cleaned(&f, committed, &format!("{committed}-{point}"))?;
        }
    }
    Ok(())
}
#[test]
fn fix003_cleanup_io_retry_preserves_evidence() -> TestResult {
    use std::os::windows::fs::OpenOptionsExt;
    for committed in [true, false] {
        for stage in ["certificate-save", "first-delete"] {
            let f = if stage == "first-delete" {
                cleaning_fixture(committed, "cleanup-certified")?
            } else {
                cleaning_fixture(committed, "cleanup-before-certificate")?
            };
            let tx = journal(&f);
            // 실제 인증 직전의 state 파일 공유 잠금으로 atomic 교체 오류를 만든다.
            let path = if stage == "certificate-save" {
                tx.join("state.json")
            } else {
                tx.join("staged/000000.json")
            };
            let held = fs::OpenOptions::new().read(true).share_mode(1).open(path)?;
            let before = snapshot(&f, "before-io-recovery")?;
            let mut runtime = ProjectRuntime::acquire(&f.root, &f.base.join("locks"))?;
            let error = runtime
                .recover()
                .expect_err("real sharing violation must preserve required evidence");
            assert!(runtime.ready().is_err());
            no_leak(&error);
            assert_eq!(snapshot(&f, "after-io-recovery")?, before);
            assert!(tx.join("manifest.json").exists());
            assert!(tx
                .join(if committed {
                    "committed.json"
                } else {
                    "rolled-back.json"
                })
                .exists());
            runtime.close()?;
            drop(held);
            cleaned(&f, committed, stage)?;
        }
    }
    Ok(())
}

fn mixed_scan_control(f: &Fixture) -> TestResult {
    let template = fs::read(f.path(ArtifactSourceId::Template(template_id())))?;
    let document = fs::read(f.path(ArtifactSourceId::Document(document_id())))?;
    let t: Value = serde_json::from_slice(&template)?;
    let d: Value = serde_json::from_slice(&document)?;
    assert_eq!(t["revision"], 7);
    assert_eq!(d["templateRevision"], 8);
    // G2 차단 fixture에 Ready를 위조하지 않는다. 동일 namespace bytes의 별도 G3
    // control에서 scan 성공을 확인하고, 원래 fixture는 두 번 Blocked를 assert한다.
    let base = f.base.join("g3-control");
    let root = base.join("project");
    fs::create_dir_all(root.join("templates"))?;
    fs::create_dir(root.join("documents"))?;
    let control = ManuallyDrop::new(Fixture { base, root });
    fs::write(
        control.path(ArtifactSourceId::Template(template_id())),
        template,
    )?;
    fs::write(
        control.path(ArtifactSourceId::Document(document_id())),
        document,
    )?;
    let mut runtime = ProjectRuntime::acquire(&control.root, &control.base.join("locks"))?;
    runtime.recover()?;
    let ready = runtime.ready()?;
    let repository = ArtifactRepository::new(&ready)?;
    repository.scan_documents()?;
    repository.scan_templates()?;
    fs::write(
        f.base.join("mixed-g3-control.json"),
        serde_json::to_vec_pretty(
            &json!({"old_template_revision":7,"new_document_template_revision":8,"same_namespace_bytes_control":control.base,"g3_scans_passed":true,"original_fixture_ready_not_issued":true}),
        )?,
    )?;
    Ok(())
}
