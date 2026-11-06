// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! sherpa-onnx's prebuilt libraries (speech to text) were built with a newer Visual C++ than older Build Tools
//! have: its standard library calls these helpers (stl/src/vector_algorithms.cpp in microsoft/STL), which older
//! toolsets' libcpmt.lib lack. Plain versions of the same, so Isle links with any Build Tools from VS 2022. Each
//! does what Microsoft's does, without its SIMD; a toolset that has them would make these duplicates, so they are
//! only built when asked (the `old-msvc` cfg, set by build.rs when the linker's library lacks them).
//!
//! On x64, `__stdcall` is the C convention.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

/// Elements of `T` from `first` to `last`.
unsafe fn slice<'a, T>(first: *const c_void, last: *const c_void) -> &'a [T] {
    let len = (last as usize - first as usize) / size_of::<T>();
    if len == 0 {
        return &[];
    }
    // SAFETY: the caller's range, as the STL hands it.
    unsafe { core::slice::from_raw_parts(first as *const T, len) }
}

unsafe fn search<T: PartialEq>(first1: *const c_void, last1: *const c_void, first2: *const c_void, count2: usize) -> *const c_void {
    if count2 == 0 {
        return first1;
    }
    // SAFETY: the caller's ranges.
    let (hay, needle) = unsafe { (slice::<T>(first1, last1), core::slice::from_raw_parts(first2 as *const T, count2)) };
    match hay.windows(count2).position(|window| window == needle) {
        Some(at) => (first1 as usize + at * size_of::<T>()) as *const c_void,
        None => last1,
    }
}

unsafe fn find_end<T: PartialEq>(first1: *const c_void, last1: *const c_void, first2: *const c_void, count2: usize) -> *const c_void {
    if count2 == 0 {
        return last1;
    }
    // SAFETY: the caller's ranges.
    let (hay, needle) = unsafe { (slice::<T>(first1, last1), core::slice::from_raw_parts(first2 as *const T, count2)) };
    match hay.windows(count2).rposition(|window| window == needle) {
        Some(at) => (first1 as usize + at * size_of::<T>()) as *const c_void,
        None => last1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_search_1(first1: *const c_void, last1: *const c_void, first2: *const c_void, count2: usize) -> *const c_void {
    unsafe { search::<u8>(first1, last1, first2, count2) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_search_2(first1: *const c_void, last1: *const c_void, first2: *const c_void, count2: usize) -> *const c_void {
    unsafe { search::<u16>(first1, last1, first2, count2) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_end_1(first1: *const c_void, last1: *const c_void, first2: *const c_void, count2: usize) -> *const c_void {
    unsafe { find_end::<u8>(first1, last1, first2, count2) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_end_2(first1: *const c_void, last1: *const c_void, first2: *const c_void, count2: usize) -> *const c_void {
    unsafe { find_end::<u16>(first1, last1, first2, count2) }
}

unsafe fn of<T: PartialEq + Copy>(haystack: *const c_void, haystack_length: usize, needle: *const c_void, needle_length: usize, last: bool) -> usize {
    if haystack_length == 0 || needle_length == 0 {
        return usize::MAX;
    }
    // SAFETY: the caller's strings.
    let (hay, needle) = unsafe {
        (core::slice::from_raw_parts(haystack as *const T, haystack_length), core::slice::from_raw_parts(needle as *const T, needle_length))
    };
    let found = if last { hay.iter().rposition(|c| needle.contains(c)) } else { hay.iter().position(|c| needle.contains(c)) };
    found.unwrap_or(usize::MAX)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_first_of_trivial_pos_1(haystack: *const c_void, haystack_length: usize, needle: *const c_void, needle_length: usize) -> usize {
    unsafe { of::<u8>(haystack, haystack_length, needle, needle_length, false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_first_of_trivial_pos_2(haystack: *const c_void, haystack_length: usize, needle: *const c_void, needle_length: usize) -> usize {
    unsafe { of::<u16>(haystack, haystack_length, needle, needle_length, false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_last_of_trivial_pos_1(haystack: *const c_void, haystack_length: usize, needle: *const c_void, needle_length: usize) -> usize {
    unsafe { of::<u8>(haystack, haystack_length, needle, needle_length, true) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_last_of_trivial_pos_2(haystack: *const c_void, haystack_length: usize, needle: *const c_void, needle_length: usize) -> usize {
    unsafe { of::<u16>(haystack, haystack_length, needle, needle_length, true) }
}

/// The largest value of a non-empty range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_max_4i(first: *const c_void, last: *const c_void) -> i32 {
    unsafe { slice::<i32>(first, last) }.iter().copied().max().unwrap_or(i32::MIN)
}

/// The first smallest (`min`) or first largest element, `last` for an empty range.
unsafe fn extreme<T: PartialOrd>(first: *const c_void, last: *const c_void, min: bool) -> *const c_void {
    let items = unsafe { slice::<T>(first, last) };
    if items.is_empty() {
        return last;
    }
    let mut best = 0;
    for (index, item) in items.iter().enumerate().skip(1) {
        let better = if min { *item < items[best] } else { items[best] < *item };
        if better {
            best = index;
        }
    }
    (first as usize + best * size_of::<T>()) as *const c_void
}

// The bool the old ABI passes after them is unused (and harmless to leave out on x64).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_min_element_f(first: *const c_void, last: *const c_void) -> *const c_void {
    unsafe { extreme::<f32>(first, last, true) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_min_element_d(first: *const c_void, last: *const c_void) -> *const c_void {
    unsafe { extreme::<f64>(first, last, true) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_max_element_f(first: *const c_void, last: *const c_void) -> *const c_void {
    unsafe { extreme::<f32>(first, last, false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_max_element_d(first: *const c_void, last: *const c_void) -> *const c_void {
    unsafe { extreme::<f64>(first, last, false) }
}

/// The index of the first element that differs, `count` when none does.
unsafe fn mismatch<T: PartialEq>(first1: *const c_void, first2: *const c_void, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let (a, b) = unsafe { (core::slice::from_raw_parts(first1 as *const T, count), core::slice::from_raw_parts(first2 as *const T, count)) };
    a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(count)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_mismatch_1(first1: *const c_void, first2: *const c_void, count: usize) -> usize {
    unsafe { mismatch::<u8>(first1, first2, count) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_mismatch_8(first1: *const c_void, first2: *const c_void, count: usize) -> usize {
    unsafe { mismatch::<u64>(first1, first2, count) }
}

/// std::remove: keeps the elements unlike `value` in order at the front; the new end.
unsafe fn remove<T: PartialEq + Copy>(first: *mut c_void, last: *mut c_void, value: T) -> *mut c_void {
    let len = (last as usize - first as usize) / size_of::<T>();
    if len == 0 {
        return first;
    }
    let items = unsafe { core::slice::from_raw_parts_mut(first as *mut T, len) };
    let mut out = 0;
    for index in 0..len {
        if items[index] != value {
            items[out] = items[index];
            out += 1;
        }
    }
    (first as usize + out * size_of::<T>()) as *mut c_void
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_remove_1(first: *mut c_void, last: *mut c_void, value: u8) -> *mut c_void {
    unsafe { remove::<u8>(first, last, value) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_remove_2(first: *mut c_void, last: *mut c_void, value: u16) -> *mut c_void {
    unsafe { remove::<u16>(first, last, value) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_remove_8(first: *mut c_void, last: *mut c_void, value: u64) -> *mut c_void {
    unsafe { remove::<u64>(first, last, value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn they_do_what_the_stl_does() {
        let hay = b"abcabcab";
        let p = |v: &[u8]| v.as_ptr() as *const c_void;
        let end = unsafe { p(hay).byte_add(hay.len()) };
        unsafe {
            assert_eq!(__std_search_1(p(hay), end, p(b"cab"), 3) as usize - hay.as_ptr() as usize, 2);
            assert_eq!(__std_find_end_1(p(hay), end, p(b"cab"), 3) as usize - hay.as_ptr() as usize, 5);
            assert_eq!(__std_search_1(p(hay), end, p(b"zz"), 2), end);
            assert_eq!(__std_find_end_1(p(hay), end, p(b""), 0), end);
            assert_eq!(__std_find_first_of_trivial_pos_1(p(hay), 8, p(b"cx"), 2), 2);
            assert_eq!(__std_find_last_of_trivial_pos_1(p(hay), 8, p(b"cx"), 2), 5);
            assert_eq!(__std_find_first_of_trivial_pos_1(p(hay), 8, p(b"x"), 1), usize::MAX);
            assert_eq!(__std_mismatch_1(p(b"abcd"), p(b"abzd"), 4), 2);
            let mut values = [1u8, 2, 1, 3, 1];
            let start = values.as_mut_ptr() as *mut c_void;
            let new_end = __std_remove_1(start, start.byte_add(5), 1);
            assert_eq!(new_end as usize - start as usize, 2);
            assert_eq!(&values[..2], &[2, 3]);
            let floats = [3.0f32, 1.0, 5.0, 1.0, 5.0];
            let (f, fe) = (floats.as_ptr() as *const c_void, floats.as_ptr().add(5) as *const c_void);
            assert_eq!((__std_min_element_f(f, fe) as usize - f as usize) / 4, 1);
            assert_eq!((__std_max_element_f(f, fe) as usize - f as usize) / 4, 2);
            let ints = [4i32, -2, 9, 9];
            assert_eq!(__std_max_4i(ints.as_ptr() as *const c_void, ints.as_ptr().add(4) as *const c_void), 9);
        }
    }
}
