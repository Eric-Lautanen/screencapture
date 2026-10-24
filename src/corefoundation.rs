//! CoreFoundation C surface (`libCoreFoundation`, loaded by bundle
//! path): the collection and number exports the macOS window-list
//! query walks. Signatures transcribed from Apple's SDK headers
//! (`CFArray.h`, `CFDictionary.h`, `CFNumber.h`, `CFString.h`, and
//! `CFBase.h` for `CFIndex`/`Boolean`).

use std::ffi::{c_char, c_long, c_void};
use std::io;
use std::mem;

use crate::dylib;

/// The CoreFoundation framework binary inside the system bundle.
const FRAMEWORK: &str = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";

/// `kCFNumberFloat64Type` (`CFNumber.h`): the answer type that makes
/// `CFNumberGetValue` coerce any `CFNumber` (integer or float) to `f64`.
const NUMBER_F64: i32 = 6;

type ArrayCountProc = unsafe extern "C" fn(Ref) -> c_long;
type ArrayValueProc = unsafe extern "C" fn(Ref, c_long) -> *const c_void;
type DictValueProc = unsafe extern "C" fn(Ref, *const c_void) -> *const c_void;
type NumberValueProc = unsafe extern "C" fn(Ref, i32, *mut c_void) -> u8;
type StringCreateProc = unsafe extern "C" fn(*const c_void, *const c_char, u32) -> Ref;
type ReleaseProc = unsafe extern "C" fn(Ref);

/// Every CoreFoundation export the window-list query uses; signatures
/// match the SDK headers exactly. Loaded per construction.
pub struct Api {
    /// `CFArrayGetCount`.
    pub array_count: ArrayCountProc,
    /// `CFArrayGetValueAtIndex`.
    pub array_value: ArrayValueProc,
    /// `CFDictionaryGetValue`.
    pub dict_value: DictValueProc,
    /// `CFNumberGetValue`.
    pub number_value: NumberValueProc,
    /// `CFStringCreateWithCString`.
    pub utf8_string: StringCreateProc,
    /// `CFRelease`.
    pub release: ReleaseProc,
}

impl Api {
    /// Load CoreFoundation and resolve every export. The library handle
    /// is parked loaded for the process lifetime (the function pointers
    /// above go invalid otherwise).
    ///
    /// # Errors
    /// The framework is absent or dropped an expected export.
    pub fn load() -> io::Result<Self> {
        let library = dylib::Library::open(FRAMEWORK)?;
        let api = Self {
            // SAFETY: each export exists in CoreFoundation with exactly
            // the signature transcribed from the SDK headers above; the
            // transmutes re-type live function addresses.
            array_count: symbol(&library, b"CFArrayGetCount\0")?,
            array_value: symbol(&library, b"CFArrayGetValueAtIndex\0")?,
            dict_value: symbol(&library, b"CFDictionaryGetValue\0")?,
            number_value: symbol(&library, b"CFNumberGetValue\0")?,
            utf8_string: symbol(&library, b"CFStringCreateWithCString\0")?,
            release: symbol(&library, b"CFRelease\0")?,
        };
        mem::forget(library);
        Ok(api)
    }

    /// The length of a `CFArray` (0 for null or oversized answers).
    pub fn array_len(&self, array: Ref) -> usize {
        if array.is_null() {
            return 0;
        }
        // SAFETY: a live array handle on the same runtime that made it.
        let count = unsafe { (self.array_count)(array) };
        usize::try_from(count).unwrap_or(0)
    }

    /// One `CFArray` element (`null` past the end or for absent values).
    pub fn array_value(&self, array: Ref, index: usize) -> *const c_void {
        let Ok(index) = c_long::try_from(index) else {
            return std::ptr::null();
        };
        // SAFETY: the index range was checked by the caller's loop.
        unsafe { (self.array_value)(array, index) }
    }

    /// One `CFDictionary` element (`null` when the key is absent).
    pub fn dict_value(&self, dict: Ref, key: Ref) -> *const c_void {
        // SAFETY: a live dictionary handle; absent keys answer null.
        unsafe { (self.dict_value)(dict, key) }
    }

    /// One `CFNumber` as `f64` (integer or float values coerce; `None`
    /// for absent or malformed entries).
    pub fn number_f64(&self, number: Ref) -> Option<f64> {
        if number.is_null() {
            return None;
        }
        let mut value: f64 = 0.0;
        // SAFETY: the pointer targets the local above; the type id is
        // the SDK-verified 64-bit IEEE constant.
        let ok = unsafe { (self.number_value)(number, NUMBER_F64, (&raw mut value).cast()) };
        (ok != 0).then_some(value)
    }

    /// One UTF-8 `CFStringRef`; release it with [`Self::release`].
    /// `None` when the text holds an interior NUL or CG refuses.
    pub fn utf8(&self, text: &str) -> Option<Ref> {
        let bytes = std::ffi::CString::new(text).ok()?;
        // SAFETY: a null allocator selects the default; the encoding
        // constant is `kCFStringEncodingUTF8` from `CFString.h`.
        let string = unsafe { (self.utf8_string)(std::ptr::null(), bytes.as_ptr(), ENCODING_UTF8) };
        (!string.is_null()).then_some(string)
    }

    /// Release one object this module or CoreGraphics handed out.
    pub fn release(&self, object: Ref) {
        // SAFETY: a live handle that was allocated with +1 retention.
        unsafe { (self.release)(object) };
    }
}

/// `kCFStringEncodingUTF8` (`CFString.h`).
const ENCODING_UTF8: u32 = 0x0800_0100;

/// A CoreFoundation object reference (`CF*Ref`) as a plain pointer.
pub type Ref = *mut c_void;

/// Resolve one export as a function pointer with the exact header
/// signature the caller's field declares.
///
/// # Errors
/// The framework does not export the name.
fn symbol<F: Copy>(library: &dylib::Library, name: &[u8]) -> io::Result<F> {
    let raw = library.symbol(name)?;
    // SAFETY: `raw` is a live function address; the caller transcribed
    // the import's signature from the framework's own headers.
    Ok(unsafe { mem::transmute_copy(&raw) })
}
