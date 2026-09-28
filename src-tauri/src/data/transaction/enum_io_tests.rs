//! FIX-005: enum 내부 관측과 실제 반환 I/O의 기존 소비자 경로를 회귀로 고정한다.
use super::*;
use serde_json::{json, Value};
use std::{collections::BTreeMap, error::Error, io::SeekFrom, path::PathBuf};

const PRIVATE: &str = r#"FIX005-CUSTOM-PRIVATE {"secret":"token"} C:\private\audit-source"#;
#[derive(Debug)]
struct OwnedCause {
    ticket: u64,
    nonce: [u8; 4],
    body: &'static str,
}
impl std::fmt::Display for OwnedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.body)
    }
}
impl Error for OwnedCause {}

struct Input {
    bytes: Vec<u8>,
    pos: usize,
    pass: usize,
    chunks: Vec<usize>,
    delivered: Vec<usize>,
    requests: BTreeMap<usize, usize>,
    fault: Option<(usize, io::Error)>,
    fired: usize,
}
impl Input {
    fn new(bytes: Vec<u8>, chunks: Vec<usize>) -> Self {
        Self {
            bytes,
            pos: 0,
            pass: 0,
            chunks,
            delivered: vec![0],
            requests: BTreeMap::new(),
            fault: None,
            fired: 0,
        }
    }
    fn fault(mut self, at: usize, custom: bool) -> Self {
        let error = if custom {
            io::Error::other(OwnedCause {
                ticket: 4040707,
                nonce: [4, 0, 0, 7],
                body: PRIVATE,
            })
        } else {
            io::Error::from_raw_os_error(5)
        };
        if custom {
            let c = error
                .get_ref()
                .and_then(|e| e.downcast_ref::<OwnedCause>())
                .unwrap();
            assert!(c.ticket == 4040707 && c.nonce == [4, 0, 0, 7] && c.body == PRIVATE);
        } else {
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.raw_os_error(), Some(5));
        }
        self.fault = Some((at, error));
        self
    }
    fn stats(&self) -> Value {
        json!({"input_bytes":self.bytes.len(),"delivered_per_pass":self.delivered,
            "rewinds":self.pass,"request_histogram":self.requests,"injections":self.fired})
    }
}
impl Read for Input {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        *self.requests.entry(out.len()).or_default() += 1;
        if out.is_empty() {
            return Ok(0);
        }
        if self.fault.as_ref().is_some_and(|(at, _)| self.pos == *at) {
            // 고정 serde는 오류 위치를 정리하며 다시 peek할 수 있으므로 fault 경계는 지속한다.
            self.fired += 1;
            let (at, error) = self.fault.take().unwrap();
            let again = if error.get_ref().is_some_and(|x| x.is::<OwnedCause>()) {
                io::Error::other(OwnedCause {
                    ticket: 4040707,
                    nonce: [4, 0, 0, 7],
                    body: PRIVATE,
                })
            } else {
                io::Error::from_raw_os_error(5)
            };
            self.fault = Some((at, again));
            return Err(error);
        }
        let cap = self.chunks[self.pos % self.chunks.len()];
        let boundary = self.fault.as_ref().map_or(self.bytes.len(), |(at, _)| *at);
        let n = out
            .len()
            .min(cap)
            .min(self.bytes.len() - self.pos)
            .min(boundary - self.pos);
        out[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        self.delivered[self.pass] += n;
        Ok(n)
    }
}
impl Seek for Input {
    fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
        assert!(matches!(at, SeekFrom::Start(0)));
        self.pos = 0;
        self.pass += 1;
        self.delivered.push(0);
        Ok(0)
    }
}
fn base() -> PathBuf {
    static BASE: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        let root = std::env::var_os("WORLDBUILD_FIX002_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        root.join(format!("fix005-reader-{}", uuid::Uuid::new_v4()))
    });
    BASE.clone()
}
fn save(name: &str, value: &Value) {
    let p = base().join("probes").join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn context() -> TransactionManifest {
    let manifest: TransactionManifest = serde_json::from_value(json!({
        "schemaVersion":1,"transactionId":format!("txn-{}", "4".repeat(64)),
        "createdAtUtc":"2026-09-08T08:00:00.123Z","projectFingerprint":"b".repeat(64),
        "operations":[{"index":0,"targetPath":"data/audit-context.json",
        "stagedPath":"staged/000000.json","backupPath":null,"originalExisted":false,
        "originalSize":null,"originalSha256":null,"stagedSize":19,
        "stagedSha256":"a".repeat(64),"stagedSchemaVersion":null,"originalSchemaVersion":null}]
    }))
    .unwrap();
    manifest.validate().unwrap();
    manifest
}
fn state(m: &TransactionManifest) -> String {
    json!({"schemaVersion":1,"transactionId":m.transaction_id,
        "projectFingerprint":m.project_fingerprint,"updatedAtUtc":"2026-09-08T08:00:00.123Z",
        "state":"prepared","appliedOperations":[]})
    .to_string()
}
fn describe(result: &io::Result<Option<TransactionStateRecord>>) -> Value {
    match result {
        Ok(None) => json!({"return":"advisory"}),
        Ok(Some(s)) => json!({"return":"typed","model_valid":s.validate().is_ok()}),
        Err(e) => {
            let mut v = json!({"return":"error","kind":format!("{:?}",e.kind()),
                "outer_os_code":e.raw_os_error(),"original_os_code":io_cause(e).raw_os_error()});
            if let Some(f) = e.get_ref().and_then(|x| x.downcast_ref::<JsonFailure>()) {
                v["category"] = json!(format!("{:?}", f.source.classify()));
                v["phase"] = json!(format!("{:?}", f.phase));
                v["refusal"] = json!(f.observed.map(|r| format!("{r:?}")));
            }
            if let Some(f) = e.get_ref().and_then(|x| x.downcast_ref::<PrivateIo>()) {
                v["category"] = json!("Io");
                v["phase"] = json!(f.1.map(|c| format!("{:?}", c.0)));
                v["refusal"] = json!(f.1.and_then(|c| c.1).map(|r| format!("{r:?}")));
            }
            v
        }
    }
}
fn assert_private(e: &io::Error, custom: bool, phase: JsonPhase, refusal: Option<StateRefusal>) {
    let p = e
        .get_ref()
        .and_then(|x| x.downcast_ref::<PrivateIo>())
        .unwrap();
    assert_eq!(p.1, Some((phase, refusal)));
    let orig = io_cause(e);
    if custom {
        assert_eq!(orig.kind(), io::ErrorKind::Other);
        let c = orig
            .get_ref()
            .and_then(|x| x.downcast_ref::<OwnedCause>())
            .unwrap();
        assert!(c.ticket == 4040707 && c.nonce == [4, 0, 0, 7] && c.body == PRIVATE);
    } else {
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(orig.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(orig.raw_os_error(), Some(5));
    }
}
fn redacted(e: &dyn Error) -> bool {
    let mut current = Some(e);
    while let Some(x) = current {
        let rendered = format!("{x:?} {x}");
        if [
            PRIVATE,
            "FIX005-CUSTOM-PRIVATE",
            r"C:\private",
            r#""secret""#,
            r#""schemaVersion""#,
            "abcdefghijklmnopqrstuvwxyz",
        ]
        .iter()
        .any(|s| rendered.contains(s))
        {
            return false;
        }
        current = x.source();
    }
    true
}

#[test]
fn fix005_enum_boundaries_and_controls() {
    let m = context();
    let mut rows = Vec::new();
    let cases = [
        (
            "unit-string-eof",
            r#"{"state":{"prepared":"x"#,
            Some("MandatoryField"),
            "Eof",
        ),
        (
            "unit-string-syntax",
            r#"{"state":{"prepared":"x\q"#,
            Some("MandatoryField"),
            "Syntax",
        ),
        (
            "other-variant",
            r#"{"state":{"applying":"x"#,
            Some("MandatoryField"),
            "Eof",
        ),
        (
            "unit-number",
            r#"{"state":{"prepared":1e"#,
            Some("MandatoryField"),
            "Eof",
        ),
        (
            "unit-bool",
            r#"{"state":{"prepared":t"#,
            Some("MandatoryField"),
            "Eof",
        ),
        (
            "unit-sequence",
            r#"{"state":{"prepared":["#,
            Some("MandatoryField"),
            "Data",
        ),
        (
            "unit-map",
            r#"{"state":{"prepared":{"#,
            Some("MandatoryField"),
            "Data",
        ),
        (
            "escaped",
            r#"{"st\u0061te":{"prep\u0061red":"x"#,
            Some("MandatoryField"),
            "Eof",
        ),
        ("unit-before-value", r#"{"state":{"prepared":"#, None, "Eof"),
        ("unit-null-n", r#"{"state":{"prepared":n"#, None, "Eof"),
        ("unit-null-nu", r#"{"state":{"prepared":nu"#, None, "Eof"),
        ("unit-null-nul", r#"{"state":{"prepared":nul"#, None, "Eof"),
        (
            "unit-null-no-close",
            r#"{"state":{"prepared":null"#,
            None,
            "Eof",
        ),
        ("string-variant-prefix", r#"{"state":"prep"#, None, "Eof"),
        (
            "unit-follow-field",
            r#"{"state":{"prepared":null},"updatedAtUtc":"x"#,
            None,
            "Eof",
        ),
        (
            "string-follow-field",
            r#"{"state":"prepared","schemaVersion":1e"#,
            None,
            "Eof",
        ),
        (
            "unit-follow-seq",
            r#"{"state":{"prepared":null},"appliedOperations":[0, "#,
            None,
            "Eof",
        ),
        (
            "unit-follow-wrong-seq",
            r#"{"state":{"prepared":null},"appliedOperations":[0,"x"#,
            Some("MandatoryField"),
            "Eof",
        ),
        (
            "prior-protocol",
            r#"{"schemaVersion":2,"state":{"prepared":"x"#,
            Some("Protocol"),
            "Eof",
        ),
        (
            "prior-duplicate",
            r#"{"state":"prepared","state":{"prepared":"x"#,
            Some("Duplicate"),
            "Eof",
        ),
    ];
    assert_eq!(cases[0].1.len(), 23);
    assert_eq!(
        super::super::prepare::sha256(cases[0].1.as_bytes()),
        "84d1bf3a5ef2a2651c17ce282566de7afc37175c9edbe21292f1169943318e46"
    );
    assert_eq!(cases[9].1.len(), 22);
    let identity = format!(
        r#"{{"projectFingerprint":"{}","state":{{"prepared":"x"#,
        "c".repeat(64)
    );
    for (name, text, refusal, category) in cases.into_iter().chain(std::iter::once((
        "prior-identity",
        identity.as_str(),
        Some("Identity"),
        "Eof",
    ))) {
        let mut r = Input::new(text.as_bytes().to_vec(), vec![1, 7, 3, 127]);
        let actual = describe(&read_state_stream(&mut r, Some(&m)));
        let observation = if refusal.is_none() {
            let mut raw = Input::new(text.as_bytes().to_vec(), vec![1]);
            describe(&read_state_stream(&mut raw, None))
        } else {
            actual.clone()
        };
        let expected = if refusal.is_none() {
            "advisory"
        } else {
            "error"
        };
        let met = actual["return"] == expected
            && observation["refusal"] == json!(refusal)
            && observation["category"] == category
            && observation["phase"] == "Value";
        rows.push(json!({"case":name,"input":text,"stats":r.stats(),"actual":actual,"observation":observation,
            "expected":{"return":expected,"refusal":refusal,"category":category,"phase":"Value"},"met":met}));
    }
    for (name, variant) in [
        ("typed-string", json!("prepared")),
        ("typed-unit", json!({"prepared":null})),
        ("typed-other-unit", json!({"applying":null})),
    ] {
        let mut valid: Value = serde_json::from_str(&state(&m)).unwrap();
        valid["state"] = variant;
        let mut r = Input::new(serde_json::to_vec(&valid).unwrap(), vec![1]);
        let actual = describe(&read_state_stream(&mut r, Some(&m)));
        let met = actual["return"] == "typed" && actual["model_valid"] == true;
        rows.push(json!({"case":name,"actual":actual,"stats":r.stats(),"met":met}));
    }
    save("enum-matrix.json", &json!(rows));
    println!(
        "FIX005_ENUM {}",
        json!({"fixture":base(),"cases":rows.len(),"failed_cases":rows.iter().filter(|r|r["met"]!=true).map(|r|&r["case"]).collect::<Vec<_>>()})
    );
    assert!(
        rows.iter().all(|r| r["met"] == true),
        "enum internal type boundary regression; see safe observations"
    );
}

fn owner_address(e: &io::Error) -> Option<usize> {
    io_cause(e)
        .get_ref()
        .and_then(|e| e.downcast_ref::<OwnedCause>())
        .map(|c| std::ptr::from_ref(c) as usize)
}

#[test]
fn fix005_io_consumers_keep_code_context_and_owner() {
    let m = context();
    let root = base().join("adapter-context/project");
    fs::create_dir_all(root.join("data")).unwrap();
    let lock = crate::data::project_lock::ProjectLock::try_acquire(
        &root,
        &base().join("adapter-context/locks"),
    )
    .unwrap();
    let project = LockedProject::bind(&lock, &root).unwrap();
    let mut plan = super::super::TransactionPlan::new();
    plan.add_json(
        "data/context.json",
        &json!({"schemaVersion":1,"value":"test"}),
    )
    .unwrap();
    let prepared = plan.prepare_for_test(&project).unwrap();
    let mut rows = Vec::new();
    for (name, input, at, phase, refusal) in [
        (
            "value",
            br#"{"schemaVersion":2,"value":"abcdefghijklmnopqrstuvwxyz"}"#.as_slice(),
            29,
            JsonPhase::Value,
            Some(StateRefusal::Protocol),
        ),
        (
            "end",
            br#"{"schemaVersion":2} "#.as_slice(),
            20,
            JsonPhase::End,
            Some(StateRefusal::Protocol),
        ),
        (
            "wrong-type",
            br#"{"schemaVersion":"2"#.as_slice(),
            19,
            JsonPhase::Value,
            Some(StateRefusal::MandatoryField),
        ),
        (
            "unobserved",
            br#"{"schemaVersion":"#.as_slice(),
            5,
            JsonPhase::Value,
            None,
        ),
        (
            "unit-wrong-string",
            br#"{"state":{"prepared":"x"#.as_slice(),
            23,
            JsonPhase::Value,
            Some(StateRefusal::MandatoryField),
        ),
        (
            "unit-null-prefix",
            br#"{"state":{"prepared":n"#.as_slice(),
            22,
            JsonPhase::Value,
            None,
        ),
    ] {
        for custom in [false, true] {
            for route in ["recovery", "commit"] {
                let mut r = Input::new(input.to_vec(), vec![2, 7, 1]).fault(at, custom);
                let result = read_state_stream(&mut r, Some(&m));
                let reader = describe(&result);
                let err = result.unwrap_err();
                assert_eq!(r.delivered, vec![at]);
                assert!(r.fired >= 1);
                assert_private(&err, custom, phase, refusal);
                assert!(redacted(&err));
                let original_address = owner_address(&err);
                assert_eq!(original_address.is_some(), custom);
                let expected_code = if custom { None } else { Some(5) };
                let debug_safe_code = format!("os_code: {expected_code:?}");
                let wrapper_code = format!("{err:?}").contains(&debug_safe_code);
                let downstream = if route == "recovery" {
                    let re = super::super::recovery::fix005_wrap(&project, &m.transaction_id, err);
                    assert_private(&re.failure().source, custom, phase, refusal);
                    assert!(redacted(&re));
                    assert_eq!(owner_address(&re.failure().source), original_address);
                    let rt = crate::data::project_runtime::RuntimeError::fix005_recovery(re);
                    let retained = rt.fix005_original();
                    assert_private(&retained.failure().source, custom, phase, refusal);
                    assert!(redacted(&rt));
                    assert_eq!(owner_address(&retained.failure().source), original_address);
                    let diag = rt.diagnostic();
                    let io = diag.io.unwrap();
                    assert_eq!(io.os_code, expected_code);
                    json!({"stage":format!("{:?}",diag.stage),"os_code":io.os_code,"met":true})
                } else {
                    let ce = super::super::apply::fix005_wrap(&prepared, err);
                    assert!(matches!(
                        ce,
                        super::super::TransactionCommitError::NotApplied(_)
                    ));
                    let super::super::CommitFailureSource::Io(stored) =
                        ce.failure().source.as_ref()
                    else {
                        panic!("missing IO")
                    };
                    assert_private(stored, custom, phase, refusal);
                    assert!(redacted(&ce));
                    assert_eq!(owner_address(stored), original_address);
                    let debug = format!("{ce:?}");
                    let artifact = crate::data::repository::ArtifactCommitError::new(ce);
                    let super::super::CommitFailureSource::Io(stored) =
                        artifact.fix005_original().failure().source.as_ref()
                    else {
                        panic!("missing artifact IO")
                    };
                    assert_private(stored, custom, phase, refusal);
                    assert_eq!(owner_address(stored), original_address);
                    let diag = artifact.diagnostic();
                    assert!(redacted(&artifact));
                    assert_eq!(diag.io.unwrap().os_code, expected_code);
                    json!({"stage":format!("{:?}",diag.stage),"os_code":diag.io.unwrap().os_code,
                        "m1_debug":debug,"met":debug.contains(&debug_safe_code)})
                };
                rows.push(json!({"case":name,"custom":custom,"route":route,"reader":reader,"stats":r.stats(),
                    "downstream":downstream,"private_wrapper_code":wrapper_code,"owner_type_values_and_address_retained":true,"public_redacted":true}));
            }
        }
    }
    let raw = super::super::apply::fix005_wrap(&prepared, io::Error::from_raw_os_error(5));
    assert!(format!("{raw:?}").contains("os_code: Some(5)"));
    assert!(redacted(&raw));
    save("io-propagation.json", &json!(rows));
    println!(
        "FIX005_IO {}",
        json!({"fixture":base(),"objects":rows.len(),"m1_failures":rows.iter().filter(|r|r["downstream"]["met"]!=true).count(),"wrapper_failures":rows.iter().filter(|r|r["private_wrapper_code"]!=true).count()})
    );
    drop(prepared);
    drop(project);
    lock.release().unwrap();
    assert!(
        rows.iter()
            .all(|r| r["downstream"]["met"] == true && r["private_wrapper_code"] == true),
        "existing safe diagnostics must retain original code"
    );
}
