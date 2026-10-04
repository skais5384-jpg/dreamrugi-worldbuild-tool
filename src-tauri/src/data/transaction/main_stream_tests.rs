use super::*;
use std::error::Error;
use std::io::{Cursor, SeekFrom};

#[test]
fn fix004_returned_io_keeps_observation_and_phase() {
    let manifest = fix003_legacy_context();
    for (label, input, at, phase, refusal) in [
        (
            "v2-value",
            br#"{"schemaVersion":2,"value":"abcdefghijklmnopqrstuvwxyz"}"#.as_slice(),
            29,
            JsonPhase::Value,
            StateRefusal::Protocol,
        ),
        // '}'를 받은 뒤 첫 end() read가 실패한다. 값 내부 오류를 End로 이름 붙이지 않는다.
        (
            "v2-end",
            br#"{"schemaVersion":2} "#.as_slice(),
            20,
            JsonPhase::End,
            StateRefusal::Protocol,
        ),
        (
            "wrong-string-value",
            br#"{"schemaVersion":"2"#.as_slice(),
            19,
            JsonPhase::Value,
            StateRefusal::MandatoryField,
        ),
    ] {
        for custom in [false, true] {
            let mut reader = ShortReader::new(input.to_vec(), 1);
            reader.fail_pass = Some(0);
            reader.fail_after = at;
            reader.custom_error = custom;
            let error = read_state_stream(&mut reader, Some(&manifest)).unwrap_err();
            assert_eq!(reader.delivered, vec![at]);
            assert_eq!(reader.rewinds, 0);
            assert!(reader.requests.iter().all(|n| *n <= 8192));
            let retained = error
                .get_ref()
                .and_then(|e| e.downcast_ref::<PrivateIo>())
                .expect("F07 actual returned error must retain context and original I/O");
            assert_eq!(retained.1, Some((phase, Some(refusal))));
            let expected_kind = if custom {
                io::ErrorKind::Other
            } else {
                io::ErrorKind::PermissionDenied
            };
            assert_eq!(error.kind(), expected_kind);
            assert_eq!(io_cause(&error).kind(), expected_kind);
            if custom {
                let cause = io_cause(&error)
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<CanaryCause>())
                    .expect("actual custom cause ownership must survive");
                assert!(
                    cause.owner == 73 && cause.body == "PRIVATE-IO-BODY-PATH-CANARY",
                    "custom cause fields must survive without public rendering"
                );
            } else {
                assert_eq!(io_cause(&error).raw_os_error(), Some(5));
            }
            // 실 반환 객체를 recovery가 쓰는 소유 구조에 옮긴 뒤에도 cause/context를 동시에 조회한다.
            let failure = super::super::recovery::RecoveryFailure {
                stage: super::super::RecoveryStage::InspectJournal,
                transaction_id: Some(manifest.transaction_id.clone()),
                project_fingerprint: manifest.project_fingerprint.clone(),
                target: None,
                source: error,
            };
            assert_eq!(failure.io_cause().kind(), expected_kind);
            if !custom {
                assert_eq!(failure.io_cause().raw_os_error(), Some(5));
            }
            let retained = failure
                .source
                .get_ref()
                .unwrap()
                .downcast_ref::<PrivateIo>()
                .unwrap();
            assert_eq!(retained.1, Some((phase, Some(refusal))));
            let recovery = super::super::RecoveryError::RecoveryRequired(Box::new(failure));
            let mut current: Option<&dyn Error> = Some(&recovery);
            while let Some(error) = current {
                assert!(
                    !format!("{error:?} {error}").contains("CANARY"),
                    "public error chain must redact the private cause"
                );
                current = error.source();
            }
            println!(
                "FIX004_READER {}",
                serde_json::json!({"case":label,"custom":custom,"delivered":at,"phase":format!("{phase:?}"),"refusal":format!("{refusal:?}"),"kind":format!("{expected_kind:?}"),"os_code":recovery.failure().io_cause().raw_os_error(),"cause_context_retained":true,"public_redacted":true})
            );
        }
    }
}

#[derive(Debug)]
struct CanaryCause {
    owner: u32,
    body: &'static str,
}
impl std::fmt::Display for CanaryCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.body)
    }
}
impl Error for CanaryCause {}

#[test]
fn fix004_partial_token_refusals_and_legacy_controls() {
    let manifest = fix003_legacy_context();
    let prefix = format!(r#"{{"padding":"{}","#, "x".repeat(70000));
    let mut cases = vec![
        (
            "string-eof",
            r#"{"schemaVersion":"2"#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ),
        (
            "string-syntax",
            r#"{"schemaVersion":"2\q"#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Syntax,
        ),
        (
            "escaped-key",
            r#"{"schema\u0056ersion":"2"#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ),
        (
            "late-escaped-key",
            format!(r#"{prefix}"schema\u0056ersion":"2"#),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ),
        (
            "version-object",
            r#"{"schemaVersion":{"#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Data,
        ),
        (
            "version-array",
            r#"{"schemaVersion":["#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Data,
        ),
        (
            "version-null-eof",
            r#"{"schemaVersion":n"#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ),
        (
            "version-bool-eof",
            r#"{"schemaVersion":t"#.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ),
        (
            "duplicate-before-string",
            r#"{"schemaVersion":1,"schemaVersion":"2"#.to_owned(),
            StateRefusal::Duplicate,
            serde_json::error::Category::Eof,
        ),
        (
            "protocol-before-string",
            r#"{"schemaVersion":2,"appliedOperations":"2"#.to_owned(),
            StateRefusal::Protocol,
            serde_json::error::Category::Eof,
        ),
        (
            "unknown-before-eof",
            r#"{"schemaVersion":7,"tail":"#.to_owned(),
            StateRefusal::Protocol,
            serde_json::error::Category::Eof,
        ),
        (
            "identity-before-string",
            format!(
                r#"{{"projectFingerprint":"{}","schemaVersion":"2"#,
                "e".repeat(64)
            ),
            StateRefusal::Identity,
            serde_json::error::Category::Eof,
        ),
    ];
    for field in ["transactionId", "projectFingerprint", "updatedAtUtc"] {
        cases.push((
            field,
            format!(r#"{{"schemaVersion":1,"{field}":1e"#),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ));
    }
    // deserialize_enum은 숫자 내용 파싱 없이 ExpectedSomeValue(Syntax)를 반환한다.
    cases.push((
        "state",
        r#"{"schemaVersion":1,"state":1e"#.to_owned(),
        StateRefusal::MandatoryField,
        serde_json::error::Category::Syntax,
    ));
    for (label, text) in [
        (
            "progress-string",
            r#"{"schemaVersion":1,"appliedOperations":"2"#,
        ),
        (
            "progress-first-element",
            r#"{"schemaVersion":1,"appliedOperations":["2"#,
        ),
        (
            "progress-later-element",
            r#"{"schemaVersion":1,"appliedOperations":[0,"2"#,
        ),
    ] {
        cases.push((
            label,
            text.to_owned(),
            StateRefusal::MandatoryField,
            serde_json::error::Category::Eof,
        ));
    }
    for (label, text, refusal, category) in cases {
        let mut reader = ShortReader::new(text.into_bytes(), 1);
        let error = match read_state_stream(&mut reader, Some(&manifest)) {
            Err(error) => error,
            Ok(_) => panic!("observed invalid type must not become advisory"),
        };
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<JsonFailure>()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(failure.phase, JsonPhase::Value);
        assert_eq!(failure.observed, Some(refusal));
        assert_eq!(
            failure.source.classify(),
            category,
            "unexpected original parser category for {label}"
        );
        assert_eq!(reader.rewinds, 0);
        assert!(reader.requests.iter().all(|n| *n <= 8192));
        if category == serde_json::error::Category::Eof {
            assert_eq!(reader.delivered[0], reader.cursor.get_ref().len() as u64);
        }
        println!(
            "FIX004_READER {}",
            serde_json::json!({"case":label,"input_bytes":reader.cursor.get_ref().len(),"delivered":reader.delivered,"phase":"Value","refusal":format!("{refusal:?}"),"original_category":format!("{category:?}")})
        );
    }
    for (label, text) in [
        ("before-value", r#"{"schemaVersion":"#.to_owned()),
        (
            "before-value-whitespace",
            "{\"schemaVersion\": \r\n\t".to_owned(),
        ),
        (
            "number-content-undecided",
            r#"{"schemaVersion":-"#.to_owned(),
        ),
        ("nonjson", "advisory interrupted state".to_owned()),
        ("truncated-v1", r#"{"schemaVersion":1,"state":"#.to_owned()),
        (
            "string-content-undecided",
            r#"{"schemaVersion":1,"updatedAtUtc":"2026-"#.to_owned(),
        ),
        (
            "progress-before-element",
            r#"{"schemaVersion":1,"appliedOperations":[0,"#.to_owned(),
        ),
        (
            "nested-and-string",
            r#"{"nested":{"schemaVersion":"2"},"text":"schemaVersion:2","schemaVersion":1,"tail":"#
                .to_owned(),
        ),
        (
            "late-v1-truncated",
            format!(r#"{prefix}"schemaVersion":1,"tail":"#),
        ),
    ] {
        let mut reader = ShortReader::new(text.into_bytes(), 1);
        assert!(
            matches!(read_state_stream(&mut reader, Some(&manifest)), Ok(None)),
            "compatible advisory control must remain allowed: {label}"
        );
        assert_eq!(reader.rewinds, 0);
        println!(
            "FIX004_READER {}",
            serde_json::json!({"case":label,"delivered":reader.delivered,"advisory":true})
        );
    }
    let full = format!(
        "{prefix}{}",
        std::str::from_utf8(&state(1))
            .unwrap()
            .strip_prefix('{')
            .unwrap()
    );
    let mut reader = ShortReader::new(full.as_bytes().to_vec(), 1);
    let valid = read_state_stream(&mut reader, Some(&manifest))
        .unwrap()
        .unwrap();
    valid.validate().unwrap();
    assert_eq!(valid.schema_version.get(), 1);
    assert_eq!(reader.rewinds, 1);
    assert_eq!(reader.delivered, vec![full.len() as u64, full.len() as u64]);
    println!(
        "FIX004_READER {}",
        serde_json::json!({"case":"late-v1-complete","delivered":reader.delivered,"typed_valid":true,"rewinds":1})
    );
    for suffix in ["garbage", "{}"] {
        let mut input = state(1);
        input.extend_from_slice(suffix.as_bytes());
        let error =
            read_state_stream(&mut ShortReader::new(input, 1), Some(&manifest)).unwrap_err();
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<JsonFailure>()
            .unwrap();
        assert_eq!(failure.phase, JsonPhase::End);
        assert!(failure.source.is_syntax());
        println!(
            "FIX004_READER {}",
            serde_json::json!({"case":if suffix == "{}" {"second-value"} else {"trailing"},"phase":"End","rejected":true})
        );
    }
}

fn state(version: u32) -> Vec<u8> {
    let evidence = if version == 2 {
        r#""originalTargets":[{"targetPath":"documents/00000000-0000-4000-8000-000000000001.json"}],"#
    } else {
        ""
    };
    format!(r#"{{{evidence}"appliedOperations":[],"projectFingerprint":"{}","schemaVersion":{version},"state":"prepared","transactionId":"txn-{}","updatedAtUtc":"2026-09-07T00:00:00.000Z"}}"#, "a".repeat(64), "b".repeat(64)).into_bytes()
}
struct ShortReader {
    cursor: Cursor<Vec<u8>>,
    chunk: usize,
    rewinds: usize,
    fail_pass: Option<usize>,
    fail_after: u64,
    custom_error: bool,
    requests: Vec<usize>,
    delivered: Vec<u64>,
}
impl ShortReader {
    fn new(bytes: Vec<u8>, chunk: usize) -> Self {
        Self {
            cursor: Cursor::new(bytes),
            chunk,
            rewinds: 0,
            fail_pass: None,
            fail_after: 23,
            custom_error: false,
            requests: vec![],
            delivered: vec![0],
        }
    }
}
impl Read for ShortReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.requests.push(buffer.len());
        if self.fail_pass == Some(self.rewinds) && self.cursor.position() >= self.fail_after {
            return Err(if self.custom_error {
                io::Error::other(CanaryCause {
                    owner: 73,
                    body: "PRIVATE-IO-BODY-PATH-CANARY",
                })
            } else {
                io::Error::from_raw_os_error(5)
            });
        }
        let n = buffer.len().min(self.chunk);
        let n = self.cursor.read(&mut buffer[..n])?;
        self.delivered[self.rewinds] += n as u64;
        Ok(n)
    }
}
impl Seek for ShortReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if position == SeekFrom::Start(0) {
            self.rewinds += 1;
            self.delivered.push(0);
        }
        self.cursor.seek(position)
    }
}
#[test]
fn fix002_stream_short_reads_and_same_reader_rewind_validate_eof() {
    let mut bytes = state(1);
    bytes.resize(65537, b' ');
    for chunk in [1, 3, 127, INPUT_BUFFER_BYTES] {
        let mut reader = ShortReader::new(bytes.clone(), chunk);
        let model: TransactionStateRecord = read_main(&mut reader).unwrap();
        model.validate().unwrap();
        assert_eq!(reader.rewinds, 1);
        assert_eq!(reader.delivered, vec![65537, 65537]);
        assert!(reader.requests.iter().all(|n| *n <= INPUT_BUFFER_BYTES));
        assert_eq!(reader.cursor.position(), bytes.len() as u64);
    }
    for suffix in [b"garbage".as_slice(), b"{}"] {
        let mut input = bytes.clone();
        input.extend_from_slice(suffix);
        let error =
            read_main::<TransactionStateRecord>(&mut ShortReader::new(input, 3)).unwrap_err();
        let cause = error
            .get_ref()
            .unwrap()
            .downcast_ref::<JsonFailure>()
            .unwrap();
        assert_eq!(cause.phase, JsonPhase::End);
        assert!(!cause.malformed_value());
    }
}
#[test]
fn fix002_stream_preserves_io_kind_os_code_and_private_cause_in_both_passes() {
    for pass in [0, 1] {
        for at in [23, state(1).len() as u64 + 11] {
            let mut input = state(1);
            input.resize(input.len() + 40, b' ');
            let mut reader = ShortReader::new(input, 3);
            reader.fail_pass = Some(pass);
            reader.fail_after = at;
            let error = read_main::<TransactionStateRecord>(&mut reader).unwrap_err();
            assert_eq!(io_cause(&error).raw_os_error(), Some(5));
            assert_eq!(error.kind(), io::Error::from_raw_os_error(5).kind());
        }
        let mut reader = ShortReader::new(state(1), 3);
        reader.fail_pass = Some(pass);
        reader.custom_error = true;
        let error = read_main::<TransactionStateRecord>(&mut reader).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        let cause = error
            .get_ref()
            .unwrap()
            .downcast_ref::<PrivateIo>()
            .unwrap();
        assert!(
            cause.0.to_string() == "PRIVATE-IO-BODY-PATH-CANARY",
            "original custom I/O must survive"
        );
        assert!(cause.source().is_none());
        assert!(!format!("{error:?} {error} {cause:?}").contains("CANARY"));
        assert!(format!("{cause:?}").contains("MainRead"));
    }
}
#[test]
fn fix002_v2_typed_pass_keeps_existing_cap_and_rejects_invalid_models() {
    for length in [65536, 65537, 131201] {
        let mut input = state(2);
        input.resize(length, b' ');
        let mut reader = ShortReader::new(input, 127);
        let result = read_main::<TransactionStateRecord>(&mut reader);
        assert_eq!(reader.delivered[0], length as u64);
        assert_eq!(
            reader.delivered[1],
            (length as u64).min(MAIN_RECORD_LIMIT + 1)
        );
        if length <= 65536 {
            result.unwrap().validate().unwrap();
        } else {
            let error = result.unwrap_err();
            let cause = error
                .get_ref()
                .unwrap()
                .downcast_ref::<RecordTooLarge>()
                .unwrap();
            assert_eq!(cause.stage, RecordStage::MainRead);
            assert_eq!(cause.limit, MAIN_RECORD_LIMIT);
        }
    }
    // 판별 pass의 성공은 최종 model의 필수 필드/자료형 검증을 대신하지 않는다.
    for text in [
        r#"{"schemaVersion":1}"#,
        r#"{"schemaVersion":2,"state":"unknown"}"#,
        r#"{"schemaVersion":1,"schemaVersion":1}"#,
    ] {
        assert!(read_main::<TransactionStateRecord>(&mut Cursor::new(text.as_bytes())).is_err());
    }
    let nested = format!(
        r#"{{"schemaVersion":1,"appliedOperations":{}0{}}}"#,
        "[".repeat(130),
        "]".repeat(130)
    );
    assert!(read_main::<TransactionStateRecord>(&mut Cursor::new(nested.into_bytes())).is_err());
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let error = stream_json::<serde_json::Value>(deep.as_bytes()).unwrap_err();
    assert!(error
        .source
        .to_string()
        .contains("recursion limit exceeded"));
}

fn fix003_legacy_context() -> TransactionManifest {
    let state: TransactionStateRecord = serde_json::from_slice(&state(1)).unwrap();
    let manifest = TransactionManifest {
        schema_version: state.schema_version,
        transaction_id: state.transaction_id,
        created_at_utc: state.updated_at_utc,
        project_fingerprint: state.project_fingerprint,
        operations: vec![super::super::TransactionOperation {
            index: 0,
            target_path: super::super::ProjectRelativePath::parse("data/control.json").unwrap(),
            staged_path: "staged/000000.json".into(),
            backup_path: None,
            original_existed: false,
            original_size: None,
            original_sha256: None,
            staged_size: 1,
            staged_sha256: "a".repeat(64),
            staged_schema_version: None,
            original_schema_version: None,
            original_raw: false,
        }],
    };
    manifest.validate().unwrap();
    manifest
}
#[test]
fn fix003_state_probe_retains_refusal_original_failure_and_io() {
    let manifest = fix003_legacy_context();
    for suffix in [b"garbage".as_slice(), b"{}"] {
        let mut input = state(1);
        input.extend_from_slice(suffix);
        let error =
            read_state_stream(&mut ShortReader::new(input, 3), Some(&manifest)).unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<JsonFailure>()
                .unwrap()
                .phase,
            JsonPhase::End
        );
    }
    for (input, reason) in [
        (
            format!(
                r#"{{"padding":"{}","schemaVersion":2,"damaged":"#,
                "x".repeat(70000)
            ),
            StateRefusal::Protocol,
        ),
        (
            r#"{"schemaVersion":1,"schemaVersion":1,"damaged":"#.to_owned(),
            StateRefusal::Duplicate,
        ),
        (
            r#"{"schemaVersion":1,"updatedAtUtc":"PRIVATE-CANARY","damaged":"#.to_owned(),
            StateRefusal::MandatoryField,
        ),
    ] {
        let error = read_state_stream(
            &mut ShortReader::new(input.into_bytes(), 3),
            Some(&manifest),
        )
        .unwrap_err();
        let cause = error
            .get_ref()
            .unwrap()
            .downcast_ref::<JsonFailure>()
            .unwrap();
        assert_eq!(cause.observed, Some(reason));
        assert!(cause.source.is_eof());
        assert_eq!(cause.phase, JsonPhase::Value);
        assert!(cause.source().is_none());
        assert!(!format!("{error:?} {error}").contains("PRIVATE-CANARY"));
    }
    for pass in [0, 1] {
        for at in [23, state(1).len() as u64 + 11] {
            let mut input = state(1);
            input.resize(input.len() + 40, b' ');
            let mut reader = ShortReader::new(input, 1);
            reader.fail_pass = Some(pass);
            reader.fail_after = at;
            let error = read_state_stream(&mut reader, Some(&manifest)).unwrap_err();
            assert_eq!(io_cause(&error).raw_os_error(), Some(5));
            assert_eq!(error.kind(), io::Error::from_raw_os_error(5).kind());
        }
        let mut reader = ShortReader::new(state(1), 1);
        reader.fail_pass = Some(pass);
        reader.custom_error = true;
        let error = read_state_stream(&mut reader, Some(&manifest)).unwrap_err();
        let cause = error
            .get_ref()
            .unwrap()
            .downcast_ref::<PrivateIo>()
            .unwrap();
        assert!(
            cause.0.to_string() == "PRIVATE-IO-BODY-PATH-CANARY",
            "original custom I/O must survive"
        );
        assert!(cause.source().is_none());
    }
}
#[test]
fn fix003_original_target_state_capacity_and_two_allocation_bound() {
    use super::super::{model::OriginalTarget, ProjectRelativePath};
    let mut model: TransactionStateRecord = serde_json::from_slice(&state(2)).unwrap();
    for count in [1u32, 32, 128, 512] {
        let targets: Vec<_> = (0..count)
            .map(|index| OriginalTarget {
                target_path: ProjectRelativePath::parse(&format!(
                    "documents/{index:08}-0000-4000-8000-000000000001.json"
                ))
                .unwrap(),
                original_size: Some(u64::MAX),
                original_sha256: Some("f".repeat(64)),
                original_schema_version: Some(SchemaVersion::new_unchecked(u32::MAX)),
                original_raw: false,
            })
            .collect();
        let names: u64 = targets
            .iter()
            .map(|t| t.target_path.as_str().len() as u64 * 6)
            .sum();
        let evidence = crate::data::json::to_deterministic_json_bytes(&targets).unwrap();
        let bound = 64 + u64::from(count) * 384 + names;
        assert!(evidence.len() as u64 <= bound);
        // 상태 교체 때 현재 파일+candidate 두 개와 기존 artifact가 함께 생존한다.
        for unit in [512u64, 4096, 65536] {
            assert!(
                2 * (evidence.len() as u64).div_ceil(unit) * unit
                    <= 2 * bound.div_ceil(unit) * unit
            );
        }
        model.original_targets = Some(targets);
        model.applied_operations = (0..count).collect();
        let bytes = crate::data::json::to_deterministic_json_bytes(&model).unwrap();
        assert_eq!(
            check_state_bound(&model).is_ok(),
            bytes.len() as u64 <= MAIN_RECORD_LIMIT
        );
    }
}
