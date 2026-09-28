//! Windows 전원 복귀는 tao의 Poll/Resumed가 아니라 실제 WM_POWERBROADCAST에서 받는다.
use crate::{
    commands::dto::{Code, Reply},
    data::application::scheduler::TriggerReason,
    state::AppState,
};
use std::sync::Arc;

pub(crate) fn dispatch_power(state: &AppState, message: u32, event: usize) -> bool {
    if message == 0x0218 && event == 0x0012 {
        state.trigger(TriggerReason::OsResume);
        true
    } else {
        false
    }
}

#[cfg(windows)]
mod windows_hook {
    use super::*;
    use std::cell::RefCell;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::WM_NCDESTROY,
        },
    };
    const SUBCLASS: usize = 0x57424713;
    struct Hook {
        hwnd: HWND,
        owner: Arc<AppState>,
    }
    thread_local! { static HOOK:RefCell<Option<Hook>>=const { RefCell::new(None) }; }

    pub(crate) fn install(hwnd: HWND, state: Arc<AppState>) -> Reply<()> {
        HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err(Code::PlatformRegistration.into());
            }
            // Arc allocation 주소는 등록 수명 동안 고정된다. 제거 성공 뒤에만 owner를 해제한다.
            let owner = state;
            let data = Arc::as_ptr(&owner) as usize;
            // SAFETY: Tauri setup의 창 소유 thread에서 호출하며 callback ABI와 refData 타입이 일치한다.
            if !unsafe { SetWindowSubclass(hwnd, Some(callback), SUBCLASS, data) }.as_bool() {
                return Err(Code::PlatformRegistration.into());
            }
            *slot = Some(Hook { hwnd, owner });
            slot.as_ref()
                .ok_or(Code::PlatformRegistration)?
                .owner
                .native_registered();
            Ok(())
        })
    }
    pub(crate) fn remove() -> Reply<()> {
        HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(hook) = slot.as_ref() else {
                return Ok(());
            };
            #[cfg(test)]
            if REMOVE_FAULT.with(|fault| {
                let mut fault = fault.borrow_mut();
                fault.1 += 1;
                if fault.0 > 0 {
                    fault.0 -= 1;
                    true
                } else {
                    false
                }
            }) {
                hook.owner.event_failure(Code::PlatformRemoval);
                return Err(Code::PlatformRemoval.into());
            }
            // SAFETY: 등록한 같은 thread/창/proc/id이며 실패할 때 owner를 보존한다.
            if !unsafe { RemoveWindowSubclass(hook.hwnd, Some(callback), SUBCLASS) }.as_bool() {
                hook.owner.event_failure(Code::PlatformRemoval);
                return Err(Code::PlatformRemoval.into());
            }
            hook.owner.native_removed();
            slot.take();
            Ok(())
        })
    }
    #[cfg(test)]
    thread_local! { static REMOVE_FAULT: RefCell<(usize, usize)> = const { RefCell::new((0, 0)) }; }
    #[cfg(test)]
    pub(crate) fn fail_removals(count: usize) {
        REMOVE_FAULT.with(|fault| *fault.borrow_mut() = (count, 0));
    }
    #[cfg(test)]
    pub(crate) fn removal_attempts() -> usize {
        REMOVE_FAULT.with(|fault| fault.borrow().1)
    }
    unsafe extern "system" fn callback(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        let pointer = data as *const AppState;
        // SAFETY: 같은 창 thread의 hook이 strong owner를 보관한다. callback용 참조를 먼저
        // 증가시켜 remove/재진입 뒤에도 유지하고 from_raw가 그 한 참조만 회수하게 한다.
        unsafe {
            Arc::increment_strong_count(pointer);
        }
        let state = unsafe { Arc::from_raw(pointer) };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch_power(&state, message, wparam.0);
            if message == WM_NCDESTROY {
                state.revoke();
                if remove().is_err() {
                    state.event_failure(Code::PlatformRemoval);
                }
            }
        }))
        .is_err()
        {
            state.event_failure(Code::Unavailable);
        }
        // SAFETY: 가로채지 않은 메시지와 원 native 인수를 다음 subclass에 전달한다.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
}
#[cfg(all(test, windows))]
pub(crate) use windows_hook::{fail_removals, removal_attempts};
#[cfg(windows)]
pub(crate) use windows_hook::{install, remove};
#[cfg(windows)]
pub(crate) fn show_startup_failure() {
    use windows::{
        core::PCWSTR,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };
    // 아직 project를 수락하지 않은 startup 실패에도 GUI에서 안전한 다음 선택을 제공한다.
    let message: Vec<u16> = crate::native_strings::STARTUP_FAILURE
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let title: Vec<u16> = crate::native_strings::TITLE
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
#[cfg(not(windows))]
pub(crate) fn remove() -> Reply<()> {
    Ok(())
}
