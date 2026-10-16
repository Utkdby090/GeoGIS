//! DJI's Thermal SDK (`libdirp`), loaded at run time. The only `unsafe` code in the workspace lives
//! here, behind a safe [`Sdk`] and [`Photo`] that keep every pointer and buffer valid for the
//! calls that use them. Signatures and structs follow `dirp_api.h` (SDK v1.8; its structs are
//! declared `#pragma pack(1)`, and every field here is 4 bytes wide, so `repr(C)` has no padding).
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::Path;

use libloading::{Library, Symbol};

type Handle = *mut c_void;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Resolution {
    width: i32,
    height: i32,
}

/// `dirp_measurement_params_t`.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub distance: f32,
    pub humidity: f32,
    pub emissivity: f32,
    pub reflection: f32,
    pub ambient_temp: f32,
}

/// `dirp_measurement_params_range_t`: `(min, max)` per setting, in [`Params`]' order.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct Range {
    pub distance: (f32, f32),
    pub humidity: (f32, f32),
    pub emissivity: (f32, f32),
    pub reflection: (f32, f32),
    pub ambient_temp: (f32, f32),
}

type CreateFn = unsafe extern "C" fn(*const u8, i32, *mut Handle) -> i32;
type DestroyFn = unsafe extern "C" fn(Handle) -> i32;
type ResolutionFn = unsafe extern "C" fn(Handle, *mut Resolution) -> i32;
type ParamsFn = unsafe extern "C" fn(Handle, *mut Params) -> i32;
type SetParamsFn = unsafe extern "C" fn(Handle, *const Params) -> i32;
type RangeFn = unsafe extern "C" fn(Handle, *mut Range) -> i32;
type MeasureFn = unsafe extern "C" fn(Handle, *mut f32, i32) -> i32;

/// The loaded library and the functions GeoFlux uses, resolved once.
pub struct Sdk {
    create: CreateFn,
    destroy: DestroyFn,
    resolution: ResolutionFn,
    get_params: ParamsFn,
    set_params: SetParamsFn,
    range: RangeFn,
    measure: MeasureFn,
    // Last, so the function pointers above are never used after it unloads.
    _lib: Library,
}

impl Sdk {
    /// Loads `libdirp` from `dir`. DJI's library loads its helpers (`libv_*`, listed in
    /// `libv_list.ini`) from the same folder, which the caller puts on the library search path
    /// and makes the working directory.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let file = if cfg!(windows) { "libdirp.dll" } else { "libdirp.so" };
        // SAFETY: loading runs the library's initialisers. It is DJI's own library, from the
        // folder GeoGIS ships it in; this process exists so that anything going wrong in it stays
        // here.
        let lib = unsafe { Library::new(dir.join(file)) }.map_err(|e| format!("{file} could not be loaded: {e}"))?;
        fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
            // SAFETY: each `T` is the exact C signature from `dirp_api.h` for `name`.
            let s: Symbol<T> = unsafe { lib.get(name) }.map_err(|e| format!("the SDK has no {}: {e}", String::from_utf8_lossy(name)))?;
            Ok(*s)
        }
        Ok(Self {
            create: sym(&lib, b"dirp_create_from_rjpeg")?,
            destroy: sym(&lib, b"dirp_destroy")?,
            resolution: sym(&lib, b"dirp_get_rjpeg_resolution")?,
            get_params: sym(&lib, b"dirp_get_measurement_params")?,
            set_params: sym(&lib, b"dirp_set_measurement_params")?,
            range: sym(&lib, b"dirp_get_measurement_params_range")?,
            measure: sym(&lib, b"dirp_measure_ex")?,
            _lib: lib,
        })
    }

    /// Opens one R-JPEG. The SDK keeps reading `jpeg` until the handle is destroyed, so the photo
    /// owns its bytes for as long as the handle lives.
    pub fn open(&self, jpeg: Vec<u8>) -> Result<Photo<'_>, i32> {
        let len = i32::try_from(jpeg.len()).map_err(|_| -8)?;
        let mut h: Handle = std::ptr::null_mut();
        // SAFETY: `jpeg` is a live buffer of `len` bytes, moved into the returned `Photo` and kept
        // until after `dirp_destroy`; `h` is a valid out-pointer.
        let code = unsafe { (self.create)(jpeg.as_ptr(), len, &mut h) };
        if code != 0 {
            return Err(code);
        }
        if h.is_null() {
            return Err(-2);
        }
        Ok(Photo { sdk: self, handle: h, _bytes: jpeg })
    }
}

/// One open R-JPEG. Dropping it destroys the SDK handle, on every path (errors included), before
/// its bytes are freed.
pub struct Photo<'a> {
    sdk: &'a Sdk,
    handle: Handle,
    _bytes: Vec<u8>,
}

fn check(code: i32) -> Result<(), i32> {
    if code == 0 { Ok(()) } else { Err(code) }
}

impl Photo<'_> {
    pub fn resolution(&self) -> Result<(i32, i32), i32> {
        let mut r = Resolution::default();
        // SAFETY: valid handle; `r` is a writable `dirp_resolution_t`.
        check(unsafe { (self.sdk.resolution)(self.handle, &mut r) })?;
        Ok((r.width, r.height))
    }

    pub fn params(&self) -> Result<Params, i32> {
        let mut p = Params::default();
        // SAFETY: valid handle; `p` is a writable `dirp_measurement_params_t`.
        check(unsafe { (self.sdk.get_params)(self.handle, &mut p) })?;
        Ok(p)
    }

    pub fn set_params(&self, p: &Params) -> Result<(), i32> {
        // SAFETY: valid handle; `p` is a readable `dirp_measurement_params_t`.
        check(unsafe { (self.sdk.set_params)(self.handle, p) })
    }

    pub fn range(&self) -> Result<Range, i32> {
        let mut r = Range::default();
        // SAFETY: valid handle; `r` is a writable `dirp_measurement_params_range_t`.
        check(unsafe { (self.sdk.range)(self.handle, &mut r) })?;
        Ok(r)
    }

    /// Temperatures into `out`, which must hold exactly width × height values.
    pub fn measure(&self, out: &mut [f32]) -> Result<(), i32> {
        let bytes = out.len().checked_mul(4).and_then(|n| i32::try_from(n).ok()).ok_or(-8)?;
        // SAFETY: valid handle; `out` is a writable buffer of `bytes` bytes, the size the SDK
        // is told.
        check(unsafe { (self.sdk.measure)(self.handle, out.as_mut_ptr(), bytes) })
    }
}

impl Drop for Photo<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle came from `dirp_create_from_rjpeg` and is destroyed exactly once.
        unsafe { (self.sdk.destroy)(self.handle) };
    }
}
