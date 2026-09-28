//! C1은 Rust 러너에 한 번만 등록한다. Node/설치된 TypeScript가 없으면 실패하며 skip하지 않는다.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command as ProcessCommand, Stdio},
    sync::Weak,
};

struct Original {
    key: RetainedRef,
    input: Weak<Work>,
    source: Weak<View>,
    result: *const (),
}

// 읽기 전용 관찰이다. 인수 중 draft, 명시적 포기 결과 안의 원 owner까지 추적한다.
fn owners(state: &Inner) -> Vec<(&Arc<Work>, &Completed)> {
    let mut values = state
        .retained
        .iter()
        .map(|(i, r)| (i, r))
        .collect::<Vec<_>>();
    for op in state.operations.values() {
        if let Some((i, r)) = &op.draft {
            values.push((i, r));
        }
        if let Some(r) = &op.result {
            if r.retain_edit {
                values.push((op.edit_input.as_ref().or(op.input.as_ref()).unwrap(), r));
            }
            if let Some((i, r)) = r.original.downcast_ref::<(Arc<Work>, Completed)>() {
                values.push((i, r));
            }
        }
    }
    values
}

fn observe(h: &Harness, originals: &mut Vec<Original>, released: &[RetainedRef]) {
    let state = h.state.lock();
    let current = owners(&state);
    for original in originals.iter() {
        let owner = current
            .iter()
            .find(|(_, r)| r.retained == Some(original.key));
        if released.contains(&original.key) {
            assert!(owner.is_none(), "explicit abandonment must occur once");
        } else {
            let (input, result) = owner.expect("original owner disappeared before explicit ack");
            assert!(Weak::ptr_eq(&original.input, &Arc::downgrade(input)));
            assert!(Weak::ptr_eq(
                &original.source,
                &Arc::downgrade(&result.sources[0])
            ));
            assert!(std::ptr::eq(
                original.result,
                &*result.original as *const _ as *const ()
            ));
        }
    }
    for (input, result) in current {
        let key = result.retained.unwrap();
        if !originals.iter().any(|r| r.key == key) {
            assert!(result.g6_clearable);
            assert_eq!(result.sources.len(), 1);
            originals.push(Original {
                key,
                input: Arc::downgrade(input),
                source: Arc::downgrade(&result.sources[0]),
                result: &*result.original as *const _ as *const (),
            });
        }
    }
}

#[test]
fn c1_actual_client_capacity_recovery_and_shutdown() {
    for (count, action, closing) in [
        (39, "abandon", false),
        (40, "abandon", false),
        (40, "handoff", false),
        (40, "handoff", true),
    ] {
        let h = Harness::new();
        let p = h.open();
        let (template, _) = h.template(&p);
        let view = h.read_template(&p, &template);
        let session = h.session(&p, vec![view["view"].clone()], "template");
        let input = json!({"kind":"update_template","project":p,"session":session,
            "view":view["view"],"revision":"1","edit":{"kind":"name","name":"capacity retained intent"}});
        let mut first = input.clone();
        first["edit"]["name"] = json!("committed base");
        assert_eq!(h.work(first)["disk"], "committed");
        let artifact = h.root.join(format!("templates/{template}.json"));
        let before = fs::read(&artifact).unwrap();
        let driver =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/state/tests/client-capacity.cjs");
        let mut child = ProcessCommand::new("node")
            .arg(driver)
            .args([
                count.to_string(),
                action.to_owned(),
                if closing { "closing" } else { "open" }.to_owned(),
            ])
            .env("WB_CLIENT_INPUT", input.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("C1 requires installed Node and repository TypeScript");
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut originals = Vec::new();
        let mut released = Vec::new();
        let mut summary = None;
        for line in BufReader::new(stdout).lines() {
            let packet: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if packet["done"] == true {
                summary = Some(packet["summary"].clone());
                break;
            }
            let command = packet["command"].clone();
            let abandoning = if command["action"] == "acknowledge_transport" {
                let state = h.state.lock();
                let id: Id = command["operation"]
                    .as_str()
                    .unwrap()
                    .to_owned()
                    .try_into()
                    .unwrap();
                state.operations.get(&id).and_then(|op| {
                    match op.result.as_ref()?.dto.as_ref().ok()? {
                        ResultDto::RetainedHandled {
                            retained,
                            action: "abandoned",
                        } => Some(*retained),
                        _ => None,
                    }
                })
            } else {
                None
            };
            // MockRuntime host가 실제 앱 event loop의 종료 poll 역할을 수행한다.
            h.state.poll();
            // 모든 driver 요청은 실제 등록/ACL/Raw decoder를 통한다. 직접 dispatch 제출은 없다.
            let mut result = h.ipc(command.clone());
            if command["action"] == "operation"
                && result.as_ref().is_ok_and(|r| r["state"] == "pending")
            {
                h.result(command["operation"].as_str().unwrap());
                result = h.ipc(command);
            }
            if result.is_ok() {
                if let Some(key) = abandoning {
                    assert!(!released.contains(&key));
                    released.push(key);
                }
            }
            observe(&h, &mut originals, &released);
            let response = match result {
                Ok(value) => json!({"ok":true,"value":value}),
                Err(value) => json!({"ok":false,"value":value}),
            };
            writeln!(stdin, "{response}").unwrap();
            stdin.flush().unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "actual client failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let summary =
            summary.expect("actual client must finish every assertion and official cleanup");
        assert_eq!(originals.len(), count);
        assert_eq!(released.len(), count);
        assert!(originals
            .iter()
            .all(|r| r.input.upgrade().is_none() && r.source.upgrade().is_none()));
        assert_eq!(
            fs::read(artifact).unwrap(),
            before,
            "stale/control duplicate must not write disk"
        );
        assert!(h.state.poll());
        assert!(h.state.lock().projects.is_empty());
        println!("FIX002_C1 {summary}");
    }
}
