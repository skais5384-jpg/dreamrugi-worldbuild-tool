//! 소유한 동일 handle만 게시/정리하는 공용 native primitive. 프로젝트 permit과는 별개다.
use std::{ffi::c_void, fs::File, io, os::windows::io::AsRawHandle, path::Path};
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
#[repr(C)]
union IoUnion {
    status: i32,
    pointer: *mut c_void,
}
#[repr(C)]
struct IoStatus {
    status: IoUnion,
    information: usize,
}
#[repr(C)]
union RenameFlags {
    replace: u8,
    flags: u32,
}
#[repr(C)]
struct RenameInfo {
    flags: RenameFlags,
    root: *mut c_void,
    len: u32,
    name: [u16; 1],
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetInformationFile(
        h: *mut c_void,
        io: *mut IoStatus,
        b: *const c_void,
        n: u32,
        class: i32,
    ) -> i32;
    fn RtlNtStatusToDosError(s: i32) -> u32;
}
fn set(f: &File, b: *const c_void, n: u32, class: i32) -> io::Result<()> {
    let mut ios = IoStatus {
        status: IoUnion { status: -1 },
        information: 0,
    };
    let s = unsafe { NtSetInformationFile(f.as_raw_handle(), &mut ios, b, n, class) };
    // OVERLAPPED를 주지 않은 동기 File만 받는다. PENDING은 완료로 보고하지 않는다.
    if s == 0 {
        Ok(())
    } else if s < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(s) } as i32,
        ))
    } else {
        Err(io::Error::other(
            "owned temporary: native request did not complete synchronously",
        ))
    }
}
pub(crate) fn disposition(f: &File, flags: u32) -> io::Result<()> {
    set(f, (&flags as *const u32).cast(), 4, 64)
}
pub(crate) fn delete_owned(f: &File) -> io::Result<()> {
    let delete: u8 = 1;
    set(f, (&delete as *const u8).cast(), 1, 13)
}
pub(crate) fn rename(f: &File, name: &str, replace: bool) -> io::Result<()> {
    rename_with_root(f, std::ptr::null_mut(), name, replace)
}

fn rename_with_root(f: &File, root: *mut c_void, name: &str, replace: bool) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':', '\0']) {
        return Err(invalid("owned temporary: invalid basename"));
    }
    let w = name.encode_utf16().collect::<Vec<_>>();
    let n = std::mem::size_of::<RenameInfo>() + w.len() * 2 + 2;
    let mut b = vec![0u64; n.div_ceil(8)];
    let p = b.as_mut_ptr().cast::<RenameInfo>();
    // repr(C) 정렬과 고정 구조체·UTF-16 이름·종료 문자까지 buffer 용량을 확보한다.
    unsafe {
        (*p).flags.replace = u8::from(replace);
        (*p).root = root;
        (*p).len = (w.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            w.as_ptr(),
            b.as_mut_ptr()
                .cast::<u8>()
                .add(std::mem::offset_of!(RenameInfo, name))
                .cast::<u16>(),
            w.len(),
        );
    }
    set(f, p.cast(), n as u32, 10)
}

/// source path를 다시 열지 않고 같은 source handle에 Win32 rename 정보를 적용한다.
/// destination의 모든 부모는 caller가 directory handle로 붙잡아 검증한다.
pub(crate) fn rename_path(f: &File, destination: &Path, replace: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetFileInformationByHandle(
            file: *mut c_void,
            class: i32,
            information: *const c_void,
            size: u32,
        ) -> i32;
    }
    if !destination.is_absolute() || destination.as_os_str().is_empty() {
        return Err(invalid("owned rename: invalid destination"));
    }
    let w = destination.as_os_str().encode_wide().collect::<Vec<_>>();
    if w.iter().any(|unit| *unit == 0) {
        return Err(invalid("owned rename: invalid destination"));
    }
    let n = std::mem::size_of::<RenameInfo>() + w.len() * 2 + 2;
    let mut b = vec![0u64; n.div_ceil(8)];
    let p = b.as_mut_ptr().cast::<RenameInfo>();
    unsafe {
        (*p).flags.replace = u8::from(replace);
        (*p).root = std::ptr::null_mut();
        (*p).len = (w.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            w.as_ptr(),
            b.as_mut_ptr()
                .cast::<u8>()
                .add(std::mem::offset_of!(RenameInfo, name))
                .cast::<u16>(),
            w.len(),
        );
    }
    // FileRenameInfo = 3. 반환 전 동기 완료되며 replace=false는 점유 대상을 보존한다.
    if unsafe { SetFileInformationByHandle(f.as_raw_handle(), 3, p.cast(), n as u32) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
