use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// transaction metadata와 상태 파일의 논리 크기 오차를 흡수하는 고정 여유분이다.
pub(crate) const TRANSACTION_METADATA_SAFETY_BYTES: u64 = 64 * 1024;

/// admission 이후 commit/rollback/recovery가 작은 상태 기록과 한 파일 복원을
/// 계속할 수 있도록 남기는 정책 여유분이다. 실제 공간을 예약하는 값은 아니다.
pub(crate) const TRANSACTION_OPERATIONAL_RESERVE_BYTES: u64 = 8 * 1024 * 1024;

const MANIFEST_BASE_BYTES: u64 = 1024;
const MANIFEST_OPERATION_BYTES: u64 = 768;
const STATE_BASE_BYTES: u64 = 1024;
const STATE_OPERATION_BYTES: u64 = 16;
const MARKER_BYTES: u64 = 1024;
const BASE_FILESYSTEM_METADATA_UNITS: u64 = 8;
const OPERATION_FILESYSTEM_METADATA_UNITS: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageAdmissionDecision {
    Admitted,
    Insufficient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StorageAdmission {
    available_bytes: u64,
    required_peak_bytes: u64,
    metadata_allowance_bytes: u64,
    operational_reserve_bytes: u64,
    allocation_unit_bytes: u64,
    decision: StorageAdmissionDecision,
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "M1-9 admission report는 후속 프로젝트 열기 계층에서 표시한다"
    )
)]
impl StorageAdmission {
    pub(crate) fn available_bytes(self) -> u64 {
        self.available_bytes
    }

    pub(crate) fn required_peak_bytes(self) -> u64 {
        self.required_peak_bytes
    }

    pub(crate) fn metadata_allowance_bytes(self) -> u64 {
        self.metadata_allowance_bytes
    }

    pub(crate) fn operational_reserve_bytes(self) -> u64 {
        self.operational_reserve_bytes
    }

    pub(crate) fn allocation_unit_bytes(self) -> u64 {
        self.allocation_unit_bytes
    }

    pub(crate) fn decision(self) -> StorageAdmissionDecision {
        self.decision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "지원 플랫폼별 query category를 하나의 안정된 오류 모델로 유지한다"
    )
)]
pub(crate) enum StorageQueryErrorKind {
    QueryFailed,
    UnsupportedPlatform,
    ArithmeticOverflow,
    CrossFilesystem,
}

#[derive(Debug)]
pub(crate) enum StorageAdmissionError {
    Query {
        kind: StorageQueryErrorKind,
        source: Option<io::Error>,
    },
    EstimateOverflow,
    Insufficient(StorageAdmission),
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "M1-9 오류 category accessor는 후속 프로젝트 열기 계층에서 소비한다"
    )
)]
impl StorageAdmissionError {
    fn query(kind: StorageQueryErrorKind, source: Option<io::Error>) -> Self {
        Self::Query { kind, source }
    }

    pub(crate) fn kind(&self) -> Option<StorageQueryErrorKind> {
        match self {
            Self::Query { kind, .. } => Some(*kind),
            Self::EstimateOverflow | Self::Insufficient(_) => None,
        }
    }

    pub(crate) fn admission(&self) -> Option<StorageAdmission> {
        match self {
            Self::Insufficient(admission) => Some(*admission),
            Self::Query { .. } | Self::EstimateOverflow => None,
        }
    }
}

impl fmt::Display for StorageAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query { kind, .. } => {
                write!(formatter, "transaction storage query failed: {kind:?}")
            }
            Self::EstimateOverflow => {
                formatter.write_str("transaction peak storage estimate overflowed")
            }
            Self::Insufficient(admission) => write!(
                formatter,
                "transaction storage admission denied: {} bytes available, {} bytes required",
                admission.available_bytes, admission.required_peak_bytes
            ),
        }
    }
}

impl Error for StorageAdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Query {
                source: Some(source),
                ..
            } => Some(source),
            Self::Query { source: None, .. } | Self::EstimateOverflow | Self::Insufficient(_) => {
                None
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TransactionStorageInput<'a> {
    pub(crate) staged_sizes: &'a [u64],
    pub(crate) backup_sizes: &'a [Option<u64>],
    pub(crate) target_name_bytes: &'a [u64],
    pub(crate) minimum_required_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct FilesystemSnapshot {
    caller_available_bytes: u64,
    allocation_unit_bytes: u64,
}

pub(crate) fn admit_transaction_storage(
    artifact_path: &Path,
    target_paths: &[PathBuf],
    input: TransactionStorageInput<'_>,
) -> Result<StorageAdmission, StorageAdmissionError> {
    let snapshot = query_artifact_filesystem(artifact_path, target_paths)?;
    evaluate_admission(snapshot, input)
}

pub(crate) fn admit_owned_transaction_storage(
    artifact_path: &Path,
    target_paths: &[PathBuf],
    input: TransactionStorageInput<'_>,
) -> Result<StorageAdmission, StorageAdmissionError> {
    let snapshot = query_artifact_filesystem(artifact_path, target_paths)?;
    let unit = snapshot.allocation_unit_bytes;
    let base = estimate_peak(unit, input)?;
    let stage = checked_sum(input.staged_sizes.iter().map(|v| round_up(*v, unit)))?;
    let backup = checked_sum(
        input
            .backup_sizes
            .iter()
            .flatten()
            .map(|v| round_up(*v, unit)),
    )?;
    let largest_stage = input.staged_sizes.iter().try_fold(0u64, |m, v| {
        Ok::<_, StorageAdmissionError>(m.max(round_up(*v, unit)?))
    })?;
    let operations = input.staged_sizes.len() as u64;
    let records = operations
        .checked_mul(4)
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    // staged/backup을 계속 보유하므로 새 target과 복원 target의 추가 파일 metadata도
    // 각각 한 단위씩 더한다. 기존 target 삭제로 돌아올 공간은 기대하지 않는다.
    let metadata_units = operations
        .checked_mul(2)
        .and_then(|n| n.checked_add(records))
        .and_then(|n| n.checked_add(3))
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    let metadata = records
        .checked_mul(round_up(8192, unit)?)
        .and_then(|n| n.checked_add(unit.checked_mul(metadata_units)?))
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    let evidence_logical = checked_sum([
        Ok(64),
        operations
            .checked_mul(384)
            .ok_or(StorageAdmissionError::EstimateOverflow),
        checked_sum(input.target_name_bytes.iter().map(|n| {
            n.checked_mul(6)
                .ok_or(StorageAdmissionError::EstimateOverflow)
        })),
    ])?;
    let two_evidence_allocations = round_up(evidence_logical, unit)?
        .checked_mul(2)
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    let required = checked_sum([
        Ok(base.required_peak_bytes),
        Ok(two_evidence_allocations),
        Ok(stage),
        Ok(backup),
        Ok(largest_stage),
        Ok(metadata),
    ])?;
    let mut extended = input;
    extended.minimum_required_bytes = required;
    evaluate_admission(snapshot, extended)
}

fn evaluate_admission(
    snapshot: FilesystemSnapshot,
    input: TransactionStorageInput<'_>,
) -> Result<StorageAdmission, StorageAdmissionError> {
    let requirement = estimate_peak(snapshot.allocation_unit_bytes, input)?;
    let decision = if snapshot.caller_available_bytes >= requirement.required_peak_bytes {
        StorageAdmissionDecision::Admitted
    } else {
        StorageAdmissionDecision::Insufficient
    };
    let admission = StorageAdmission {
        available_bytes: snapshot.caller_available_bytes,
        required_peak_bytes: requirement.required_peak_bytes,
        metadata_allowance_bytes: requirement.metadata_allowance_bytes,
        operational_reserve_bytes: requirement.operational_reserve_bytes,
        allocation_unit_bytes: snapshot.allocation_unit_bytes,
        decision,
    };
    if decision == StorageAdmissionDecision::Admitted {
        Ok(admission)
    } else {
        Err(StorageAdmissionError::Insufficient(admission))
    }
}

#[derive(Debug, Clone, Copy)]
struct StorageRequirement {
    required_peak_bytes: u64,
    metadata_allowance_bytes: u64,
    operational_reserve_bytes: u64,
}

fn estimate_peak(
    allocation_unit: u64,
    input: TransactionStorageInput<'_>,
) -> Result<StorageRequirement, StorageAdmissionError> {
    if allocation_unit == 0
        || input.staged_sizes.len() != input.backup_sizes.len()
        || input.staged_sizes.len() != input.target_name_bytes.len()
    {
        return Err(StorageAdmissionError::EstimateOverflow);
    }

    let operation_count = u64::try_from(input.staged_sizes.len())
        .map_err(|_| StorageAdmissionError::EstimateOverflow)?;
    let staged_allocated = checked_sum(
        input
            .staged_sizes
            .iter()
            .map(|size| round_up(*size, allocation_unit)),
    )?;
    let backup_allocated = checked_sum(
        input
            .backup_sizes
            .iter()
            .flatten()
            .map(|size| round_up(*size, allocation_unit)),
    )?;
    let largest_backup = input
        .backup_sizes
        .iter()
        .flatten()
        .try_fold(0_u64, |largest, size| {
            Ok::<_, StorageAdmissionError>(largest.max(round_up(*size, allocation_unit)?))
        })?;

    // JSON escaping의 최악값(한 입력 byte당 \u00XX 6 bytes)을 사용해 manifest를
    // 실제 serialization보다 작게 잡지 않는다.
    let escaped_target_bytes = checked_sum(input.target_name_bytes.iter().map(|length| {
        length
            .checked_mul(6)
            .ok_or(StorageAdmissionError::EstimateOverflow)
    }))?;
    let manifest_logical = checked_sum([
        Ok(MANIFEST_BASE_BYTES),
        operation_count
            .checked_mul(MANIFEST_OPERATION_BYTES)
            .ok_or(StorageAdmissionError::EstimateOverflow),
        Ok(escaped_target_bytes),
    ])?;
    let state_logical = checked_sum([
        Ok(STATE_BASE_BYTES),
        operation_count
            .checked_mul(STATE_OPERATION_BYTES)
            .ok_or(StorageAdmissionError::EstimateOverflow),
    ])?;

    let manifest_allocated = round_up(manifest_logical, allocation_unit)?;
    let state_allocated = round_up(state_logical, allocation_unit)?;
    let marker_allocated = round_up(MARKER_BYTES, allocation_unit)?;
    let metadata_allowance = round_up(TRANSACTION_METADATA_SAFETY_BYTES, allocation_unit)?;
    let fixed_operational_reserve =
        round_up(TRANSACTION_OPERATIONAL_RESERVE_BYTES, allocation_unit)?;
    let operational_reserve = fixed_operational_reserve
        .checked_add(largest_backup)
        .ok_or(StorageAdmissionError::EstimateOverflow)?;

    let metadata_units = BASE_FILESYSTEM_METADATA_UNITS
        .checked_add(
            operation_count
                .checked_mul(OPERATION_FILESYSTEM_METADATA_UNITS)
                .ok_or(StorageAdmissionError::EstimateOverflow)?,
        )
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    let filesystem_metadata = metadata_units
        .checked_mul(allocation_unit)
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    let two_state_files = state_allocated
        .checked_mul(2)
        .ok_or(StorageAdmissionError::EstimateOverflow)?;
    let calculated_peak = checked_sum([
        Ok(staged_allocated),
        Ok(backup_allocated),
        Ok(manifest_allocated),
        Ok(two_state_files),
        Ok(marker_allocated),
        Ok(filesystem_metadata),
        Ok(metadata_allowance),
        Ok(operational_reserve),
    ])?;

    Ok(StorageRequirement {
        required_peak_bytes: calculated_peak.max(input.minimum_required_bytes),
        metadata_allowance_bytes: metadata_allowance,
        operational_reserve_bytes: operational_reserve,
    })
}

fn round_up(value: u64, unit: u64) -> Result<u64, StorageAdmissionError> {
    if unit == 0 {
        return Err(StorageAdmissionError::EstimateOverflow);
    }
    if value == 0 {
        return Ok(0);
    }
    let units = value
        .checked_add(unit - 1)
        .ok_or(StorageAdmissionError::EstimateOverflow)?
        / unit;
    units
        .checked_mul(unit)
        .ok_or(StorageAdmissionError::EstimateOverflow)
}

fn checked_sum(
    values: impl IntoIterator<Item = Result<u64, StorageAdmissionError>>,
) -> Result<u64, StorageAdmissionError> {
    values.into_iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(value?)
            .ok_or(StorageAdmissionError::EstimateOverflow)
    })
}

fn query_artifact_filesystem(
    artifact_path: &Path,
    target_paths: &[PathBuf],
) -> Result<FilesystemSnapshot, StorageAdmissionError> {
    #[cfg(test)]
    if let Some(result) = test_support::query_override(artifact_path, target_paths) {
        return result;
    }
    os::query_artifact_filesystem(artifact_path, target_paths)
}

fn existing_probe(path: &Path) -> Result<PathBuf, StorageAdmissionError> {
    let mut candidate = path.to_path_buf();
    loop {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !candidate.pop() {
                    return Err(StorageAdmissionError::query(
                        StorageQueryErrorKind::QueryFailed,
                        Some(error),
                    ));
                }
            }
            Err(source) => {
                return Err(StorageAdmissionError::query(
                    StorageQueryErrorKind::QueryFailed,
                    Some(source),
                ));
            }
        }
    }
}

#[cfg(windows)]
mod os {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use super::*;

    const VOLUME_PATH_CAPACITY: usize = 32_768;

    #[derive(Debug)]
    struct VolumeSnapshot {
        available: u64,
        allocation_unit: u64,
        identity: Vec<u16>,
    }

    pub(super) fn query_artifact_filesystem(
        artifact_path: &Path,
        target_paths: &[PathBuf],
    ) -> Result<FilesystemSnapshot, StorageAdmissionError> {
        let artifact = query_one(&existing_probe(artifact_path)?)?;
        for target in target_paths {
            let target = query_one(&existing_probe(target)?)?;
            if target.identity != artifact.identity {
                return Err(StorageAdmissionError::query(
                    StorageQueryErrorKind::CrossFilesystem,
                    None,
                ));
            }
        }
        Ok(FilesystemSnapshot {
            caller_available_bytes: artifact.available,
            allocation_unit_bytes: artifact.allocation_unit,
        })
    }

    fn query_one(path: &Path) -> Result<VolumeSnapshot, StorageAdmissionError> {
        let metadata = std::fs::metadata(path).map_err(query_failed)?;
        let directory = if metadata.is_dir() {
            path
        } else {
            path.parent().ok_or_else(|| {
                StorageAdmissionError::query(StorageQueryErrorKind::QueryFailed, None)
            })?
        };
        let directory_wide = wide_null(directory.as_os_str())?;
        let mut available = 0_u64;
        // SAFETY: 입력은 NUL 종료된 읽기 전용 buffer이고, 결과 포인터는 유효한 u64다.
        let free_ok = unsafe {
            GetDiskFreeSpaceExW(
                directory_wide.as_ptr(),
                &mut available,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if free_ok == 0 {
            return Err(query_failed(io::Error::last_os_error()));
        }

        let path_wide = wide_null(path.as_os_str())?;
        let mut volume_path = vec![0_u16; VOLUME_PATH_CAPACITY];
        let capacity = u32::try_from(volume_path.len()).map_err(|_| {
            StorageAdmissionError::query(StorageQueryErrorKind::ArithmeticOverflow, None)
        })?;
        // SAFETY: 두 buffer는 호출 기간 유효하고 출력 길이는 전달한 capacity로 제한된다.
        let volume_ok =
            unsafe { GetVolumePathNameW(path_wide.as_ptr(), volume_path.as_mut_ptr(), capacity) };
        if volume_ok == 0 {
            return Err(query_failed(io::Error::last_os_error()));
        }
        let end = volume_path
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| {
                StorageAdmissionError::query(StorageQueryErrorKind::QueryFailed, None)
            })?;
        volume_path.truncate(end + 1);

        let mut sectors_per_cluster = 0_u32;
        let mut bytes_per_sector = 0_u32;
        let mut free_clusters = 0_u32;
        let mut total_clusters = 0_u32;
        // SAFETY: volume_path는 GetVolumePathNameW가 만든 NUL 종료 root이고 네 출력 포인터는
        // 모두 호출 기간 유효한 u32를 가리킨다.
        let allocation_ok = unsafe {
            GetDiskFreeSpaceW(
                volume_path.as_ptr(),
                &mut sectors_per_cluster,
                &mut bytes_per_sector,
                &mut free_clusters,
                &mut total_clusters,
            )
        };
        if allocation_ok == 0 {
            return Err(query_failed(io::Error::last_os_error()));
        }
        let allocation_unit = u64::from(sectors_per_cluster)
            .checked_mul(u64::from(bytes_per_sector))
            .filter(|value| *value != 0)
            .ok_or_else(|| {
                StorageAdmissionError::query(StorageQueryErrorKind::ArithmeticOverflow, None)
            })?;

        // Win32가 반환한 volume root에서 ASCII drive/prefix case만 정규화한다.
        for unit in &mut volume_path {
            if *unit >= u16::from(b'A') && *unit <= u16::from(b'Z') {
                *unit += u16::from(b'a' - b'A');
            }
        }
        Ok(VolumeSnapshot {
            available,
            allocation_unit,
            identity: volume_path,
        })
    }

    fn wide_null(value: &OsStr) -> Result<Vec<u16>, StorageAdmissionError> {
        let mut wide = value.encode_wide().collect::<Vec<_>>();
        if wide.contains(&0) {
            return Err(StorageAdmissionError::query(
                StorageQueryErrorKind::QueryFailed,
                Some(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "filesystem query path contains NUL",
                )),
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    fn query_failed(source: io::Error) -> StorageAdmissionError {
        StorageAdmissionError::query(StorageQueryErrorKind::QueryFailed, Some(source))
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available_to_caller: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;
        fn GetDiskFreeSpaceW(
            root_path_name: *const u16,
            sectors_per_cluster: *mut u32,
            bytes_per_sector: *mut u32,
            number_of_free_clusters: *mut u32,
            total_number_of_clusters: *mut u32,
        ) -> i32;
        fn GetVolumePathNameW(
            file_name: *const u16,
            volume_path_name: *mut u16,
            buffer_length: u32,
        ) -> i32;
    }
}

#[cfg(all(
    any(target_os = "linux", target_os = "macos"),
    target_pointer_width = "64"
))]
mod os {
    use std::ffi::{c_char, c_int, c_ulong, CString};
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    use super::*;

    #[repr(C)]
    struct StatVfs {
        f_bsize: c_ulong,
        f_frsize: c_ulong,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: c_ulong,
        f_flag: c_ulong,
        f_namemax: c_ulong,
        #[cfg(target_os = "linux")]
        spare: [c_int; 6],
    }

    #[derive(Debug)]
    struct VolumeSnapshot {
        available: u64,
        allocation_unit: u64,
        device: u64,
        #[cfg(target_os = "linux")]
        mount_id: u64,
    }

    pub(super) fn query_artifact_filesystem(
        artifact_path: &Path,
        target_paths: &[PathBuf],
    ) -> Result<FilesystemSnapshot, StorageAdmissionError> {
        #[cfg(target_os = "linux")]
        let mounts = super::linux_mountinfo::MountTable::read_current().map_err(query_failed)?;
        let artifact = query_one(
            &existing_probe(artifact_path)?,
            #[cfg(target_os = "linux")]
            &mounts,
        )?;
        for target in target_paths {
            let target = query_one(
                &existing_probe(target)?,
                #[cfg(target_os = "linux")]
                &mounts,
            )?;
            if !same_filesystem(&artifact, &target) {
                return Err(StorageAdmissionError::query(
                    StorageQueryErrorKind::CrossFilesystem,
                    None,
                ));
            }
        }
        Ok(FilesystemSnapshot {
            caller_available_bytes: artifact.available,
            allocation_unit_bytes: artifact.allocation_unit,
        })
    }

    fn query_one(
        path: &Path,
        #[cfg(target_os = "linux")] mounts: &super::linux_mountinfo::MountTable,
    ) -> Result<VolumeSnapshot, StorageAdmissionError> {
        // mountinfo의 component 비교와 statvfs/metadata가 같은 실제 대상을 보도록 먼저
        // existing probe를 canonical path로 고정한다. 이후 topology 교체는 snapshot TOCTOU다.
        let canonical = std::fs::canonicalize(path).map_err(query_failed)?;
        let path_bytes = CString::new(canonical.as_os_str().as_bytes()).map_err(|source| {
            StorageAdmissionError::query(
                StorageQueryErrorKind::QueryFailed,
                Some(io::Error::new(io::ErrorKind::InvalidInput, source)),
            )
        })?;
        let mut raw = MaybeUninit::<StatVfs>::uninit();
        // SAFETY: path_bytes는 NUL 종료되고 raw는 statvfs 전체 출력 크기와 정렬을 가진다.
        if unsafe { statvfs(path_bytes.as_ptr(), raw.as_mut_ptr()) } != 0 {
            return Err(StorageAdmissionError::query(
                StorageQueryErrorKind::QueryFailed,
                Some(io::Error::last_os_error()),
            ));
        }
        // SAFETY: 성공한 statvfs가 구조체 전체를 초기화했다.
        let raw = unsafe { raw.assume_init() };
        let allocation_unit = if raw.f_frsize == 0 {
            raw.f_bsize
        } else {
            raw.f_frsize
        };
        let allocation_unit = u64::try_from(allocation_unit)
            .ok()
            .filter(|value| *value != 0)
            .ok_or_else(|| {
                StorageAdmissionError::query(StorageQueryErrorKind::ArithmeticOverflow, None)
            })?;
        let available = raw.f_bavail.checked_mul(allocation_unit).ok_or_else(|| {
            StorageAdmissionError::query(StorageQueryErrorKind::ArithmeticOverflow, None)
        })?;
        let device = std::fs::metadata(&canonical).map_err(query_failed)?;
        #[cfg(target_os = "linux")]
        let mount_id = mounts
            .mount_id_for(canonical.as_os_str().as_bytes())
            .map_err(query_failed)?;
        Ok(VolumeSnapshot {
            available,
            allocation_unit,
            device: device.dev(),
            #[cfg(target_os = "linux")]
            mount_id,
        })
    }

    fn same_filesystem(left: &VolumeSnapshot, right: &VolumeSnapshot) -> bool {
        if left.device != right.device {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            super::linux_mountinfo::same_identity(
                left.device,
                left.mount_id,
                right.device,
                right.mount_id,
            )
        }
        #[cfg(target_os = "macos")]
        {
            true
        }
    }

    fn query_failed(source: io::Error) -> StorageAdmissionError {
        StorageAdmissionError::query(StorageQueryErrorKind::QueryFailed, Some(source))
    }

    unsafe extern "C" {
        fn statvfs(path: *const c_char, buffer: *mut StatVfs) -> c_int;
    }
}

#[cfg(any(test, target_os = "linux"))]
mod linux_mountinfo {
    use std::collections::HashSet;
    #[cfg(target_os = "linux")]
    use std::fs;
    use std::io;

    #[cfg(target_os = "linux")]
    const MOUNTINFO_PATH: &str = "/proc/self/mountinfo";

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct MountEntry {
        id: u64,
        mount_point: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct MountTable {
        entries: Vec<MountEntry>,
    }

    impl MountTable {
        #[cfg(target_os = "linux")]
        pub(super) fn read_current() -> io::Result<Self> {
            let bytes = fs::read(MOUNTINFO_PATH)?;
            Self::parse(&bytes)
        }

        pub(super) fn parse(bytes: &[u8]) -> io::Result<Self> {
            if bytes.is_empty() {
                return Err(invalid_mountinfo());
            }
            let mut entries = Vec::new();
            let mut ids = HashSet::new();
            let lines = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
            for (index, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    if index + 1 == lines.len() && bytes.ends_with(b"\n") {
                        continue;
                    }
                    return Err(invalid_mountinfo());
                }
                if line.contains(&b'\r') {
                    return Err(invalid_mountinfo());
                }
                let fields = line.split(|byte| *byte == b' ').collect::<Vec<_>>();
                if fields.iter().any(|field| field.is_empty()) {
                    return Err(invalid_mountinfo());
                }
                let separators = fields
                    .iter()
                    .enumerate()
                    .filter_map(|(index, field)| (*field == b"-").then_some(index))
                    .collect::<Vec<_>>();
                let [separator] = separators.as_slice() else {
                    return Err(invalid_mountinfo());
                };
                if *separator < 6 || fields.len() != separator + 4 {
                    return Err(invalid_mountinfo());
                }

                let id = parse_decimal(fields[0])?;
                let _parent_id = parse_decimal(fields[1])?;
                parse_device(fields[2])?;
                let root = decode_path(fields[3])?;
                let mount_point = decode_path(fields[4])?;
                if !is_absolute_normalized(&root) || !is_absolute_normalized(&mount_point) {
                    return Err(invalid_mountinfo());
                }
                if !ids.insert(id) {
                    return Err(invalid_mountinfo());
                }
                entries.push(MountEntry { id, mount_point });
            }
            if entries.is_empty() {
                return Err(invalid_mountinfo());
            }
            Ok(Self { entries })
        }

        pub(super) fn mount_id_for(&self, path: &[u8]) -> io::Result<u64> {
            if !is_absolute_normalized(path) {
                return Err(invalid_mountinfo());
            }
            let mut selected: Option<&MountEntry> = None;
            for entry in &self.entries {
                if !component_prefix(&entry.mount_point, path) {
                    continue;
                }
                match selected {
                    None => selected = Some(entry),
                    Some(current) if entry.mount_point.len() > current.mount_point.len() => {
                        selected = Some(entry);
                    }
                    Some(current) if entry.mount_point.len() == current.mount_point.len() => {
                        return Err(invalid_mountinfo());
                    }
                    Some(_) => {}
                }
            }
            selected.map(|entry| entry.id).ok_or_else(invalid_mountinfo)
        }
    }

    pub(super) fn same_identity(
        left_device: u64,
        left_mount: u64,
        right_device: u64,
        right_mount: u64,
    ) -> bool {
        left_device == right_device && left_mount == right_mount
    }

    fn parse_decimal(field: &[u8]) -> io::Result<u64> {
        if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
            return Err(invalid_mountinfo());
        }
        let text = std::str::from_utf8(field).map_err(|_| invalid_mountinfo())?;
        let value = text.parse::<u64>().map_err(|_| invalid_mountinfo())?;
        if value == 0 {
            return Err(invalid_mountinfo());
        }
        Ok(value)
    }

    fn parse_device(field: &[u8]) -> io::Result<()> {
        let mut parts = field.split(|byte| *byte == b':');
        let major = parts.next().ok_or_else(invalid_mountinfo)?;
        let minor = parts.next().ok_or_else(invalid_mountinfo)?;
        if parts.next().is_some() {
            return Err(invalid_mountinfo());
        }
        parse_decimal_allow_zero(major)?;
        parse_decimal_allow_zero(minor)
    }

    fn parse_decimal_allow_zero(field: &[u8]) -> io::Result<()> {
        if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
            return Err(invalid_mountinfo());
        }
        let text = std::str::from_utf8(field).map_err(|_| invalid_mountinfo())?;
        text.parse::<u64>()
            .map(|_| ())
            .map_err(|_| invalid_mountinfo())
    }

    fn decode_path(field: &[u8]) -> io::Result<Vec<u8>> {
        let mut decoded = Vec::with_capacity(field.len());
        let mut index = 0;
        while index < field.len() {
            if field[index] != b'\\' {
                decoded.push(field[index]);
                index += 1;
                continue;
            }
            let end = index.checked_add(4).ok_or_else(invalid_mountinfo)?;
            let escaped = field.get(index..end).ok_or_else(invalid_mountinfo)?;
            let value = match escaped {
                b"\\040" => b' ',
                b"\\011" => b'\t',
                b"\\012" => b'\n',
                b"\\134" => b'\\',
                _ => return Err(invalid_mountinfo()),
            };
            decoded.push(value);
            index = end;
        }
        Ok(decoded)
    }

    fn is_absolute_normalized(path: &[u8]) -> bool {
        path.first() == Some(&b'/') && (path == b"/" || path.last() != Some(&b'/'))
    }

    fn component_prefix(prefix: &[u8], path: &[u8]) -> bool {
        if prefix == b"/" {
            return path.starts_with(b"/");
        }
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with(b"/"))
    }

    fn invalid_mountinfo() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "filesystem mount identity is unavailable or malformed",
        )
    }
}

#[cfg(not(any(
    windows,
    all(
        any(target_os = "linux", target_os = "macos"),
        target_pointer_width = "64"
    )
)))]
mod os {
    use super::*;

    pub(super) fn query_artifact_filesystem(
        _artifact_path: &Path,
        _target_paths: &[PathBuf],
    ) -> Result<FilesystemSnapshot, StorageAdmissionError> {
        Err(StorageAdmissionError::query(
            StorageQueryErrorKind::UnsupportedPlatform,
            None,
        ))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub(crate) enum TestStorageResponse {
        Available {
            available_bytes: u64,
            allocation_unit_bytes: u64,
        },
        QueryFailure,
        Unsupported,
        CrossFilesystem,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct StorageQueryRecord {
        pub(crate) artifact_path: PathBuf,
        pub(crate) target_paths: Vec<PathBuf>,
    }

    struct Override {
        response: TestStorageResponse,
        records: Vec<StorageQueryRecord>,
    }

    thread_local! {
        static OVERRIDE: RefCell<Option<Override>> = const { RefCell::new(None) };
    }

    pub(crate) fn with_storage_response<T>(
        response: TestStorageResponse,
        run: impl FnOnce() -> T,
    ) -> (T, Vec<StorageQueryRecord>) {
        struct Reset(Option<Override>);
        impl Drop for Reset {
            fn drop(&mut self) {
                OVERRIDE.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }

        let previous = OVERRIDE.with(|slot| {
            slot.borrow_mut().replace(Override {
                response,
                records: Vec::new(),
            })
        });
        let reset = Reset(previous);
        let value = run();
        let records = OVERRIDE.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|state| std::mem::take(&mut state.records))
                .unwrap_or_default()
        });
        drop(reset);
        (value, records)
    }

    pub(super) fn query_override(
        artifact_path: &Path,
        target_paths: &[PathBuf],
    ) -> Option<Result<FilesystemSnapshot, StorageAdmissionError>> {
        OVERRIDE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let state = slot.as_mut()?;
            state.records.push(StorageQueryRecord {
                artifact_path: artifact_path.to_path_buf(),
                target_paths: target_paths.to_vec(),
            });
            Some(match state.response {
                TestStorageResponse::Available {
                    available_bytes,
                    allocation_unit_bytes,
                } => Ok(FilesystemSnapshot {
                    caller_available_bytes: available_bytes,
                    allocation_unit_bytes,
                }),
                TestStorageResponse::QueryFailure => Err(StorageAdmissionError::query(
                    StorageQueryErrorKind::QueryFailed,
                    Some(io::Error::other("injected free-space query failure")),
                )),
                TestStorageResponse::Unsupported => Err(StorageAdmissionError::query(
                    StorageQueryErrorKind::UnsupportedPlatform,
                    None,
                )),
                TestStorageResponse::CrossFilesystem => Err(StorageAdmissionError::query(
                    StorageQueryErrorKind::CrossFilesystem,
                    None,
                )),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        staged: &'a [u64],
        backups: &'a [Option<u64>],
        names: &'a [u64],
    ) -> TransactionStorageInput<'a> {
        TransactionStorageInput {
            staged_sizes: staged,
            backup_sizes: backups,
            target_name_bytes: names,
            minimum_required_bytes: 0,
        }
    }

    #[test]
    fn exact_boundary_passes_and_one_byte_or_zero_available_fails() {
        let requirement = estimate_peak(4096, input(&[4097], &[Some(1)], &[12])).unwrap();
        let artifact = Path::new("project/.worldbuild/transactions");
        let targets = [PathBuf::from("project/data/a.json")];
        let (exact, records) = test_support::with_storage_response(
            test_support::TestStorageResponse::Available {
                available_bytes: requirement.required_peak_bytes,
                allocation_unit_bytes: 4096,
            },
            || admit_transaction_storage(artifact, &targets, input(&[4097], &[Some(1)], &[12])),
        );
        let exact = exact.unwrap();
        assert_eq!(exact.decision(), StorageAdmissionDecision::Admitted);
        assert_eq!(exact.available_bytes(), exact.required_peak_bytes());
        assert_eq!(records[0].artifact_path, artifact);

        for available in [requirement.required_peak_bytes - 1, 0] {
            let (result, _) = test_support::with_storage_response(
                test_support::TestStorageResponse::Available {
                    available_bytes: available,
                    allocation_unit_bytes: 4096,
                },
                || admit_transaction_storage(artifact, &targets, input(&[4097], &[Some(1)], &[12])),
            );
            let error = result.unwrap_err();
            let denied = error.admission().unwrap();
            assert_eq!(denied.decision(), StorageAdmissionDecision::Insufficient);
        }
    }

    #[test]
    fn allocation_rounding_and_minimum_estimate_are_conservative() {
        let one = estimate_peak(4096, input(&[1], &[Some(1)], &[1])).unwrap();
        let full = estimate_peak(4096, input(&[4096], &[Some(4096)], &[1])).unwrap();
        assert_eq!(one.required_peak_bytes, full.required_peak_bytes);

        let two_operations =
            estimate_peak(4096, input(&[1, 1], &[Some(1), Some(1)], &[1, 1])).unwrap();
        assert!(two_operations.required_peak_bytes > one.required_peak_bytes);

        let minimum = one.required_peak_bytes + 1;
        let raised = estimate_peak(
            4096,
            TransactionStorageInput {
                staged_sizes: &[1],
                backup_sizes: &[Some(1)],
                target_name_bytes: &[1],
                minimum_required_bytes: minimum,
            },
        )
        .unwrap();
        assert_eq!(raised.required_peak_bytes, minimum);
    }

    #[test]
    fn every_arithmetic_boundary_fails_closed() {
        for input in [
            TransactionStorageInput {
                staged_sizes: &[u64::MAX],
                backup_sizes: &[None],
                target_name_bytes: &[1],
                minimum_required_bytes: 0,
            },
            TransactionStorageInput {
                staged_sizes: &[1],
                backup_sizes: &[Some(u64::MAX)],
                target_name_bytes: &[1],
                minimum_required_bytes: 0,
            },
            TransactionStorageInput {
                staged_sizes: &[1],
                backup_sizes: &[None],
                target_name_bytes: &[u64::MAX],
                minimum_required_bytes: 0,
            },
        ] {
            assert!(matches!(
                estimate_peak(u64::MAX, input),
                Err(StorageAdmissionError::EstimateOverflow)
            ));
        }
        assert!(matches!(
            estimate_peak(0, input(&[1], &[None], &[1])),
            Err(StorageAdmissionError::EstimateOverflow)
        ));
    }

    #[test]
    fn linux_mount_identity_requires_both_device_and_mount_id() {
        assert!(linux_mountinfo::same_identity(42, 7, 42, 7));
        assert!(!linux_mountinfo::same_identity(42, 7, 42, 8));
        assert!(!linux_mountinfo::same_identity(42, 7, 43, 7));
    }

    #[test]
    fn mountinfo_uses_escaped_component_longest_mount_point() {
        let table = linux_mountinfo::MountTable::parse(
            b"24 1 8:1 / / rw,relatime - ext4 /dev/root rw\n\
              25 24 8:1 /project /project rw,relatime - ext4 /dev/root rw\n\
              26 25 8:1 /data /project/data rw,relatime - ext4 /dev/root rw\n\
              27 25 8:1 /space\\040root /project/with\\040space rw,relatime - ext4 /dev/root rw\n\
              28 25 8:1 /slash\\134root /project/slash\\134name rw,relatime - ext4 /dev/root rw\n",
        )
        .unwrap();

        assert_eq!(table.mount_id_for(b"/project/data/file.json").unwrap(), 26);
        assert_eq!(table.mount_id_for(b"/project/data2/file.json").unwrap(), 25);
        assert_eq!(
            table
                .mount_id_for(b"/project/with space/file.json")
                .unwrap(),
            27
        );
        assert_eq!(
            table
                .mount_id_for(b"/project/slash\\name/file.json")
                .unwrap(),
            28
        );
    }

    #[test]
    fn malformed_ambiguous_or_missing_mountinfo_fails_closed() {
        for malformed in [
            b"".as_slice(),
            b"24 1 8:1 / / rw ext4 /dev/root rw\n".as_slice(),
            b"24 1 8:1 / /bad\\041escape rw - ext4 /dev/root rw\n".as_slice(),
            b"24 1 8:1 / / rw - ext4 /dev/root rw\n24 1 8:1 /x /x rw - ext4 /dev/root rw\n"
                .as_slice(),
        ] {
            assert!(linux_mountinfo::MountTable::parse(malformed).is_err());
        }

        let missing = linux_mountinfo::MountTable::parse(
            b"25 24 8:1 /project /project rw - ext4 /dev/root rw\n",
        )
        .unwrap();
        assert!(missing.mount_id_for(b"/outside/file.json").is_err());

        let ambiguous = linux_mountinfo::MountTable::parse(
            b"24 1 8:1 / / rw - ext4 /dev/root rw\n\
              25 24 8:1 /one /project rw - ext4 /dev/root rw\n\
              26 24 8:1 /two /project rw - ext4 /dev/root rw\n",
        )
        .unwrap();
        assert!(ambiguous.mount_id_for(b"/project/file.json").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_real_query_populates_all_required_outputs_and_uses_caller_available() {
        let root = std::env::temp_dir();
        let snapshot = os::query_artifact_filesystem(&root, std::slice::from_ref(&root)).unwrap();
        assert!(snapshot.caller_available_bytes > 0);
        assert!(snapshot.allocation_unit_bytes > 0);
    }
}

#[cfg(test)]
#[path = "storage_owned_tests.rs"]
mod owned_tests;
