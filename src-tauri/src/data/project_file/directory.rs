//! G3의 flat namespace를 검사하는 좁은 Windows directory guard다.
use std::fs::{self, File, ReadDir};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct LocationMismatch {
    case_only: bool,
    package_namespace: bool,
}
impl std::fmt::Display for LocationMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("opened artifact location changed or is aliased")
    }
}
impl std::error::Error for LocationMismatch {}
pub(crate) fn location_mismatch_reason(error: &io::Error) -> Option<&'static str> {
    error
        .get_ref()?
        .downcast_ref::<LocationMismatch>()
        .map(|e| {
            if e.package_namespace {
                "location_package_namespace_mismatch"
            } else if e.case_only {
                "location_case_mismatch"
            } else {
                "location_mismatch"
            }
        })
}

/// 경로 fingerprint와 달리 같은 경로에 다시 만든 디렉터리도 구별한다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirectoryIdentity {
    volume: u64,
    index: [u8; 16],
}

pub(crate) struct ProjectDirectory {
    file: File,
    path: PathBuf,
    identity: DirectoryIdentity,
}

impl ProjectDirectory {
    pub(crate) fn open_root(canonical_root: &Path) -> io::Result<Self> {
        Self::open(canonical_root, false, false)
    }

    /// 소유한 백업 저장소처럼 guard 아래 자식의 생성·삭제가 필요한 범위 전용이다.
    /// DELETE 공유는 주지 않아 guard 디렉터리 자체의 교체는 계속 차단한다.
    pub(crate) fn open_mutable_root(canonical_root: &Path) -> io::Result<Self> {
        Self::open(canonical_root, true, false)
    }

    /// handle-relative no-replace 이동의 목적지로 쓸 directory만 자식 추가 권한을 연다.
    pub(crate) fn open_move_destination(canonical_root: &Path) -> io::Result<Self> {
        let file = open_move_destination(canonical_root)?;
        validate_directory(&file, canonical_root)?;
        let identity = file_identity(&file)?;
        Ok(Self {
            file,
            path: canonical_root.to_owned(),
            identity,
        })
    }

    /// 앱이 소유한 정확한 디렉터리를 handle 기준으로 비재귀 삭제할 때만 사용한다.
    /// 자식 변경은 허용하지만 이 handle 자체의 DELETE 공유는 주지 않는다.
    pub(crate) fn open_owned_root(canonical_root: &Path) -> io::Result<Self> {
        Self::open(canonical_root, true, true)
    }

    fn open(canonical_root: &Path, mutable_children: bool, deletable: bool) -> io::Result<Self> {
        let file = open_directory(canonical_root, mutable_children, deletable)?;
        validate_directory(&file, canonical_root)?;
        let identity = file_identity(&file)?;
        Ok(Self {
            file,
            path: canonical_root.to_owned(),
            identity,
        })
    }

    pub(crate) fn same_volume(&self, file: &File) -> io::Result<bool> {
        Ok(file_identity(file)?.volume == self.identity.volume)
    }

    pub(crate) fn identity(&self) -> DirectoryIdentity {
        self.identity
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        validate_directory(&self.file, &self.path)
    }

    /// root guard를 보유한 채 마지막 component 자체를 검사한다. dangling link는 부재가 아니다.
    pub(crate) fn open_namespace(&self, name: &str) -> io::Result<Option<Self>> {
        self.validate()?;
        if !matches!(name, "templates" | "documents" | "workspace") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid artifact namespace",
            ));
        }
        let path = self.path.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => validate_directory_metadata(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.validate()?;
                // 부재 확인 중 새 entry가 관측되면 빈 namespace 증표를 만들지 않는다.
                return match fs::symlink_metadata(&path) {
                    Err(confirm) if confirm.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(confirm) => Err(confirm),
                    Ok(_) => Err(io::Error::other(
                        "artifact namespace changed during absence inspection",
                    )),
                };
            }
            Err(error) => return Err(error),
        }
        // 여기서의 NotFound는 이미 관측한 namespace 소실이다. None으로 바꾸지 않는다.
        let directory = Self::open_root(&path)?;
        self.validate()?;
        Ok(Some(directory))
    }

    pub(crate) fn read_dir(&self) -> io::Result<ReadDir> {
        self.validate()?;
        // Windows guard는 FILE_SHARE_DELETE를 주지 않는다. 따라서 이 path를 다른
        // directory로 교체한 뒤 열거하는 경쟁을 OS가 차단한다. 종료 때도 guard를 검사한다.
        fs::read_dir(&self.path)
    }

    pub(crate) fn validate_file(&self, file: &File, name: &str) -> io::Result<()> {
        self.validate()?;
        validate_file_location(file, &self.path.join(name))
    }

    /// Open a flat child against the held directory. The caller must bind the
    /// returned handle with `validate_file` before reading it.
    pub(crate) fn open_child_candidate(&self, name: &str) -> io::Result<File> {
        self.validate()?;
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid child name",
            ));
        }
        let path = self.path.join(name);
        #[cfg(windows)]
        {
            let file = super::open_windows_candidate(&path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || is_reparse(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsafe child file",
                ));
            }
            Ok(file)
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            unsupported()
        }
    }

    /// 검증한 동일 handle에만 삭제 disposition을 설정한다. 디렉터리가 비어 있지 않으면
    /// OS가 실패시키며 caller가 경로 기반 재귀 정리로 우회하지 않는다.
    pub(crate) fn delete_owned(self) -> io::Result<()> {
        #[cfg(windows)]
        {
            crate::data::atomic_file::owned::delete_owned(&self.file)
        }
        #[cfg(not(windows))]
        {
            let _ = self;
            unsupported()
        }
    }

    /// source와 destination을 모두 handle로 검증한 뒤 source 자체 handle을
    /// destination의 한 자식 이름으로 no-replace 이동한다.
    pub(crate) fn rename_owned_into(
        self,
        destination: &ProjectDirectory,
        name: &str,
    ) -> io::Result<()> {
        self.validate()?;
        destination.validate()?;
        if self.identity.volume != destination.identity.volume {
            return Err(io::Error::new(
                io::ErrorKind::CrossesDevices,
                "owned directory move crosses volumes",
            ));
        }
        #[cfg(windows)]
        {
            crate::data::atomic_file::owned::rename_path(
                &self.file,
                &destination.path.join(name),
                false,
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (self, destination, name);
            unsupported()
        }
    }
}

#[cfg(windows)]
fn open_directory(path: &Path, mutable_children: bool, deletable: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(if mutable_children {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        } else {
            FILE_SHARE_READ
        })
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    if deletable {
        // GENERIC_READ | DELETE. 실제 삭제는 검증한 이 handle에 disposition을 설정한다.
        options.access_mode(0x8001_0000);
    }
    options.open(path)
}

#[cfg(windows)]
fn open_move_destination(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    // GENERIC_READ | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .access_mode(0x8000_0006)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(windows)]
pub(crate) fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0000_0400 != 0
}

#[cfg(not(windows))]
pub(crate) fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn validate_directory_metadata(metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.is_dir() || is_reparse(metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "artifact namespace is not a non-reparse directory",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_directory(file: &File, expected: &Path) -> io::Result<()> {
    validate_directory_metadata(&file.metadata()?)?;
    validate_file_location(file, expected)
}

#[cfg(windows)]
fn validate_file_location(file: &File, expected: &Path) -> io::Result<()> {
    let actual = super::normalize_windows_verbatim_path(&super::windows_final_path(file)?);
    let expected = super::normalize_windows_verbatim_path(expected);
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            LocationMismatch {
                // 개인 경로를 내보내지 않고 패키지 실행 환경의 경로 차이만 구분한다.
                package_namespace: [actual.as_path(), expected.as_path()].iter().any(|path| {
                    path.components().any(|part| {
                        part.as_os_str()
                            .eq_ignore_ascii_case(std::ffi::OsStr::new("Packages"))
                    })
                }),
                case_only: actual
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy()),
            },
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn file_identity(file: &File) -> io::Result<DirectoryIdentity> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    // FILE_ID_INFO의 128-bit ID를 사용해 ReFS에서도 64-bit ID 충돌에 기대지 않는다.
    // 기존 handle 검증의 OS 경계만 보강하며 출력 공간은 호출 동안 유효하다.
    #[repr(C)]
    #[derive(Default)]
    struct Information {
        volume: u64,
        index: [u8; 16],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            class: i32,
            information: *mut Information,
            size: u32,
        ) -> i32;
    }
    let mut information = Information::default();
    // SAFETY: File이 소유한 유효 handle과 정확한 repr(C) 출력 공간을 동기 호출에만 빌린다.
    const FILE_ID_INFO: i32 = 18;
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_ID_INFO,
            &mut information,
            std::mem::size_of::<Information>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(DirectoryIdentity {
        volume: information.volume,
        index: information.index,
    })
}

// 기존 Linux/macOS 단일 file helper는 변경하지 않는다. 새 namespace guard를 안전하게
// 제공할 수 없는 플랫폼에서는 경로 exists 검사로 성공을 흉내내지 않고 명시적으로 거부한다.
#[cfg(not(windows))]
fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "guarded artifact directory enumeration is unavailable on this platform",
    ))
}
#[cfg(not(windows))]
fn open_directory(_: &Path, _: bool, _: bool) -> io::Result<File> {
    unsupported()
}
#[cfg(not(windows))]
fn open_move_destination(_: &Path) -> io::Result<File> {
    unsupported()
}
#[cfg(not(windows))]
fn validate_directory(_: &File, _: &Path) -> io::Result<()> {
    unsupported()
}
#[cfg(not(windows))]
fn validate_file_location(_: &File, _: &Path) -> io::Result<()> {
    unsupported()
}
#[cfg(not(windows))]
pub(crate) fn file_identity(_: &File) -> io::Result<DirectoryIdentity> {
    unsupported()
}
