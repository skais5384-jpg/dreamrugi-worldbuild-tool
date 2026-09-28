//! 주 기록의 필수 version이 복구 방식을 결정한다. 보조 폴더 부재는 버전 증거가 아니다.
use super::{
    model::OWNED_TRANSACTION_SCHEMA_VERSION, prepare::sync_directory, CommittedMarker,
    LockedProject, RolledBackMarker, TransactionId, TransactionManifest, TransactionState,
    TransactionStateRecord, TRANSACTION_SCHEMA_VERSION,
};
use crate::data::{
    project_file::{directory::is_reparse, open_existing_private_file},
    schema::SchemaVersion,
    storage_estimate::TRANSACTION_METADATA_SAFETY_BYTES,
};
use serde::{de::DeserializeOwned, Deserialize};
use std::{
    fs,
    io::{self, BufReader, Read, Seek},
    path::Path,
};

pub(super) const OWNED_DIRECTORY: &str = "owned-temp-v1";
pub(super) const RECORD_LIMIT: u64 = 8192;
// FIX-001에서 도입한 v2 전용 상한이다. 기존 공간 allowance는 v1 입력 cap이 아니다.
// protocol을 결정하기 전 공통 경로에는 이 상한을 적용하지 않는다.
pub(super) const MAIN_RECORD_LIMIT: u64 = TRANSACTION_METADATA_SAFETY_BYTES;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordStage {
    OwnershipRead,
    OwnershipVerify,
    MainRead,
}
#[derive(Debug)]
pub(super) struct RecordTooLarge {
    pub(super) stage: RecordStage,
    pub(super) limit: u64,
}
impl std::fmt::Display for RecordTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "journal record exceeds {} bytes at {:?}",
            self.limit, self.stage
        )
    }
}
impl std::error::Error for RecordTooLarge {}
pub(super) fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
pub(super) fn bounded_read(
    reader: &mut impl Read,
    limit: u64,
    stage: RecordStage,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // take는 실제 읽기 요청까지 제한한다. metadata와 다른 handle을 다시 열지 않는다.
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            RecordTooLarge { stage, limit },
        ));
    }
    Ok(bytes)
}
// 전체 raw JSON 대신 고정 입력 버퍼와 기존 typed model만 보유한다.
// operations Vec 등 완성된 model의 메모리는 입력 규모에 따라 늘어난다.
const INPUT_BUFFER_BYTES: usize = 8192;
#[path = "state_token.rs"]
mod state_token;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonPhase {
    Value,
    End,
}
struct JsonFailure {
    source: serde_json::Error,
    phase: JsonPhase,
    observed: Option<StateRefusal>,
}
impl std::fmt::Debug for JsonFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainJsonFailure")
            .field("stage", &RecordStage::MainRead)
            .field("phase", &self.phase)
            .field("observed", &self.observed)
            .field("category", &self.source.classify())
            .field("line", &self.source.line())
            .field("column", &self.source.column())
            .finish()
    }
}
impl std::fmt::Display for JsonFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid main journal JSON at {:?}", self.phase)
    }
}
impl std::error::Error for JsonFailure {}
// arbitrary I/O 메시지는 보존하되 공개 Debug/Display/source chain에는 올리지 않는다.
struct PrivateIo(io::Error, Option<(JsonPhase, Option<StateRefusal>)>);
impl std::fmt::Debug for PrivateIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainReadIo")
            .field("stage", &RecordStage::MainRead)
            .field("kind", &self.0.kind())
            .field("os_code", &self.0.raw_os_error())
            .field("json_context", &self.1)
            .finish()
    }
}
impl std::fmt::Display for PrivateIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("main journal I/O failed")
    }
}
impl std::error::Error for PrivateIo {}
fn safe_io(error: io::Error) -> io::Error {
    if error.raw_os_error().is_some() {
        error
    } else {
        io::Error::new(error.kind(), PrivateIo(error, None))
    }
}
impl JsonFailure {
    fn into_io(self) -> io::Error {
        if self.source.is_io() {
            // 고정 serde_json 1.0.151의 From 구현은 원 io::Error 자체를 돌려준다.
            // 원 I/O와 부분 관찰을 같이 소유한다. outer kind는 기존 복구 분류를 유지하고,
            // OS code/custom cause는 아래 private accessor로 원 객체에서 조회한다.
            let source: io::Error = self.source.into();
            io::Error::new(
                source.kind(),
                PrivateIo(source, Some((self.phase, self.observed))),
            )
        } else {
            io::Error::new(io::ErrorKind::InvalidData, self)
        }
    }
    fn malformed_value(&self) -> bool {
        self.phase == JsonPhase::Value
            && matches!(
                self.source.classify(),
                serde_json::error::Category::Syntax | serde_json::error::Category::Eof
            )
    }
}
pub(crate) fn io_cause(error: &io::Error) -> &io::Error {
    error
        .get_ref()
        .and_then(|e| e.downcast_ref::<PrivateIo>())
        .map_or(error, |private| &private.0)
}
#[derive(Debug, Clone)]
pub(crate) struct JournalDiagnostic {
    pub(crate) stage: RecordStage,
    pub(crate) phase: Option<JsonPhase>,
    pub(crate) refusal: Option<StateRefusal>,
    pub(crate) json: Option<super::artifact_diagnostics::JsonDiagnostic>,
    pub(crate) limit: Option<u64>,
}
pub(super) fn diagnostic(error: &io::Error) -> Option<(JournalDiagnostic, Option<&io::Error>)> {
    let source = error.get_ref()?;
    let mut result = JournalDiagnostic {
        stage: RecordStage::MainRead,
        phase: None,
        refusal: None,
        json: None,
        limit: None,
    };
    if let Some(private) = source.downcast_ref::<PrivateIo>() {
        if let Some((phase, refusal)) = private.1 {
            result.phase = Some(phase);
            result.refusal = refusal;
        }
        return Some((result, Some(&private.0)));
    }
    if let Some(failure) = source.downcast_ref::<JsonFailure>() {
        result.phase = Some(failure.phase);
        result.refusal = failure.observed;
        result.json = Some((&failure.source).into());
        return Some((result, None));
    }
    if let Some(failure) = source.downcast_ref::<RecordTooLarge>() {
        result.stage = failure.stage;
        result.limit = Some(failure.limit);
        return Some((result, None));
    }
    None
}
fn stream_json<T: DeserializeOwned>(reader: impl Read) -> Result<T, JsonFailure> {
    let mut buffered = BufReader::with_capacity(INPUT_BUFFER_BYTES, reader);
    let mut de = serde_json::Deserializer::from_reader(&mut buffered);
    let value = T::deserialize(&mut de).map_err(|source| JsonFailure {
        source,
        phase: JsonPhase::Value,
        observed: None,
    })?;
    de.end().map_err(|source| JsonFailure {
        source,
        phase: JsonPhase::End,
        observed: None,
    })?;
    Ok(value)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolField {
    schema_version: SchemaVersion,
}

pub(super) trait MainRecord: DeserializeOwned {
    fn version(&self) -> SchemaVersion;
}
macro_rules! main_record {
    ($($t:ty),+) => { $(impl MainRecord for $t { fn version(&self) -> SchemaVersion { self.schema_version } })+ };
}
main_record!(
    TransactionManifest,
    TransactionStateRecord,
    CommittedMarker,
    RolledBackMarker
);

fn supported(version: SchemaVersion) -> io::Result<()> {
    if version == TRANSACTION_SCHEMA_VERSION || version == OWNED_TRANSACTION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(invalid("unknown journal protocol"))
    }
}
fn read_typed<T: MainRecord>(
    file: &mut (impl Read + Seek),
    version: SchemaVersion,
) -> io::Result<T> {
    // 첫 pass의 BufReader/Deserializer는 이미 해제됐다. 같은 검증 File만 되감는다.
    file.rewind().map_err(safe_io)?;
    let value: T = if version == OWNED_TRANSACTION_SCHEMA_VERSION {
        let mut limited = file.take(MAIN_RECORD_LIMIT + 1);
        let result = stream_json(&mut limited);
        if result.as_ref().is_err_and(|failure| failure.source.is_io()) {
            return result.map_err(JsonFailure::into_io);
        }
        // buffer 경계를 EOF로 오인하지 않는다. 초과 시 partial JSON 오류보다 cap 오류를 보존한다.
        if limited.limit() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                RecordTooLarge {
                    stage: RecordStage::MainRead,
                    limit: MAIN_RECORD_LIMIT,
                },
            ));
        }
        result.map_err(JsonFailure::into_io)?
    } else {
        stream_json(file).map_err(JsonFailure::into_io)?
    };
    if value.version() != version {
        return Err(invalid("journal protocol changed while reading"));
    }
    Ok(value)
}
pub(super) fn read_main<T: MainRecord>(file: &mut (impl Read + Seek)) -> io::Result<T> {
    // 필드 위치에 의존하지 않는 구조적 판별이다. 필수 version 중복/누락과 전체 JSON/EOF를 검사한다.
    // 판별 중 무관한 필드는 serde의 IgnoredAny로 건너뛰며 Value/raw 본문을 만들지 않는다.
    let header: ProtocolField = stream_json(&mut *file).map_err(JsonFailure::into_io)?;
    supported(header.schema_version)?;
    read_typed(file, header.schema_version)
}
fn open_main(directory: &Path, name: &str) -> io::Result<Option<fs::File>> {
    let path = directory.join(name);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    open_existing_private_file(&fs::canonicalize(directory)?, &path).map(Some)
}
fn read<T: MainRecord>(directory: &Path, name: &str) -> io::Result<Option<T>> {
    open_main(directory, name)?
        .map(|mut file| read_main(&mut file))
        .transpose()
}
// 관찰은 완성된 state와 별도다. 뒤의 EOF가 이미 읽은 모순을 지우지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateRefusal {
    Protocol,
    Duplicate,
    MandatoryField,
    Identity,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum StateField {
    SchemaVersion,
    TransactionId,
    ProjectFingerprint,
    UpdatedAtUtc,
    State,
    AppliedOperations,
    OriginalTargets,
    #[serde(other)]
    Other,
}
struct StateProbe<'a> {
    tokens: &'a state_token::Tokens,
    legacy: Option<&'a TransactionManifest>,
    version: Option<SchemaVersion>,
    refusal: Option<StateRefusal>,
    seen: u8,
}
impl StateProbe<'_> {
    fn refuse(&mut self, why: StateRefusal) {
        self.refusal.get_or_insert(why);
    }
    fn remember_type(&mut self) {
        if self.tokens.wrong_type.replace(false) {
            self.refuse(StateRefusal::MandatoryField);
        }
    }
    fn value<'de, T: DeserializeOwned, A: serde::de::MapAccess<'de>>(
        &mut self,
        map: &mut A,
        shape: state_token::Shape,
    ) -> Result<T, A::Error> {
        let result = map.next_value_seed(state_token::Value::field(self.tokens, shape));
        // 값 변환이 EOF/Syntax/I/O로 실패해도 첫 token에서 얻은 거부 근거는 남는다.
        self.remember_type();
        result
    }
}
impl<'de> serde::de::Visitor<'de> for &mut StateProbe<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("state object")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        use state_token::Shape;
        while let Some(key) = map.next_key::<StateField>()? {
            let bit = match key {
                StateField::SchemaVersion => 1,
                StateField::TransactionId => 2,
                StateField::ProjectFingerprint => 4,
                StateField::UpdatedAtUtc => 8,
                StateField::State => 16,
                StateField::AppliedOperations => 32,
                StateField::OriginalTargets => 64,
                StateField::Other => 0,
            };
            if self.seen & bit != 0 {
                self.refuse(StateRefusal::Duplicate);
            }
            self.seen |= bit;
            match key {
                StateField::SchemaVersion => {
                    let version: SchemaVersion = self.value(&mut map, Shape::Number)?;
                    self.version = Some(version);
                    if version != TRANSACTION_SCHEMA_VERSION {
                        self.refuse(StateRefusal::Protocol);
                    }
                }
                StateField::TransactionId => {
                    let id: TransactionId = self.value(&mut map, Shape::String)?;
                    if self.legacy.is_some_and(|m| m.transaction_id != id) {
                        self.refuse(StateRefusal::Identity);
                    }
                }
                StateField::ProjectFingerprint => {
                    let fingerprint: String = self.value(&mut map, Shape::String)?;
                    if !super::is_lowercase_sha256(&fingerprint) {
                        self.refuse(StateRefusal::MandatoryField);
                    }
                    if self
                        .legacy
                        .is_some_and(|m| m.project_fingerprint != fingerprint)
                    {
                        self.refuse(StateRefusal::Identity);
                    }
                }
                StateField::UpdatedAtUtc => {
                    let value: String = self.value(&mut map, Shape::String)?;
                    if !crate::data::utc_time::is_utc_milliseconds(&value) {
                        self.refuse(StateRefusal::MandatoryField);
                    }
                }
                StateField::State => {
                    let result = map.next_value_seed(state_token::State(self.tokens));
                    self.remember_type();
                    let value = result?;
                    if matches!(
                        value,
                        TransactionState::CleaningCommitted | TransactionState::CleaningRolledBack
                    ) {
                        self.refuse(StateRefusal::Protocol);
                    }
                }
                StateField::AppliedOperations => {
                    let result = map.next_value_seed(state_token::Progress(self.tokens));
                    self.remember_type();
                    result?;
                }
                StateField::OriginalTargets => {
                    if self.legacy.is_some() {
                        self.refuse(StateRefusal::Protocol);
                    }
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
                StateField::Other => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}
fn read_state_stream(
    file: &mut (impl Read + Seek),
    legacy: Option<&TransactionManifest>,
) -> io::Result<Option<TransactionStateRecord>> {
    use serde::Deserializer;
    let tokens = state_token::Tokens::default();
    let mut probe = StateProbe {
        tokens: &tokens,
        legacy,
        version: None,
        refusal: None,
        seen: 0,
    };
    let result = {
        let mut buffered = BufReader::with_capacity(INPUT_BUFFER_BYTES, &mut *file);
        let input = state_token::TokenReader {
            buffered: &mut buffered,
            tokens: &tokens,
        };
        let mut de = serde_json::Deserializer::from_reader(input);
        de.deserialize_map(&mut probe)
            .map_err(|source| JsonFailure {
                source,
                phase: JsonPhase::Value,
                observed: None,
            })
            .and_then(|()| {
                de.end().map_err(|source| JsonFailure {
                    source,
                    phase: JsonPhase::End,
                    observed: None,
                })
            })
    };
    if let Err(mut failure) = result {
        if failure.source.is_data() && probe.refusal.is_none() {
            probe.refuse(StateRefusal::MandatoryField);
        }
        failure.observed = probe.refusal;
        if legacy.is_some() && probe.refusal.is_none() && failure.malformed_value() {
            return Ok(None);
        }
        return Err(failure.into_io());
    }
    // v2 자체는 정상 입력일 수 있다. advisory 전제인 v1과 관찰이 모순될 때만 거부한다.
    if legacy.is_some() && probe.refusal.is_some() {
        return Err(invalid(
            "legacy state contradicts observed mandatory evidence",
        ));
    }
    let version = probe
        .version
        .ok_or_else(|| invalid("state protocol missing"))?;
    supported(version)?;
    read_typed(file, version).map(Some)
}

fn read_state(
    directory: &Path,
    legacy: Option<&TransactionManifest>,
) -> io::Result<Option<TransactionStateRecord>> {
    let Some(mut file) = open_main(directory, "state.json")? else {
        return Ok(None);
    };
    read_state_stream(&mut file, legacy)
}
pub(super) fn check_state_bound(state: &TransactionStateRecord) -> io::Result<()> {
    if state.schema_version == OWNED_TRANSACTION_SCHEMA_VERSION
        && crate::data::json::to_deterministic_json_bytes(state)
            .map_err(io::Error::other)?
            .len() as u64
            > MAIN_RECORD_LIMIT
    {
        return Err(invalid("owned state exceeds metadata bound"));
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    EmptyPreparing,
    Legacy,
    OwnedPreparing,
    OwnedActive,
    CleaningCommitted,
    CleaningRolledBack,
}
pub(super) struct Journal {
    pub(super) mode: Mode,
    pub(super) manifest: Option<TransactionManifest>,
    pub(super) state: Option<TransactionStateRecord>,
    pub(super) committed: bool,
    pub(super) rolled_back: bool,
}
impl Journal {
    pub(super) fn version(&self) -> io::Result<SchemaVersion> {
        match self.mode {
            Mode::Legacy => Ok(TRANSACTION_SCHEMA_VERSION),
            Mode::OwnedPreparing
            | Mode::OwnedActive
            | Mode::CleaningCommitted
            | Mode::CleaningRolledBack => Ok(OWNED_TRANSACTION_SCHEMA_VERSION),
            Mode::EmptyPreparing => Err(invalid("empty journal has no protocol version")),
        }
    }
    pub(super) fn cleaning(&self) -> bool {
        matches!(
            self.mode,
            Mode::CleaningCommitted | Mode::CleaningRolledBack
        )
    }
    pub(super) fn validate_project(&self, project: &LockedProject<'_>) -> io::Result<()> {
        let fingerprint = self
            .manifest
            .as_ref()
            .map(|m| m.project_fingerprint.as_str())
            .or_else(|| self.state.as_ref().map(|s| s.project_fingerprint.as_str()));
        if fingerprint.is_some_and(|f| f != project.fingerprint()) {
            return Err(invalid("journal belongs to another project"));
        }
        Ok(())
    }
}
pub(super) fn inspect(directory: &Path) -> io::Result<Journal> {
    let manifest: Option<TransactionManifest> = read(directory, "manifest.json")?;
    if let Some(m) = &manifest {
        m.validate()
            .map_err(|_| invalid("invalid manifest model/version"))?;
    }
    let legacy = manifest
        .as_ref()
        .filter(|m| m.schema_version == TRANSACTION_SCHEMA_VERSION);
    let state = read_state(directory, legacy)?;
    let committed: Option<CommittedMarker> = read(directory, "committed.json")?;
    let rolled_back: Option<RolledBackMarker> = read(directory, "rolled-back.json")?;
    if committed.is_some() && rolled_back.is_some() {
        return Err(invalid("contradictory completion markers"));
    }
    let mut identity: Option<(SchemaVersion, &TransactionId, &str)> = None;
    if let Some(m) = &manifest {
        m.validate()
            .map_err(|_| invalid("invalid manifest model/version"))?;
        identity = Some((m.schema_version, &m.transaction_id, &m.project_fingerprint));
    } else if let Some(s) = &state {
        identity = Some((s.schema_version, &s.transaction_id, &s.project_fingerprint));
    } else if let Some(m) = &committed {
        identity = Some((m.schema_version, &m.transaction_id, &m.project_fingerprint));
    } else if let Some(m) = &rolled_back {
        identity = Some((m.schema_version, &m.transaction_id, &m.project_fingerprint));
    }
    // 모든 남은 필수 기록을 검증하며 손상 state를 marker 때문에 무시하지 않는다.
    let check = |version, id: &TransactionId, fingerprint: &str| -> io::Result<()> {
        if directory.file_name().and_then(|s| s.to_str()) != Some(id.as_str()) {
            return Err(invalid("journal transaction identity mismatch"));
        }
        if let Some((v, i, f)) = identity {
            if v != version || i != id || f != fingerprint {
                return Err(invalid("mandatory journal records disagree"));
            }
        }
        Ok(())
    };
    if let Some(m) = &manifest {
        check(m.schema_version, &m.transaction_id, &m.project_fingerprint)?;
    }
    if let Some(s) = &state {
        s.validate()
            .map_err(|_| invalid("invalid state model/version"))?;
        check(s.schema_version, &s.transaction_id, &s.project_fingerprint)?;
    }
    if let Some(m) = &committed {
        m.validate()
            .map_err(|_| invalid("invalid committed model/version"))?;
        check(m.schema_version, &m.transaction_id, &m.project_fingerprint)?;
    }
    if let Some(m) = &rolled_back {
        m.validate()
            .map_err(|_| invalid("invalid rolled-back model/version"))?;
        check(m.schema_version, &m.transaction_id, &m.project_fingerprint)?;
    }
    if let (Some(m), Some(s)) = (&manifest, &state) {
        if m.original_targets() != s.original_targets {
            return Err(invalid("state originals disagree with manifest"));
        }
    }
    let owned = match fs::symlink_metadata(directory.join(OWNED_DIRECTORY)) {
        Ok(m) if m.is_dir() && !is_reparse(&m) => true,
        Ok(_) => return Err(invalid("invalid ownership directory")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    let mode = match identity.map(|i| i.0) {
        None => {
            if owned {
                return Err(invalid("ownership evidence without mandatory version"));
            }
            for name in ["staged", "backups"] {
                if directory_present(directory, name)?
                    && fs::read_dir(directory.join(name))?.next().is_some()
                {
                    return Err(invalid("artifacts without mandatory initial evidence"));
                }
            }
            Mode::EmptyPreparing
        }
        Some(v) if v == TRANSACTION_SCHEMA_VERSION => {
            if owned {
                return Err(invalid("legacy version contradicts ownership evidence"));
            }
            Mode::Legacy
        }
        Some(v) if v == OWNED_TRANSACTION_SCHEMA_VERSION => {
            let s = state
                .as_ref()
                .ok_or_else(|| invalid("owned journal requires state"))?;
            match s.state {
                TransactionState::CleaningCommitted if rolled_back.is_none() => {
                    Mode::CleaningCommitted
                }
                TransactionState::CleaningRolledBack if committed.is_none() => {
                    Mode::CleaningRolledBack
                }
                TransactionState::Preparing
                    if s.applied_operations.is_empty()
                        && committed.is_none()
                        && rolled_back.is_none() =>
                {
                    if owned
                        && fs::read_dir(directory.join(OWNED_DIRECTORY))?
                            .next()
                            .is_some()
                    {
                        return Err(invalid("ownership mutation evidence in preparing journal"));
                    }
                    Mode::OwnedPreparing
                }
                TransactionState::CleaningCommitted
                | TransactionState::CleaningRolledBack
                | TransactionState::Preparing => {
                    return Err(invalid("contradictory protocol state"))
                }
                _ => {
                    if manifest.is_none() || !owned {
                        return Err(invalid("owned journal required evidence missing"));
                    }
                    Mode::OwnedActive
                }
            }
        }
        Some(_) => return Err(invalid("unknown journal protocol")),
    };
    Ok(Journal {
        mode,
        manifest,
        state,
        committed: committed.is_some(),
        rolled_back: rolled_back.is_some(),
    })
}
pub(super) fn active(directory: &Path) -> io::Result<bool> {
    match inspect(directory)?.mode {
        Mode::Legacy => Ok(false),
        Mode::OwnedActive => Ok(true),
        _ => Err(invalid("journal is not active")),
    }
}
/// namespace 의무를 끝낸 뒤에만 cleanup 상태를 durable하게 쓴다.
/// 이 주 기록은 마지막 파일로 지워지므로 중간 재시도는 legacy로 내려가지 않는다.
pub(super) fn finish_cleanup(project: &LockedProject<'_>, directory: &Path) -> io::Result<()> {
    let _evidence = verify_cleanup(project, directory)?;
    for name in ["staged", "backups", OWNED_DIRECTORY] {
        remove_auxiliary(directory, name)?;
    }
    super::owned_temp::checkpoint("cleanup-auxiliary", 0)?;
    for name in ["manifest.json", "committed.json", "rolled-back.json"] {
        let p = directory.join(name);
        match fs::remove_file(p) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
        sync_directory(directory)?;
        super::owned_temp::checkpoint(
            if name == "manifest.json" {
                "cleanup-manifest"
            } else {
                "cleanup-marker"
            },
            0,
        )?;
    }
    // state가 사라지는 순간에는 namespace 임시 객체와 다른 journal 내용이 이미 없다.
    fs::remove_file(directory.join("state.json"))?;
    sync_directory(directory)?;
    super::owned_temp::checkpoint("cleanup-anchor", 0)?;
    fs::remove_dir(directory)?;
    sync_directory(&project.transactions_root())
}
pub(super) fn certify_cleanup(project: &LockedProject<'_>, directory: &Path) -> io::Result<()> {
    let journal = inspect(directory)?;
    journal.validate_project(project)?;
    if journal.cleaning() {
        return finish_cleanup(project, directory);
    }
    if journal.mode != Mode::OwnedActive || !(journal.committed || journal.rolled_back) {
        return Err(invalid("owned cleanup requires verified completion"));
    }
    let mut state = journal
        .state
        .ok_or_else(|| invalid("cleanup state missing"))?;
    let manifest = journal
        .manifest
        .ok_or_else(|| invalid("cleanup manifest missing"))?;
    super::owned_temp::cleanup(project, &manifest.transaction_id, directory, &manifest)?;
    let _evidence = super::owned_temp::verify_completed_targets(
        project,
        directory,
        &manifest,
        journal.committed,
        true,
    )?;
    state.state = if journal.committed {
        TransactionState::CleaningCommitted
    } else {
        TransactionState::CleaningRolledBack
    };
    state.applied_operations.clear();
    state.updated_at_utc =
        crate::data::utc_time::now_utc_milliseconds().map_err(io::Error::other)?;
    check_state_bound(&state)?;
    super::owned_temp::checkpoint("cleanup-before-certificate", 0)?;
    crate::data::atomic_file::save_deterministic_json(&directory.join("state.json"), &state)
        .map_err(io::Error::other)?;
    sync_directory(directory)?;
    super::owned_temp::checkpoint("cleanup-certified", 0)?;
    finish_cleanup(project, directory)
}

// 완료 증명 후 일부 record가 지워질 수 있다. 이 단계는 namespace를 변경하지 않으며,
// 남은 보조 파일에도 크기/경로 상한은 적용한다.
pub(super) fn validate_cleanup_records(directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory.join(OWNED_DIRECTORY))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| invalid("unknown cleanup record"))?;
        let (index, suffix) = name
            .split_once('-')
            .ok_or_else(|| invalid("unknown cleanup record"))?;
        if index.len() != 6
            || !index.bytes().all(|b| b.is_ascii_digit())
            || !matches!(
                suffix,
                "apply.intent.json"
                    | "apply.acquired.json"
                    | "restore.intent.json"
                    | "restore.acquired.json"
            )
        {
            return Err(invalid("unknown cleanup record"));
        }
        let mut file = open_existing_private_file(&fs::canonicalize(directory)?, &entry.path())?;
        bounded_read(&mut file, RECORD_LIMIT, RecordStage::OwnershipRead)?;
    }
    Ok(())
}
pub(super) fn cleanup_preparing(project: &LockedProject<'_>, directory: &Path) -> io::Result<()> {
    let _evidence = verify_preparing(project, directory)?;
    // state의 대상 목록은 마지막까지 남는다. 삭제 실패 뒤에도 원본/temp를 재검증한다.
    for name in ["staged", "backups", OWNED_DIRECTORY] {
        remove_auxiliary(directory, name)?;
    }
    match fs::remove_file(directory.join("manifest.json")) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    sync_directory(directory)?;
    fs::remove_file(directory.join("state.json"))?;
    sync_directory(directory)?;
    fs::remove_dir(directory)?;
    sync_directory(&project.transactions_root())
}

// 검증한 guard를 정리 종료까지 보유하며 raw enum에 삭제 권한을 주지 않는다.
pub(super) fn verify_preparing(
    project: &LockedProject<'_>,
    directory: &Path,
) -> io::Result<super::owned_temp::TargetEvidence> {
    super::recovery::validate_journal_tree(project, &directory_id(directory)?, directory)
        .map_err(io::Error::other)?;
    let journal = inspect(directory)?;
    journal.validate_project(project)?;
    if journal.mode != Mode::OwnedPreparing {
        return Err(invalid("preparing evidence missing"));
    }
    let state = journal
        .state
        .ok_or_else(|| invalid("preparing state missing"))?;
    let targets = state
        .original_targets
        .ok_or_else(|| invalid("preparing original targets missing"))?;
    super::owned_temp::verify_original_targets(project, &state.transaction_id, &targets)
}
fn directory_id(directory: &Path) -> io::Result<TransactionId> {
    TransactionId::parse(
        directory
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("journal name invalid"))?,
    )
    .map_err(|_| invalid("journal ID invalid"))
}
fn directory_present(directory: &Path, name: &str) -> io::Result<bool> {
    match fs::symlink_metadata(directory.join(name)) {
        Ok(m) if m.is_dir() && !is_reparse(&m) => Ok(true),
        Ok(_) => Err(invalid("invalid auxiliary directory")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
pub(super) fn verify_cleanup(
    project: &LockedProject<'_>,
    directory: &Path,
) -> io::Result<Option<super::owned_temp::TargetEvidence>> {
    super::recovery::validate_journal_tree(project, &directory_id(directory)?, directory)
        .map_err(io::Error::other)?;
    let journal = inspect(directory)?;
    journal.validate_project(project)?;
    if !journal.cleaning() {
        return Err(invalid("cleanup requires validated evidence"));
    }
    let staged = directory_present(directory, "staged")?;
    let backups = directory_present(directory, "backups")?;
    let owned = directory_present(directory, OWNED_DIRECTORY)?;
    let Some(manifest) = &journal.manifest else {
        if staged || backups || owned {
            return Err(invalid("auxiliary evidence remains after manifest removal"));
        }
        return Ok(None);
    };
    let committed = journal.mode == Mode::CleaningCommitted;
    if (committed && !journal.committed) || (!committed && !journal.rolled_back) {
        return Err(invalid(
            "manifest cleanup requires matching completion marker",
        ));
    }
    if (staged && !backups) || ((staged || backups) && !owned) {
        return Err(invalid("auxiliary removal order contradicts completion"));
    }
    for (name, present) in [("staged", staged), ("backups", backups)] {
        if !present {
            continue;
        }
        let mut allowed = std::collections::BTreeMap::new();
        for op in &manifest.operations {
            if name == "staged" {
                allowed.insert(
                    op.staged_path.as_str(),
                    (op.staged_size, op.staged_sha256.as_str()),
                );
            } else if let (Some(path), Some(size), Some(hash)) =
                (&op.backup_path, op.original_size, &op.original_sha256)
            {
                allowed.insert(path.as_str(), (size, hash.as_str()));
                if staged && open_main(directory, path)?.is_none() {
                    return Err(invalid("backup removed before staged directory"));
                }
            }
        }
        for entry in fs::read_dir(directory.join(name))? {
            let entry = entry?;
            let relative = format!(
                "{name}/{}",
                entry
                    .file_name()
                    .to_str()
                    .ok_or_else(|| invalid("invalid artifact name"))?
            );
            let (size, hash) = allowed
                .get(relative.as_str())
                .ok_or_else(|| invalid("unexpected completion artifact"))?;
            let mut file = open_main(directory, &relative)?
                .ok_or_else(|| invalid("completion artifact disappeared"))?;
            let mut bytes = Vec::new();
            file.by_ref()
                .take(
                    size.checked_add(1)
                        .ok_or_else(|| invalid("artifact size overflow"))?,
                )
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 != *size || super::prepare::sha256(&bytes) != *hash {
                return Err(invalid("completion artifact differs from manifest"));
            }
        }
    }
    super::owned_temp::verify_completed_targets(
        project,
        directory,
        manifest,
        committed,
        staged || backups,
    )
    .map(Some)
}
fn remove_auxiliary(directory: &Path, name: &str) -> io::Result<()> {
    if !directory_present(directory, name)? {
        return Ok(());
    }
    let path = directory.join(name);
    let mut entries = fs::read_dir(&path)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for (index, entry) in entries.into_iter().enumerate() {
        fs::remove_file(entry.path())?;
        sync_directory(&path)?;
        super::owned_temp::checkpoint(
            &format!("cleanup-{name}-file"),
            u32::try_from(index).map_err(|_| invalid("cleanup index overflow"))?,
        )?;
    }
    fs::remove_dir(&path)?;
    sync_directory(directory)?;
    super::owned_temp::checkpoint(&format!("cleanup-{name}"), 0)
}

#[cfg(test)]
#[path = "main_stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_record_reader_caps_consumption_and_preserves_category() {
        for stage in [
            RecordStage::OwnershipRead,
            RecordStage::OwnershipVerify,
            RecordStage::MainRead,
        ] {
            let limit = if stage == RecordStage::MainRead {
                MAIN_RECORD_LIMIT
            } else {
                RECORD_LIMIT
            };
            for length in [limit, limit + 1, limit * 2] {
                let mut cursor = io::Cursor::new(vec![b' '; length as usize]);
                let result = bounded_read(&mut cursor, limit, stage);
                assert_eq!(cursor.position(), length.min(limit + 1));
                if length <= limit {
                    assert_eq!(result.unwrap().len() as u64, length);
                } else {
                    let error = result.unwrap_err();
                    let detail = error
                        .get_ref()
                        .unwrap()
                        .downcast_ref::<RecordTooLarge>()
                        .unwrap();
                    assert_eq!(detail.stage, stage);
                    assert_eq!(detail.limit, limit);
                    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                }
            }
        }
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from_raw_os_error(5))
            }
        }
        assert_eq!(
            bounded_read(&mut Failed, RECORD_LIMIT, RecordStage::OwnershipRead)
                .unwrap_err()
                .raw_os_error(),
            Some(5)
        );
    }
}

#[cfg(test)]
#[path = "enum_io_tests.rs"]
mod enum_io_tests;
