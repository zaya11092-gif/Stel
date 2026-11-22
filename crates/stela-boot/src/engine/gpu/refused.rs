//! The first DRM calls Linux refuses a Mesa driver, one line each: the ioctl
//! by name with what it asked for, or the mapping, and the errno. A Vulkan
//! driver turns most such refusals into one generic result
//! (`VK_ERROR_OUT_OF_DEVICE_MEMORY`), so these lines say which call it was.
//! Interrupted calls, which the C library repeats, are not refusals.
use crate::output;
use core::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

/// How many refusals are shown; one more line says that more were not.
const SHOWN: u32 = 96;
static REFUSED: AtomicU32 = AtomicU32::new(0);

/// DRM's ioctl type, `'d'`, and its first driver command.
const DRM: u32 = b'd' as u32;
const COMMAND_BASE: u32 = 0x40;
/// What [`Input`] keeps of an argument: every amdgpu input it decodes.
const KEPT: usize = 64;

const EINTR: i64 = -4;
const EAGAIN: i64 = -11;
/// Linux's `ERESTARTSYS`, which a client may see from an interrupted wait.
const ERESTARTSYS: i64 = -512;
/// Linux's `ETIME`: a bounded wait that ran out, as the completion
/// waiter's idle waits all do; never a refusal.
const ETIME: i64 = -62;

/// An ioctl argument's first bytes as the caller passed them: the kernel
/// writes its answer over the same structure. Only amdgpu's are decoded.
#[cfg_attr(not(feature = "radv"), allow(dead_code))]
pub(super) struct Input {
    bytes: [u8; KEPT],
}

#[cfg_attr(not(feature = "radv"), allow(dead_code))]
impl Input {
    fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("four bytes"))
    }

    fn u64(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.bytes[at..at + 8].try_into().expect("eight bytes"))
    }
}

/// Whether refusals are still shown, so a call's input is worth keeping.
fn showing() -> bool {
    REFUSED.load(Ordering::Relaxed) <= SHOWN
}

/// `argument`'s input before ioctl `request` runs, while refusals are shown.
///
/// # Safety
/// `argument` is what `request` says it is: its size in bytes, as the kernel
/// reads or writes it.
pub(super) unsafe fn before(request: u32, argument: *const c_void) -> Option<Input> {
    if !showing() || argument.is_null() {
        return None;
    }
    let size = ((request >> 16) & 0x3fff) as usize;
    let mut bytes = [0; KEPT];
    // SAFETY: the caller's: the argument holds the request's size.
    unsafe {
        ptr::copy_nonoverlapping(argument.cast::<u8>(), bytes.as_mut_ptr(), size.min(KEPT));
    }
    Some(Input { bytes })
}

/// Takes a refusal's turn: whether to show it, after saying once that the
/// rest are not.
fn turn() -> bool {
    let index = REFUSED.fetch_add(1, Ordering::Relaxed);
    if index == SHOWN {
        output::arguments(format_args!(
            "GPU DRM refused more calls; the first {SHOWN} are above\n"
        ));
    }
    index < SHOWN
}

/// Reports ioctl `request` on `fd` if Linux refused it with `result`.
pub(super) fn control(fd: c_int, request: u32, input: Option<&Input>, result: i64) {
    if result >= 0 || matches!(result, EINTR | EAGAIN | ERESTARTSYS) {
        return;
    }
    if result == ETIME && request & 0xff == 0xca && (request >> 8) & 0xff == DRM {
        return;
    }
    if !turn() {
        return;
    }
    let number = request & 0xff;
    let drm = (request >> 8) & 0xff == DRM;
    let name = if drm { name(number) } else { None };
    let mut detail = alloc::string::String::new();
    if let (true, Some(input)) = (drm, input) {
        describe(number, input, &mut detail);
    }
    match name {
        Some(name) => output::arguments(format_args!(
            "GPU DRM ioctl {name} ({request:#010x}) fd={fd}{detail} refused: {}\n",
            Errno(result)
        )),
        None => output::arguments(format_args!(
            "GPU DRM ioctl {request:#010x} fd={fd} refused: {}\n",
            Errno(result)
        )),
    }
}

/// Whether a call's `result` is an error: `-4095..=-1`, as Linux's
/// `IS_ERR_VALUE`. A mapping's address in the upper half of the address
/// space reads as a negative number and is none.
pub(super) fn is_error(result: i64) -> bool {
    (-4095..=-1).contains(&result)
}

/// Reports a mapping of `size` bytes of `fd` at `offset` Linux refused with
/// `result`; a mapped address is not reported.
pub(super) fn map(fd: c_int, offset: u64, size: usize, result: i64) {
    if is_error(result) && turn() {
        output::arguments(format_args!(
            "GPU DRM mmap fd={fd} offset={offset:#x} bytes={size:#x} refused: {}\n",
            Errno(result)
        ));
    }
}

/// A negative errno, with its name when it is a common one.
struct Errno(i64);

impl core::fmt::Display for Errno {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match -self.0 {
            1 => "EPERM",
            2 => "ENOENT",
            5 => "EIO",
            9 => "EBADF",
            12 => "ENOMEM",
            13 => "EACCES",
            14 => "EFAULT",
            16 => "EBUSY",
            19 => "ENODEV",
            22 => "EINVAL",
            28 => "ENOSPC",
            34 => "ERANGE",
            38 => "ENOSYS",
            62 => "ETIME",
            95 => "EOPNOTSUPP",
            110 => "ETIMEDOUT",
            125 => "ECANCELED",
            _ => "",
        };
        write!(formatter, "{}", self.0)?;
        if !name.is_empty() {
            write!(formatter, " ({name})")?;
        }
        Ok(())
    }
}

/// DRM's own ioctls and, past [`COMMAND_BASE`], the driver's, by number.
fn name(number: u32) -> Option<&'static str> {
    Some(match number {
        0x00 => "VERSION",
        0x09 => "GEM_CLOSE",
        0x0a => "GEM_FLINK",
        0x0b => "GEM_OPEN",
        0x0c => "GET_CAP",
        0x0d => "SET_CLIENT_CAP",
        0x2d => "PRIME_HANDLE_TO_FD",
        0x2e => "PRIME_FD_TO_HANDLE",
        0xbf => "SYNCOBJ_CREATE",
        0xc0 => "SYNCOBJ_DESTROY",
        0xc1 => "SYNCOBJ_HANDLE_TO_FD",
        0xc2 => "SYNCOBJ_FD_TO_HANDLE",
        0xc3 => "SYNCOBJ_WAIT",
        0xc4 => "SYNCOBJ_RESET",
        0xc5 => "SYNCOBJ_SIGNAL",
        0xca => "SYNCOBJ_TIMELINE_WAIT",
        0xcb => "SYNCOBJ_QUERY",
        0xcc => "SYNCOBJ_TRANSFER",
        0xcd => "SYNCOBJ_TIMELINE_SIGNAL",
        0xcf => "SYNCOBJ_EVENTFD",
        0xd1 => "SET_CLIENT_NAME",
        command @ COMMAND_BASE..0xa0 => return driver(command - COMMAND_BASE),
        _ => return None,
    })
}

#[cfg(feature = "radv")]
fn driver(command: u32) -> Option<&'static str> {
    Some(match command {
        0x00 => "AMDGPU_GEM_CREATE",
        0x01 => "AMDGPU_GEM_MMAP",
        0x02 => "AMDGPU_CTX",
        0x03 => "AMDGPU_BO_LIST",
        0x04 => "AMDGPU_CS",
        0x05 => "AMDGPU_INFO",
        0x06 => "AMDGPU_GEM_METADATA",
        0x07 => "AMDGPU_GEM_WAIT_IDLE",
        0x08 => "AMDGPU_GEM_VA",
        0x09 => "AMDGPU_WAIT_CS",
        0x10 => "AMDGPU_GEM_OP",
        0x11 => "AMDGPU_GEM_USERPTR",
        0x12 => "AMDGPU_WAIT_FENCES",
        0x13 => "AMDGPU_VM",
        0x14 => "AMDGPU_FENCE_TO_HANDLE",
        0x15 => "AMDGPU_SCHED",
        _ => return None,
    })
}

#[cfg(not(feature = "radv"))]
fn driver(command: u32) -> Option<&'static str> {
    // xe's commands (`uapi/drm/xe_drm.h`'s `DRM_IOCTL_XE_*`), named when the
    // environment's xe refuses one the image's ANV asks.
    #[cfg(feature = "anv")]
    let xe = match command {
        0x00 => Some("XE_DEVICE_QUERY"),
        _ => None,
    };
    #[cfg(not(feature = "anv"))]
    let xe = None;
    xe
}

/// What the call asked for, for the driver commands whose refusal a Vulkan
/// driver reports as out of memory, written after a space each.
#[cfg(feature = "radv")]
fn describe(number: u32, input: &Input, out: &mut alloc::string::String) {
    use core::fmt::Write;
    let Some(command) = number.checked_sub(COMMAND_BASE) else {
        return;
    };
    // `struct drm_amdgpu_*` of `uapi/drm/amdgpu_drm.h`, at their offsets.
    let _ = match command {
        // gem_create_in: bo_size, alignment, domains, domain_flags.
        0x00 => write!(
            out,
            " size={:#x} alignment={:#x} domains={:#x} flags={:#x}",
            input.u64(0),
            input.u64(8),
            input.u64(16),
            input.u64(24)
        ),
        // gem_mmap_in: handle.
        0x01 => write!(out, " handle={}", input.u32(0)),
        // ctx_in: op, flags, ctx_id, priority.
        0x02 => write!(
            out,
            " op={} flags={:#x} ctx={} priority={}",
            input.u32(0),
            input.u32(4),
            input.u32(8),
            input.u32(12) as i32
        ),
        // info: return_pointer, return_size, query, then the query's own.
        0x05 => write!(
            out,
            " query={:#x} size={} arguments={:#x},{:#x}",
            input.u32(12),
            input.u32(8),
            input.u32(16),
            input.u32(20)
        ),
        // gem_va: handle, _pad, operation, flags, va_address, offset_in_bo,
        // map_size.
        0x08 => write!(
            out,
            " handle={} op={} flags={:#x} va={:#x} offset={:#x} size={:#x}",
            input.u32(0),
            input.u32(8),
            input.u32(12),
            input.u64(16),
            input.u64(24),
            input.u64(32)
        ),
        // gem_op: handle, op, value.
        0x10 => write!(
            out,
            " handle={} op={} value={:#x}",
            input.u32(0),
            input.u32(4),
            input.u64(8)
        ),
        // vm_in: op, flags.
        0x13 => write!(out, " op={} flags={:#x}", input.u32(0), input.u32(4)),
        _ => Ok(()),
    };
}

/// The xe command's fields, which ANV's refusals report.
#[cfg(all(not(feature = "radv"), feature = "anv"))]
fn describe(number: u32, input: &Input, out: &mut alloc::string::String) {
    use core::fmt::Write;
    if number == COMMAND_BASE {
        // `struct drm_xe_device_query`: extensions, then the u32 query id.
        write!(out, " query={}", input.u32(8)).ok();
    }
}

#[cfg(not(any(feature = "radv", feature = "anv")))]
fn describe(_: u32, _: &Input, _: &mut alloc::string::String) {}
