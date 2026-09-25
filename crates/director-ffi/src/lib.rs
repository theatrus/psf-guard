//! Caller-owned byte buffers: no allocator ownership crosses the native boundary.

#[cfg(panic = "abort")]
compile_error!("Build Director FFI with --profile director (panic=unwind), not --release");

use psf_guard_director_core::{evaluate_json, MAX_REQUEST_BYTES};

pub const OK: i32 = 0;
pub const INVALID_BUFFER: i32 = -1;
pub const BUFFER_TOO_SMALL: i32 = -2;
pub const INTERNAL_ERROR: i32 = -3;
pub const INPUT_TOO_LARGE: i32 = -4;

#[unsafe(no_mangle)]
pub extern "C" fn psfg_director_abi_version() -> u32 {
    1
}

/// Evaluate UTF-8 JSON; output is UTF-8 without a terminator. A zero-capacity
/// output queries the required length. BUFFER_TOO_SMALL writes no output bytes.
/// Contract/validation errors are JSON responses, not ABI failures.
///
/// # Safety
/// Non-null `input` with an in-limit length must point to `input_len` readable
/// bytes. Null input and oversized lengths are rejected before reading.
/// `output_len` must point to a
/// writable usize. For nonzero capacity, `output` must point to `output_capacity`
/// writable bytes. All ranges must be valid, nonoverlapping, and exclusively
/// available for this call. A null output is allowed only with zero capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn psfg_director_evaluate(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
) -> i32 {
    if output_len.is_null() {
        return INVALID_BUFFER;
    }
    unsafe {
        *output_len = 0;
    }
    if input.is_null() || (output.is_null() && output_capacity != 0) {
        return INVALID_BUFFER;
    }
    if input_len > MAX_REQUEST_BYTES {
        return INPUT_TOO_LARGE;
    }
    // No retained pointers or mutable engine state. In unwind builds contain
    // panics; production abort-profile panics still terminate the host process.
    let result = std::panic::catch_unwind(|| {
        let input = unsafe { std::slice::from_raw_parts(input, input_len) };
        serde_json::to_vec(&evaluate_json(input))
    });
    let Ok(Ok(bytes)) = result else {
        return INTERNAL_ERROR;
    };
    unsafe {
        *output_len = bytes.len();
    }
    if output_capacity < bytes.len() {
        return BUFFER_TOO_SMALL;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_negotiation_and_guard_bytes() {
        let input = b"{}";
        let mut len = 99;
        let mut buffer = [0xa5; 1024];
        unsafe {
            assert_eq!(
                psfg_director_evaluate(
                    input.as_ptr(),
                    input.len(),
                    std::ptr::null_mut(),
                    0,
                    &mut len
                ),
                BUFFER_TOO_SMALL
            );
            assert!(len > 0);
            assert_eq!(
                psfg_director_evaluate(
                    input.as_ptr(),
                    input.len(),
                    buffer.as_mut_ptr(),
                    len - 1,
                    &mut len
                ),
                BUFFER_TOO_SMALL
            );
            assert!(buffer.iter().all(|b| *b == 0xa5));
            assert_eq!(
                psfg_director_evaluate(
                    input.as_ptr(),
                    input.len(),
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut len
                ),
                OK
            );
        }
        let actual: serde_json::Value = serde_json::from_slice(&buffer[..len]).unwrap();
        assert_eq!(actual["status"], "error");
        assert!(buffer[len..].iter().all(|b| *b == 0xa5));
    }

    #[test]
    fn invalid_buffers_and_limits_fail_without_work() {
        let mut len = 123;
        unsafe {
            assert_eq!(
                psfg_director_evaluate(std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut len),
                INVALID_BUFFER
            );
            assert_eq!(len, 0);
            assert_eq!(
                psfg_director_evaluate(b"x".as_ptr(), 1, std::ptr::null_mut(), 1, &mut len),
                INVALID_BUFFER
            );
            assert_eq!(
                psfg_director_evaluate(
                    b"x".as_ptr(),
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut()
                ),
                INVALID_BUFFER
            );
            assert_eq!(
                psfg_director_evaluate(
                    b"x".as_ptr(),
                    MAX_REQUEST_BYTES + 1,
                    std::ptr::null_mut(),
                    0,
                    &mut len
                ),
                INPUT_TOO_LARGE
            );
        }
        assert_eq!(len, 0);
    }

    #[test]
    fn independent_callers_do_not_share_mutable_state() {
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..20 {
                        let input = b"{}";
                        let mut buffer = [0; 1024];
                        let mut len = 0;
                        let status = unsafe {
                            psfg_director_evaluate(
                                input.as_ptr(),
                                input.len(),
                                buffer.as_mut_ptr(),
                                buffer.len(),
                                &mut len,
                            )
                        };
                        assert_eq!(status, OK);
                        assert_eq!(
                            buffer[..len],
                            serde_json::to_vec(&evaluate_json(input)).unwrap()
                        );
                    }
                });
            }
        });
    }
}
