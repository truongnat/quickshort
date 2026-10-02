//! macOS OCR via Vision's VNRecognizeTextRequest — no bundled model, runs on
//! the Neural Engine, supports Vietnamese/English text.

use anyhow::{Context, Result};
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation,
    VNRequest, VNRequestTextRecognitionLevel,
};

pub fn extract(img: &RgbaImage) -> Result<String> {
    // Feed Vision encoded image data — a PNG roundtrip is the cheapest way to
    // build a valid NSData source without touching CoreGraphics types.
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let data = NSData::from_vec(png);
    let opts = NSDictionary::<VNImageOption, objc2::runtime::AnyObject>::new();
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &data,
        &opts,
    );

    let req = VNRecognizeTextRequest::new();
    req.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    req.setUsesLanguageCorrection(true);
    req.setAutomaticallyDetectsLanguage(true);
    let langs = NSArray::from_retained_slice(&[
        NSString::from_str("vi-VN"),
        NSString::from_str("en-US"),
    ]);
    req.setRecognitionLanguages(&langs);

    let req_vn: Retained<VNRequest> = unsafe { Retained::cast_unchecked(req.clone()) };
    let reqs = NSArray::from_retained_slice(&[req_vn]);
    handler
        .performRequests_error(&reqs)
        .context("VNRecognizeTextRequest failed")?;

    let mut lines = Vec::new();
    if let Some(results) = req.results() {
        for obs in results.iter() {
            let Some(obs) = obs.downcast_ref::<VNRecognizedTextObservation>() else {
                continue;
            };
            if let Some(best) = obs.topCandidates(1).iter().next() {
                lines.push(best.string().to_string());
            }
        }
    }
    Ok(lines.join("\n"))
}
