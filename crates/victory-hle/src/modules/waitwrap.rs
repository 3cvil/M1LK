#![allow(non_snake_case, clippy::not_unsafe_ptr_arg_deref)]
//! (M1LK Wait Handle) The title's native waits, timed on the thread that presents.
//!
//! Installed only with `VICTORY_WAIT_HANDLE=1`: the title's imports of the wait calls and of
//! the calls that make events resolve here, and each one forwards to the host's own with the
//! same arguments. On the thread that presents (`victory_gpu::waitlog`) the wait is timed
//! and recorded with the handle and a few return addresses; on any other thread the
//! wrapper adds one boolean test to the call.

use std::ffi::c_void;
use std::time::Instant;

use victory_gpu::waitlog::{self, Kind};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;

use crate::{HleExport, HleModule, hle_export};

/// The two return addresses above the wrapper's caller, and the caller's.
fn callers() -> [usize; 3] {
    let mut frames = [std::ptr::null_mut(); 3];
    // SAFETY: fills at most three return addresses.
    unsafe { RtlCaptureStackBackTrace(2, 3, frames.as_mut_ptr(), std::ptr::null_mut()) };
    [frames[0] as usize, frames[1] as usize, frames[2] as usize]
}

/// Runs `call`, timing it if this is the presenting thread.
#[inline]
fn timed<R>(kind: Kind, handle: usize, call: impl FnOnce() -> R) -> R {
    if !waitlog::on_critical_thread() {
        return call();
    }
    let at = callers();
    let began = Instant::now();
    let result = call();
    waitlog::record(kind, handle, began.elapsed().as_nanos() as u64, at);
    result
}

pub extern "system" fn WaitForSingleObject(handle: HANDLE, ms: u32) -> u32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Single, handle as usize, || unsafe {
        windows_sys::Win32::System::Threading::WaitForSingleObject(handle, ms)
    })
}

pub extern "system" fn WaitForSingleObjectEx(handle: HANDLE, ms: u32, alertable: i32) -> u32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Single, handle as usize, || unsafe {
        windows_sys::Win32::System::Threading::WaitForSingleObjectEx(handle, ms, alertable)
    })
}

fn first(count: u32, handles: *const HANDLE) -> usize {
    if count == 0 || handles.is_null() {
        return 0;
    }
    // SAFETY: the title's own array of `count` handles.
    unsafe { *handles as usize }
}

pub extern "system" fn WaitForMultipleObjects(count: u32, handles: *const HANDLE, all: i32, ms: u32) -> u32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Multiple, first(count, handles), || unsafe {
        windows_sys::Win32::System::Threading::WaitForMultipleObjects(count, handles, all, ms)
    })
}

pub extern "system" fn WaitForMultipleObjectsEx(
    count: u32,
    handles: *const HANDLE,
    all: i32,
    ms: u32,
    alertable: i32,
) -> u32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Multiple, first(count, handles), || unsafe {
        windows_sys::Win32::System::Threading::WaitForMultipleObjectsEx(count, handles, all, ms, alertable)
    })
}

pub extern "system" fn Sleep(ms: u32) {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Sleep, 0, || unsafe { windows_sys::Win32::System::Threading::Sleep(ms) })
}

pub extern "system" fn SleepEx(ms: u32, alertable: i32) -> u32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Sleep, 0, || unsafe { windows_sys::Win32::System::Threading::SleepEx(ms, alertable) })
}

pub extern "system" fn WaitOnAddress(address: *const c_void, compare: *const c_void, size: usize, ms: u32) -> i32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Address, address as usize, || unsafe {
        windows_sys::Win32::System::Threading::WaitOnAddress(address, compare, size, ms)
    })
}

pub extern "system" fn SleepConditionVariableSRW(
    condition: *mut c_void,
    lock: *mut c_void,
    ms: u32,
    flags: u32,
) -> i32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Condition, condition as usize, || unsafe {
        windows_sys::Win32::System::Threading::SleepConditionVariableSRW(condition.cast(), lock.cast(), ms, flags)
    })
}

pub extern "system" fn SleepConditionVariableCS(condition: *mut c_void, section: *mut c_void, ms: u32) -> i32 {
    // SAFETY: forwarding the title's own arguments.
    timed(Kind::Condition, condition as usize, || unsafe {
        windows_sys::Win32::System::Threading::SleepConditionVariableCS(condition.cast(), section.cast(), ms)
    })
}

pub extern "system" fn CreateEventW(attributes: *const c_void, manual: i32, initial: i32, name: *const u16) -> HANDLE {
    // SAFETY: forwarding the title's own arguments.
    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(attributes.cast(), manual, initial, name)
    };
    waitlog::note_title(handle as usize);
    handle
}

pub extern "system" fn CreateEventExW(attributes: *const c_void, name: *const u16, flags: u32, access: u32) -> HANDLE {
    // SAFETY: forwarding the title's own arguments.
    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateEventExW(attributes.cast(), name, flags, access)
    };
    waitlog::note_title(handle as usize);
    handle
}

pub extern "system" fn CreateSemaphoreW(attributes: *const c_void, initial: i32, maximum: i32, name: *const u16) -> HANDLE {
    // SAFETY: forwarding the title's own arguments.
    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateSemaphoreW(attributes.cast(), initial, maximum, name)
    };
    waitlog::note_title(handle as usize);
    handle
}

pub extern "system" fn ReleaseSemaphore(semaphore: HANDLE, count: i32, previous: *mut i32) -> i32 {
    waitlog::signal(semaphore as usize);
    // SAFETY: forwarding the title's own arguments.
    unsafe { windows_sys::Win32::System::Threading::ReleaseSemaphore(semaphore, count, previous) }
}

pub extern "system" fn SetEvent(event: HANDLE) -> i32 {
    waitlog::signal(event as usize);
    // SAFETY: forwarding the title's own argument.
    unsafe { windows_sys::Win32::System::Threading::SetEvent(event) }
}

/// The wrappers, if the switch is on (none otherwise: the imports go to the host as before).
pub(crate) fn module() -> HleModule {
    let exports: Vec<HleExport> = if waitlog::enabled() {
        waitlog::set_locator(victory_loader::locate);
        tracing::info!("M1LK Wait Handle: native waits of the presenting thread are logged");
        vec![
            hle_export!(WaitForSingleObject),
            hle_export!(WaitForSingleObjectEx),
            hle_export!(WaitForMultipleObjects),
            hle_export!(WaitForMultipleObjectsEx),
            hle_export!(Sleep),
            hle_export!(SleepEx),
            hle_export!(WaitOnAddress),
            hle_export!(SleepConditionVariableSRW),
            hle_export!(SleepConditionVariableCS),
            hle_export!(CreateEventW),
            hle_export!(CreateEventExW),
            hle_export!(CreateSemaphoreW),
            hle_export!(ReleaseSemaphore),
            hle_export!(SetEvent),
        ]
    } else {
        Vec::new()
    };
    HleModule {
        name: "waitwrap",
        dlls: &[
            "kernel32.dll",
            "kernelbase.dll",
            "api-ms-win-core-synch-*",
            "api-ms-win-core-handle-*",
            "api-ms-win-core-realtime-*",
        ],
        exports,
    }
}
