//! Immutable CoreVideo storage shared by the raster cache and Metal, without an RGBA atlas.
use std::{ffi::c_void, ptr::NonNull};

use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext};
use rune::native_surface::macos::PixelBuffer;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferCreate(
        allocator: *const c_void,
        width: usize,
        height: usize,
        format: u32,
        attributes: *const CFDictionary,
        output: *mut *mut CFType,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: *const CFType, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *const CFType, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *const CFType) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *const CFType) -> usize;
    fn CVPixelBufferGetDataSize(buffer: *const CFType) -> usize;
    static kCVPixelBufferIOSurfacePropertiesKey: &'static CFString;
    static kCVPixelBufferMetalCompatibilityKey: &'static CFString;
}

#[derive(Clone)]
pub struct Bitmap {
    pub pixels: PixelBuffer,
    pub bytes: usize,
}

impl Bitmap {
    pub fn draw(
        width: usize,
        height: usize,
        draw: impl FnOnce(&CGContext),
    ) -> Result<Self, String> {
        let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        // SAFETY: CoreVideo exports these CFString keys; the dictionaries live through create.
        let attributes = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[
                    AsRef::<CFType>::as_ref(&*empty),
                    CFBoolean::new(true).as_ref(),
                ],
            )
        };
        let mut raw = std::ptr::null_mut();
        // SAFETY: out points to initialized pointer storage. The returned buffer has +1 ownership.
        let status = unsafe {
            CVPixelBufferCreate(
                std::ptr::null(),
                width,
                height,
                u32::from_be_bytes(*b"BGRA"),
                attributes.as_opaque(),
                &mut raw,
            )
        };
        if status != 0 {
            return Err(format!("Could not allocate PDF bitmap ({status})"));
        }
        // SAFETY: A successful CoreVideo create returns a retained CF object.
        let buffer =
            unsafe { CFRetained::from_raw(NonNull::new(raw).ok_or("Missing PDF bitmap")?) };
        // SAFETY: This newly created buffer has no readers until drawing and unlocking finish.
        let status = unsafe { CVPixelBufferLockBaseAddress(&*buffer, 0) };
        if status != 0 {
            return Err(format!("Could not lock PDF bitmap ({status})"));
        }
        let result: Result<usize, String> = (|| {
            // SAFETY: The locked buffer owns its base address and stride through context teardown.
            let (data, stride, bytes) = unsafe {
                (
                    CVPixelBufferGetBaseAddress(&*buffer),
                    CVPixelBufferGetBytesPerRow(&*buffer),
                    CVPixelBufferGetDataSize(&*buffer),
                )
            };
            if data.is_null() || stride < width * 4 {
                return Err("Invalid PDF bitmap storage".into());
            }
            // SAFETY: sRGB is a system constant; BGRA is little-endian premultiplied-first.
            let space =
                CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
                    .ok_or("Unable to create PDF color space")?;
            let context = unsafe {
                CGBitmapContextCreate(data, width, height, 8, stride, Some(&space), 0x2002)
            }
            .ok_or("Unable to create PDF drawing context")?;
            CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_rect(
                Some(&context),
                CGRect::new(
                    CGPoint::new(0.0, 0.0),
                    CGSize::new(width as f64, height as f64),
                ),
            );
            draw(&context);
            drop(context);
            Ok(bytes)
        })();
        // SAFETY: Balances the successful lock, including when context creation fails.
        let status = unsafe { CVPixelBufferUnlockBaseAddress(&*buffer, 0) };
        if status != 0 {
            return Err(format!("Could not unlock PDF bitmap ({status})"));
        }
        let bytes = result?;
        // SAFETY: Drawing is complete. No mutable aliases escape; all later access is read-only.
        let pixels = unsafe { PixelBuffer::from_raw(raw.cast()) }?;
        Ok(Self { pixels, bytes })
    }
}
