//! The Cocoa boundary. All callers run on a worker; only owned RGBA crosses back to Rune.
use block2::RcBlock;
use objc2::{AllocAnyThread, rc::autoreleasepool};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFString, CFType, CFURL, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_image_io::{
    CGImageSource, kCGImageSourceCreateThumbnailFromImageAlways,
    kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceThumbnailMaxPixelSize,
};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
    QLThumbnailGenerator, QLThumbnailRepresentation,
};
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn image(path: &Path, max_pixels: u32) -> Result<Bitmap, String> {
    let url = CFURL::from_file_path(path).ok_or("Invalid image path")?;
    let limit = CFNumber::new_i32(max_pixels as i32);
    // ImageIO takes these exact CFString keys and CFBoolean/CFNumber values. It decodes a
    // transformed thumbnail directly, rather than allocating the full camera image first.
    let image = unsafe {
        let options = CFDictionary::<CFString, CFType>::from_slices(
            &[
                kCGImageSourceCreateThumbnailFromImageAlways,
                kCGImageSourceCreateThumbnailWithTransform,
                kCGImageSourceThumbnailMaxPixelSize,
            ],
            &[
                CFBoolean::new(true).as_ref(),
                CFBoolean::new(true).as_ref(),
                AsRef::<CFType>::as_ref(&*limit),
            ],
        );
        let source = CGImageSource::with_url(&url, None).ok_or("This image could not be read.")?;
        source
            .thumbnail_at_index(0, Some(options.as_opaque()))
            .ok_or("This image could not be decoded.")?
    };
    rgba(&image)
}

pub fn thumbnail(path: &Path, cancelled: &impl Fn() -> bool) -> Option<Bitmap> {
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let (send, receive) = mpsc::sync_channel(1);
        // Quick Look owns the copied callback until completion. Pixels are copied while its
        // representation is valid; Objective-C objects never leave the completion queue.
        unsafe {
            let generator = QLThumbnailGenerator::sharedGenerator();
            let request =
                QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                    QLThumbnailGenerationRequest::alloc(),
                    &url,
                    CGSize::new(700.0, 900.0),
                    1.0,
                    QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
                );
            let callback = RcBlock::new(
                move |representation: *mut QLThumbnailRepresentation, _error: *mut NSError| {
                    let bitmap = representation
                        .as_ref()
                        .and_then(|r| rgba(&r.CGImage()).ok());
                    let _ = send.try_send(bitmap);
                },
            );
            generator.generateBestRepresentationForRequest_completionHandler(&request, &callback);
            let start = Instant::now();
            loop {
                if cancelled() || start.elapsed() > Duration::from_secs(8) {
                    generator.cancelRequest(&request);
                    return None;
                }
                match receive.recv_timeout(Duration::from_millis(25)) {
                    Ok(value) => return value,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                }
            }
        }
    })
}

fn rgba(image: &CGImage) -> Result<Bitmap, String> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("The preview image is too large.".into());
    }
    let mut data = vec![0u8; width * height * 4];
    let space = CGColorSpace::new_device_rgb().ok_or("Could not create a preview color space.")?;
    // The buffer is retained until the context is dropped and sized for its exact stride.
    let context = unsafe {
        CGBitmapContextCreate(
            data.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }
    .ok_or("Could not create a preview bitmap.")?;
    CGContext::draw_image(
        Some(&context),
        CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        ),
        Some(image),
    );
    drop(context);
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for c in &mut pixel[..3] {
            *c = (u32::from(*c) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
    }
    Ok(Bitmap {
        width: width as u32,
        height: height as u32,
        rgba: data,
    })
}
