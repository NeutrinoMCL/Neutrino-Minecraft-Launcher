//! 把内存里的图片字节 / RGBA 缓冲转换成 `slint::Image`。
//!
//! Slint 的 `Image.source` 是 `image` 类型而不是 URL，所以远程 / 本地图片都要先在这里解码。

/// 解码 PNG / JPEG / WebP / GIF 等内存字节成 RGBA。
///
/// `slint::Image` 不是 `Send`，所以耗时的解码放在后台线程做，回到 UI 线程再 [`from_rgba`]。
pub fn decode_rgba(bytes: &[u8]) -> Option<image::RgbaImage> {
    image::load_from_memory(bytes)
        .ok()
        .map(|image| image.to_rgba8())
}

/// 把一张 RGBA 图转成 `slint::Image`。
pub fn from_rgba(image: &image::RgbaImage) -> slint::Image {
    let (width, height) = image.dimensions();
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);

    for (destination, source) in pixels.make_mut_slice().iter_mut().zip(image.pixels()) {
        *destination = slint::Rgba8Pixel {
            r: source[0],
            g: source[1],
            b: source[2],
            a: source[3],
        };
    }

    slint::Image::from_rgba8(pixels)
}
