//! COM memory management and type conversion utilities for the OPC DA client.
//!
//! This module provides safe wrappers around COM memory allocations and arrays,
//! as well as traits for converting between COM-native and Rust-native types.

use windows::Win32::System::Variant::{VARIANT, VariantClear};
use windows::{
    Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree},
    core::PWSTR,
};

// ── Memory Management ───────────────────────────────────────────────

/// A safe wrapper around arrays allocated by COM.
///
/// This struct ensures proper cleanup of COM-allocated memory when dropped.
/// It provides safe access to the underlying array through slices.
///
/// `Clone` is deliberately **not** derived: the inner [`RemotePointer`]
/// owns COM-allocated memory that `Drop` frees, so a derived `Clone` would
/// be a shallow pointer copy → double-free on drop.
#[derive(Debug, PartialEq)]
pub struct RemoteArray<T: Sized> {
    pointer: RemotePointer<T>,
    len: u32,
}

impl<T: Sized> RemoteArray<T> {
    /// Creates a new `RemoteArray` with the specified length.
    /// The underlying pointer is initialized to null.
    #[inline(always)]
    pub fn new(len: u32) -> Self {
        Self {
            pointer: RemotePointer::null(),
            len,
        }
    }

    /// Creates a `RemoteArray` from a raw pointer and length.
    ///
    /// # Safety
    /// The caller must ensure that the pointer is valid and points to a COM-allocated array.
    #[inline(always)]
    pub(crate) fn from_mut_ptr(pointer: *mut T, len: u32) -> Self {
        Self {
            pointer: RemotePointer::from_raw(pointer),
            len,
        }
    }

    /// Creates a `RemoteArray` from a constant pointer and length.
    ///
    /// # Safety
    /// The caller must ensure that the pointer is valid and points to a COM-allocated array.
    #[inline(always)]
    pub(crate) fn from_ptr(pointer: *const T, len: u32) -> Self {
        Self {
            pointer: RemotePointer::from_raw(pointer as *mut T),
            len,
        }
    }

    /// Creates an empty `RemoteArray`.
    #[inline(always)]
    pub fn empty() -> Self {
        Self {
            pointer: RemotePointer::null(),
            len: 0,
        }
    }

    /// Returns a mutable pointer to the array pointer.
    ///
    /// This is useful when calling COM functions that output an array via a pointer to a pointer.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut *mut T {
        self.pointer.as_mut_ptr()
    }

    /// Returns a slice to the underlying array.
    ///
    /// # Safety
    /// The caller must ensure that the `pointer` is valid for reads and points to an array of `len` elements.
    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        if self.pointer.inner.is_null() || self.len == 0 {
            return &[];
        }

        let len = usize::try_from(self.len).unwrap_or(0);

        // Pointer and length are guaranteed to be valid
        unsafe { core::slice::from_raw_parts(self.pointer.inner, len) }
    }

    /// Returns a mutable slice to the underlying array.
    ///
    /// # Safety
    /// The caller must ensure that the `pointer` is valid for reads and writes and points to an array of `len` elements.
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.pointer.inner.is_null() || self.len == 0 {
            return &mut [];
        }

        let len = usize::try_from(self.len).unwrap_or(0);

        // Pointer and length are guaranteed to be valid
        unsafe { core::slice::from_raw_parts_mut(self.pointer.inner, len) }
    }

    /// Returns the length of the array.
    #[inline(always)]
    pub fn len(&self) -> u32 {
        if self.pointer.inner.is_null() {
            return 0;
        }

        self.len
    }

    /// Checks if the array is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0 || self.pointer.inner.is_null()
    }

    /// Returns a mutable pointer to the length.
    ///
    /// This is useful when calling COM functions that output the length via a pointer.
    #[inline(always)]
    pub fn as_mut_len_ptr(&mut self) -> *mut u32 {
        &mut self.len
    }

    /// Sets the length of the array.
    ///
    /// # Safety
    /// The caller must ensure that the new length is valid for the underlying array.
    #[inline(always)]
    pub(crate) unsafe fn set_len(&mut self, len: u32) {
        self.len = len;
    }
}

impl<T: Sized> Default for RemoteArray<T> {
    /// Creates an empty `RemoteArray` by default.
    #[inline(always)]
    fn default() -> Self {
        Self::empty()
    }
}

/// A safe wrapper around a pointer allocated by COM.
///
/// This struct ensures proper cleanup of COM-allocated memory when dropped.
/// It provides methods to access the underlying pointer.
#[repr(transparent)]
#[derive(Debug, PartialEq)]
pub struct RemotePointer<T: Sized> {
    inner: *mut T,
}

impl<T: Sized> RemotePointer<T> {
    /// Creates a new `RemotePointer` initialized to null.
    #[inline(always)]
    pub fn null() -> Self {
        Self {
            inner: core::ptr::null_mut(),
        }
    }

    /// Returns a mutable pointer to the inner pointer.
    ///
    /// Useful for COM functions that output data via a pointer to a pointer.
    #[inline(always)]
    pub(crate) fn from_raw(pointer: *mut T) -> Self {
        Self { inner: pointer }
    }

    pub(crate) fn copy_slice(value: &[T]) -> Self {
        let pointer = unsafe { CoTaskMemAlloc(core::mem::size_of_val(value)) };
        unsafe {
            core::ptr::copy_nonoverlapping(value.as_ptr(), pointer as _, value.len());
        }
        Self {
            inner: pointer as _,
        }
    }

    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut *mut T {
        &mut self.inner
    }

    /// Returns an `Option` referencing the inner value if it is not null.
    ///
    /// # Safety
    /// The caller must ensure that the inner pointer is valid for reads.
    #[inline(always)]
    pub fn as_ref(&self) -> Option<&T> {
        // Pointer is guaranteed to be valid
        unsafe { self.inner.as_ref() }
    }

    #[inline(always)]
    pub fn ok(&self) -> windows::core::Result<&T> {
        // Pointer is guaranteed to be valid
        unsafe { self.inner.as_ref() }.ok_or_else(|| {
            windows::core::Error::new(windows::Win32::Foundation::E_POINTER, "Pointer is null")
        })
    }

    #[inline(always)]
    pub fn from_option<R: Into<RemotePointer<T>>>(value: Option<R>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::null(),
        }
    }
}

impl<T: Sized> Default for RemotePointer<T> {
    /// Creates a new `RemotePointer` initialized to null by default.
    #[inline(always)]
    fn default() -> Self {
        Self::null()
    }
}

impl From<PWSTR> for RemotePointer<u16> {
    /// Converts a `PWSTR` to a `RemotePointer<u16>`.
    #[inline(always)]
    fn from(value: PWSTR) -> Self {
        Self {
            inner: value.as_ptr(),
        }
    }
}

impl From<&str> for RemotePointer<u16> {
    /// Converts a string slice to a `RemotePointer<u16>`.
    #[inline(always)]
    fn from(value: &str) -> Self {
        Self::copy_slice(&value.encode_utf16().chain(Some(0)).collect::<Vec<u16>>())
    }
}

impl TryFrom<RemotePointer<u16>> for String {
    type Error = windows::core::Error;

    /// Attempts to convert a `RemotePointer<u16>` to a `String`.
    ///
    /// # Errors
    /// Returns an error if the pointer is null or if the string conversion fails.
    #[inline(always)]
    fn try_from(value: RemotePointer<u16>) -> Result<Self, Self::Error> {
        if value.inner.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }

        // Has checked for null pointer
        Ok(unsafe { PWSTR(value.inner).to_string() }?)
    }
}

impl TryFrom<RemotePointer<u16>> for Option<String> {
    type Error = windows::core::Error;

    /// Attempts to convert a `RemotePointer<u16>` to an `Option<String>`.
    ///
    /// # Errors
    /// Returns an error if the string conversion fails.
    #[inline(always)]
    fn try_from(value: RemotePointer<u16>) -> Result<Self, Self::Error> {
        if value.inner.is_null() {
            return Ok(None);
        }

        // Has checked for null pointer
        Ok(Some(unsafe { PWSTR(value.inner).to_string() }?))
    }
}

impl RemotePointer<u16> {
    /// Returns a mutable pointer to a `PWSTR`.
    #[inline(always)]
    pub fn as_mut_pwstr_ptr(&mut self) -> *mut PWSTR {
        &mut self.inner as *mut *mut u16 as *mut PWSTR
    }
}

impl<T: Sized> Drop for RemotePointer<T> {
    /// Drops the `RemotePointer`, freeing the COM-allocated memory.
    #[inline(always)]
    fn drop(&mut self) {
        if !self.inner.is_null() {
            unsafe {
                CoTaskMemFree(Some(self.inner as _));
            }
        }
    }
}

/// Release each VARIANT in a COM-allocated array, preventing BSTR/SafeArray leaks when the
/// array's backing buffer is freed by [`RemotePointer::drop`].
///
/// Call this before the `RemoteArray<VARIANT>` goes out of scope.
///
/// # Safety contract (caller)
///
/// Every element must be a valid COM-allocated VARIANT (produced by the OPC server).
pub fn clear_variant_array(arr: &mut RemoteArray<VARIANT>) {
    for v in arr.as_mut_slice() {
        // SAFETY: each element is a COM-allocated VARIANT; VariantClear releases its
        // embedded resources (BSTR, SafeArray, IUnknown) via the COM allocator.
        unsafe {
            let _ = VariantClear(v);
        }
    }
}

/// Release the `vDataValue` VARIANT in each `tagOPCITEMSTATE` (read-path cleanup).
///
/// Call this before the `RemoteArray<tagOPCITEMSTATE>` goes out of scope.
pub fn clear_item_states(arr: &mut RemoteArray<crate::bindings::da::tagOPCITEMSTATE>) {
    for s in arr.as_mut_slice() {
        // SAFETY: vDataValue is COM-allocated; VariantClear releases its resources.
        unsafe {
            let _ = VariantClear(&mut s.vDataValue);
        }
    }
}

/// A safe wrapper around locally allocated memory needing to be passed to COM functions.
///
/// This struct is useful for preparing data to be read by COM functions.
pub struct LocalPointer<T: Sized> {
    inner: Option<Box<T>>,
}

impl<T: Sized> LocalPointer<T> {
    /// Creates a new `LocalPointer` from an optional value.
    #[inline(always)]
    pub fn new(value: Option<T>) -> Self {
        Self {
            inner: value.map(Box::new),
        }
    }

    /// Creates a `LocalPointer` from a boxed value.
    #[inline(always)]
    pub fn from_box(value: Box<T>) -> Self {
        Self { inner: Some(value) }
    }

    #[inline(always)]
    pub fn from_option<R: Into<LocalPointer<T>>>(value: Option<R>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::new(None),
        }
    }

    /// Returns a constant pointer to the inner value.
    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        match &self.inner {
            Some(value) => value.as_ref() as *const T,
            None => std::ptr::null_mut(),
        }
    }

    /// Returns a mutable pointer to the inner value.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        match &mut self.inner {
            Some(value) => value.as_mut() as *mut T,
            None => std::ptr::null_mut(),
        }
    }

    /// Consumes the `LocalPointer`, returning the inner value if it exists.
    #[inline(always)]
    pub fn into_inner(self) -> Option<T> {
        self.inner.map(|v| *v)
    }

    /// Returns a reference to the inner value if it exists.
    #[inline(always)]
    pub fn inner(&self) -> Option<&T> {
        self.inner.as_ref().map(|v| v.as_ref())
    }
}

// Implementations for string handling

impl<S: AsRef<str>> From<S> for LocalPointer<Vec<u16>> {
    /// Converts a string slice to a `LocalPointer` containing a UTF-16 encoded null-terminated string.
    #[inline(always)]
    fn from(s: S) -> Self {
        Self::new(Some(s.as_ref().encode_utf16().chain(Some(0)).collect()))
    }
}

impl From<&[String]> for LocalPointer<Vec<Vec<u16>>> {
    /// Converts a slice of `String`s to a `LocalPointer` containing vectors of UTF-16 encoded null-terminated strings.
    #[inline(always)]
    fn from(values: &[String]) -> Self {
        Self::new(Some(
            values
                .iter()
                .map(|s| s.encode_utf16().chain(Some(0)).collect())
                .collect(),
        ))
    }
}

impl<T> LocalPointer<Vec<T>> {
    /// Returns the length of the inner vector.
    #[inline(always)]
    pub fn len(&self) -> usize {
        match &self.inner {
            Some(values) => values.len(),
            None => 0,
        }
    }

    /// Checks if the inner vector is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        match &self.inner {
            Some(values) => values.is_empty(),
            None => true,
        }
    }

    /// Returns a constant pointer to the inner array.
    #[inline(always)]
    pub fn as_array_ptr(&self) -> *const T {
        match &self.inner {
            Some(values) => values.as_ptr(),
            None => std::ptr::null(),
        }
    }

    /// Returns a mutable pointer to the inner array.
    #[inline(always)]
    pub fn as_mut_array_ptr(&mut self) -> *mut T {
        match &mut self.inner {
            Some(values) => values.as_mut_ptr(),
            None => std::ptr::null_mut(),
        }
    }
}

impl LocalPointer<Vec<Vec<u16>>> {
    /// Converts the inner vector of UTF-16 strings to a vector of `PWSTR`.
    #[inline(always)]
    pub fn as_pwstr_array(&self) -> Vec<windows::core::PWSTR> {
        match &self.inner {
            Some(values) => values
                .iter()
                .map(|value| windows::core::PWSTR(value.as_ptr() as _))
                .collect(),
            None => vec![windows::core::PWSTR::null()],
        }
    }

    /// Converts the inner vector of UTF-16 strings to a vector of `PCWSTR`.
    #[inline(always)]
    pub fn as_pcwstr_array(&self) -> Vec<windows::core::PCWSTR> {
        match &self.inner {
            Some(values) => values
                .iter()
                .map(|value| windows::core::PCWSTR::from_raw(value.as_ptr() as _))
                .collect(),
            None => vec![windows::core::PCWSTR::null()],
        }
    }
}

impl LocalPointer<Vec<u16>> {
    /// Converts the inner UTF-16 string to a `PWSTR`.
    #[inline(always)]
    pub fn as_pwstr(&self) -> windows::core::PWSTR {
        match &self.inner {
            Some(value) => windows::core::PWSTR(value.as_ptr() as _),
            None => windows::core::PWSTR::null(),
        }
    }

    /// Converts the inner UTF-16 string to a `PCWSTR`.
    #[inline(always)]
    pub fn as_pcwstr(&self) -> windows::core::PCWSTR {
        match &self.inner {
            Some(value) => windows::core::PCWSTR::from_raw(value.as_ptr() as _),
            None => windows::core::PCWSTR::null(),
        }
    }
}

// ── Native Conversion Traits ────────────────────────────────────────

pub(crate) trait IntoBridge<Bridge> {
    fn into_bridge(self) -> Bridge;
}

pub(crate) trait ToNative<Native> {
    fn to_native(&self) -> Native;
}

pub(crate) trait FromNative<Native> {
    fn from_native(native: &Native) -> Self
    where
        Self: Sized;
}

pub(crate) trait TryToNative<Native> {
    fn try_to_native(&self) -> windows::core::Result<Native>;
}

pub(crate) trait TryFromNative<Native> {
    fn try_from_native(native: &Native) -> windows::core::Result<Self>
    where
        Self: Sized;
}

pub(crate) trait TryToLocal<Local> {
    fn try_to_local(&self) -> windows::core::Result<Local>;
}

impl<Native, T: TryFromNative<Native>> TryToLocal<T> for Native {
    fn try_to_local(&self) -> windows::core::Result<T> {
        T::try_from_native(self)
    }
}

impl<Native, T: FromNative<Native>> TryFromNative<Native> for T {
    fn try_from_native(native: &Native) -> windows::core::Result<Self> {
        Ok(Self::from_native(native))
    }
}

impl<Native, T: ToNative<Native>> TryToNative<Native> for T {
    fn try_to_native(&self) -> windows::core::Result<Native> {
        Ok(self.to_native())
    }
}

impl<Bridge, B: IntoBridge<Bridge>> IntoBridge<Vec<Bridge>> for Vec<B> {
    fn into_bridge(self) -> Vec<Bridge> {
        self.into_iter().map(IntoBridge::into_bridge).collect()
    }
}

impl<Bridge, B: IntoBridge<Bridge> + Clone> IntoBridge<Vec<Bridge>> for &[B] {
    fn into_bridge(self) -> Vec<Bridge> {
        self.iter().cloned().map(IntoBridge::into_bridge).collect()
    }
}

impl<Native, T: TryToNative<Native>> TryToNative<Vec<Native>> for Vec<T> {
    fn try_to_native(&self) -> windows::core::Result<Vec<Native>> {
        self.iter().map(TryToNative::try_to_native).collect()
    }
}

impl TryFromNative<RemoteArray<windows::core::HRESULT>> for Vec<windows::core::Result<()>> {
    fn try_from_native(
        native: &RemoteArray<windows::core::HRESULT>,
    ) -> windows::core::Result<Self> {
        Ok(native.as_slice().iter().map(|v| (*v).ok()).collect())
    }
}

impl<Native, T: TryFromNative<Native>> TryFromNative<RemoteArray<Native>> for Vec<T> {
    fn try_from_native(native: &RemoteArray<Native>) -> windows::core::Result<Self> {
        native.as_slice().iter().map(T::try_from_native).collect()
    }
}

impl<Native, T: TryFromNative<Native>>
    TryFromNative<(RemoteArray<Native>, RemoteArray<windows::core::HRESULT>)>
    for Vec<windows::core::Result<T>>
{
    fn try_from_native(
        native: &(RemoteArray<Native>, RemoteArray<windows::core::HRESULT>),
    ) -> windows::core::Result<Self> {
        let (results, errors) = native;
        if results.len() != errors.len() {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_INVALIDARG,
                "Results and errors arrays have different lengths",
            ));
        }

        Ok(results
            .as_slice()
            .iter()
            .zip(errors.as_slice())
            .map(|(result, error)| {
                if error.is_ok() {
                    T::try_from_native(result)
                } else {
                    Err((*error).into())
                }
            })
            .collect())
    }
}

impl TryFromNative<windows::Win32::Foundation::FILETIME> for std::time::SystemTime {
    fn try_from_native(
        native: &windows::Win32::Foundation::FILETIME,
    ) -> windows::core::Result<Self> {
        let ft = ((native.dwHighDateTime as u64) << 32) | (u64::from(native.dwLowDateTime));
        let duration_since_1601 = std::time::Duration::from_nanos(ft * 100);

        let windows_to_unix_epoch_diff = std::time::Duration::from_secs(11_644_473_600);
        let duration_since_unix_epoch = duration_since_1601
            .checked_sub(windows_to_unix_epoch_diff)
            .ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "FILETIME is before UNIX_EPOCH",
                )
            })?;

        Ok(std::time::UNIX_EPOCH + duration_since_unix_epoch)
    }
}

#[macro_export]
/// Helper macro for instantiating native COM structs from safe types.
macro_rules! try_from_native {
    ($native:expr) => {
        $crate::opc_da::com_utils::TryFromNative::try_from_native($native)?
    };
}

impl TryToNative<windows::Win32::Foundation::FILETIME> for std::time::SystemTime {
    fn try_to_native(&self) -> windows::core::Result<windows::Win32::Foundation::FILETIME> {
        let duration_since_unix_epoch =
            self.duration_since(std::time::UNIX_EPOCH).map_err(|_| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "SystemTime is before UNIX_EPOCH",
                )
            })?;

        let duration_since_windows_epoch =
            duration_since_unix_epoch + std::time::Duration::from_secs(11_644_473_600);

        let ft = duration_since_windows_epoch.as_nanos() / 100;

        Ok(windows::Win32::Foundation::FILETIME {
            dwLowDateTime: ft as u32,
            dwHighDateTime: (ft >> 32) as u32,
        })
    }
}

impl TryFromNative<windows::core::PWSTR> for String {
    fn try_from_native(native: &windows::core::PWSTR) -> windows::core::Result<Self> {
        RemotePointer::from(*native).try_into()
    }
}
