//! The Cocoa boundary. All callers run on a worker; only owned RGBA crosses back to Rune.
use block2::RcBlock;
use objc2::{AllocAnyThread, rc::autoreleasepool};
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFString, CFType, CFURL, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality,
};
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyOrientation, kCGImagePropertyPixelHeight,
    kCGImagePropertyPixelWidth, kCGImageSourceSubsampleFactor,
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

/// Decodes an image at about `max_pixels` on its longer side.
///
/// The decoder itself drops pixels by a factor of two, four or eight (`SubsampleFactor`), which
/// for JPEG costs a fraction of ImageIO's thumbnail resampling. The last step down to
/// `max_pixels`, and the EXIF rotation, happen in the one draw that fills the RGBA buffer.
pub fn image(path: &Path, max_pixels: u32) -> Result<Bitmap, String> {
    let url = CFURL::from_file_path(path).ok_or("Invalid image path")?;
    // SAFETY: ImageIO takes these exact CFString keys with CFNumber values, and returns its
    // properties as a CFString-keyed dictionary.
    let (image, orientation) = unsafe {
        let source = CGImageSource::with_url(&url, None).ok_or("This image could not be read.")?;
        let properties = source.properties_at_index(0, None);
        let number = |key: &CFString| {
            let properties = properties.as_deref()?;
            let typed = &*std::ptr::from_ref(properties).cast::<CFDictionary<CFString, CFType>>();
            typed.get(key)?.downcast::<CFNumber>().ok()?.as_i32()
        };
        let longest = number(kCGImagePropertyPixelWidth)
            .max(number(kCGImagePropertyPixelHeight))
            .unwrap_or(0);
        let factor = [8, 4, 2]
            .into_iter()
            .find(|factor| longest / factor >= max_pixels as i32)
            .unwrap_or(1);
        let factor = CFNumber::new_i32(factor);
        let options = CFDictionary::<CFString, CFType>::from_slices(
            &[kCGImageSourceSubsampleFactor],
            &[AsRef::<CFType>::as_ref(&*factor)],
        );
        let image = source
            .image_at_index(0, Some(options.as_opaque()))
            .ok_or("This image could not be decoded.")?;
        (image, number(kCGImagePropertyOrientation).unwrap_or(1))
    };
    let (width, height) = (
        CGImage::width(Some(&image)) as f64,
        CGImage::height(Some(&image)) as f64,
    );
    if width < 1.0 || height < 1.0 {
        return Err("This image could not be decoded.".into());
    }
    let scale = (f64::from(max_pixels) / width.max(height)).min(1.0);
    let (drawn_width, drawn_height) = (
        (width * scale).round().max(1.0),
        (height * scale).round().max(1.0),
    );
    // EXIF orientations 5 to 8 turn the picture a quarter, so the bitmap swaps its sides.
    let (out_width, out_height) = if orientation >= 5 {
        (drawn_height, drawn_width)
    } else {
        (drawn_width, drawn_height)
    };
    draw_rgba(out_width as usize, out_height as usize, |context| {
        let (w, h) = (out_width, out_height);
        // Each case maps the stored picture onto the upright one (Core Graphics is y-up).
        match orientation {
            2 => {
                CGContext::translate_ctm(Some(context), w, 0.0);
                CGContext::scale_ctm(Some(context), -1.0, 1.0);
            }
            3 => {
                CGContext::translate_ctm(Some(context), w, h);
                CGContext::rotate_ctm(Some(context), std::f64::consts::PI);
            }
            4 => {
                CGContext::translate_ctm(Some(context), 0.0, h);
                CGContext::scale_ctm(Some(context), 1.0, -1.0);
            }
            5 => {
                CGContext::translate_ctm(Some(context), w, h);
                CGContext::scale_ctm(Some(context), 1.0, -1.0);
                CGContext::rotate_ctm(Some(context), std::f64::consts::FRAC_PI_2);
            }
            6 => {
                CGContext::translate_ctm(Some(context), 0.0, h);
                CGContext::rotate_ctm(Some(context), -std::f64::consts::FRAC_PI_2);
            }
            7 => {
                CGContext::scale_ctm(Some(context), -1.0, 1.0);
                CGContext::rotate_ctm(Some(context), std::f64::consts::FRAC_PI_2);
            }
            8 => {
                CGContext::translate_ctm(Some(context), w, 0.0);
                CGContext::rotate_ctm(Some(context), std::f64::consts::FRAC_PI_2);
            }
            _ => {}
        }
        CGContext::set_interpolation_quality(Some(context), CGInterpolationQuality::Medium);
        CGContext::draw_image(
            Some(context),
            CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(drawn_width, drawn_height),
            ),
            Some(&image),
        );
    })
}

pub fn thumbnail(path: &Path, cancelled: &impl Fn() -> bool) -> Option<Bitmap> {
    thumbnail_at(path, [700.0, 900.0], cancelled)
}

pub(super) fn thumbnail_at(
    path: &Path,
    [width, height]: [f64; 2],
    cancelled: &impl Fn() -> bool,
) -> Option<Bitmap> {
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
                    CGSize::new(width, height),
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
    draw_rgba(width, height, |context| {
        CGContext::draw_image(
            Some(context),
            CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(width as f64, height as f64),
            ),
            Some(image),
        );
    })
}

/// Fills a straight-alpha RGBA bitmap of this size with whatever `draw` paints.
fn draw_rgba(width: usize, height: usize, draw: impl FnOnce(&CGContext)) -> Result<Bitmap, String> {
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
    draw(&context);
    drop(context);
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        // Photos are opaque: their premultiplied colours are already straight.
        if alpha == 255 {
            continue;
        }
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
