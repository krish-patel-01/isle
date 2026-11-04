// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Previews of the Files tab's files (thumbnails.js): Explorer's own thumbnail of a file, a picture cut to the
//! tile's square or a document's page whole; else the file's icon.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat, RgbaImage, imageops::FilterType};

/// The tile's preview is 52 CSS px square; made at twice that, sharp at 200%.
const SIZE: u32 = 104;

/// What a tile shows: a PNG preview (whole: a page, fitted rather than cut to the square).
pub struct Preview {
    pub png: Vec<u8>,
    pub whole: bool,
}

/// The kinds of file whose preview fills the square, cut to it.
pub fn is_picture(path: &Path) -> bool {
    let ext = path.extension().map(|ext| ext.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "heic" | "heif" | "tif" | "tiff" | "avif")
}

/// `path`'s thumbnail as Explorer makes it; none for a file without one (it then shows its icon).
pub fn preview(path: &Path) -> Option<Preview> {
    let (width, height, rgba) = imp::thumbnail(path, SIZE * 2)?;
    let image = RgbaImage::from_raw(width, height, rgba)?;
    let picture = is_picture(path);
    let image = if picture {
        // Its middle, square, as the tile shows it.
        let side = width.min(height);
        let cut = DynamicImage::ImageRgba8(image).crop_imm((width - side) / 2, (height - side) / 2, side, side);
        if side > SIZE { cut.resize_exact(SIZE, SIZE, FilterType::Triangle) } else { cut }
    } else {
        let fitted = DynamicImage::ImageRgba8(image);
        if width.max(height) > SIZE { fitted.resize(SIZE, SIZE, FilterType::Triangle) } else { fitted }
    };
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, ImageFormat::Png).ok()?;
    Some(Preview { png: png.into_inner(), whole: !picture })
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY,
    };
    use windows::core::HSTRING;

    /// Explorer's thumbnail of `path`, about `size` px: its width, height and RGBA pixels, top row first.
    pub fn thumbnail(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
        // SAFETY: COM on this thread, a shell item for the path, its thumbnail's pixels read into a buffer of
        // the size it reports, and the bitmap freed.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
            let bitmap = factory
                .GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK)
                .ok()?;
            let dc = GetDC(None);
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, ..Default::default() },
                ..Default::default()
            };
            let mut result = None;
            if GetDIBits(dc, bitmap, 0, 0, None, &mut header, DIB_RGB_COLORS) != 0 {
                let (width, height) = (header.bmiHeader.biWidth, header.bmiHeader.biHeight.abs());
                header.bmiHeader.biBitCount = 32;
                header.bmiHeader.biCompression = BI_RGB.0;
                header.bmiHeader.biHeight = -height;
                let mut pixels = vec![0u8; (width * height * 4) as usize];
                if width > 0
                    && height > 0
                    && GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS)
                        != 0
                {
                    // BGRA to RGBA; a thumbnail without alpha is opaque.
                    let opaque = pixels.chunks(4).all(|p| p[3] == 0);
                    for p in pixels.chunks_mut(4) {
                        p.swap(0, 2);
                        if opaque {
                            p[3] = 255;
                        }
                    }
                    result = Some((width as u32, height as u32, pixels));
                }
            }
            ReleaseDC(None, dc);
            let _ = DeleteObject(bitmap.into());
            result
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn thumbnail(_path: &std::path::Path, _size: u32) -> Option<(u32, u32, Vec<u8>)> {
        None
    }
}
