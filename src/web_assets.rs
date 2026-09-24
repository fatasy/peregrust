//! Project-local browser asset loading and ImageBitmap pixel transfer.
//!
//! Network fetch is deliberately outside this extension's scope. The local
//! fetch bridge returns bytes to JavaScript without serializing them as JSON.

use std::borrow::Cow;
use std::cell::RefCell;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use deno_core::cppgc::Ref;
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use deno_image::bitmap::ImageBitmap;
use image::GenericImageView;
use serde::Serialize;
use tokio::io::AsyncReadExt;

use crate::host::SharedHost;

const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8192;
const MAX_IMAGE_PIXELS: u64 = 67_108_864;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalAssetMetadata {
    size: u64,
}

/// Maps a URL/path to the project without allowing a parent component or an
/// absolute path to escape. Existing symlinks are checked after canonicalize.
fn candidate_path(root: &Path, request: &str) -> Result<PathBuf, JsErrorBox> {
    if request.starts_with("file:") {
        let url = deno_core::ModuleSpecifier::parse(request)
            .map_err(|_| JsErrorBox::type_error("invalid file URL"))?;
        return url
            .to_file_path()
            .map_err(|_| JsErrorBox::type_error("invalid file URL path"));
    }
    if request.contains('\\') || request.starts_with("//") {
        return Err(JsErrorBox::type_error("invalid local asset path"));
    }
    let relative = Path::new(request.strip_prefix('/').unwrap_or(request));
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(JsErrorBox::type_error(
                        "asset path must stay inside the project",
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(JsErrorBox::type_error(
                    "asset path must stay inside the project",
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(JsErrorBox::type_error("asset path must name a file"));
    }
    Ok(root.join(normalized))
}

async fn resolved_asset(root: &Path, request: &str) -> Result<Option<PathBuf>, JsErrorBox> {
    let candidate = candidate_path(root, request)?;
    let path = match tokio::fs::canonicalize(candidate).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(JsErrorBox::generic(format!(
                "cannot resolve local asset: {error}"
            )));
        }
    };
    if !path.starts_with(root) {
        return Err(JsErrorBox::type_error("asset path is outside the project"));
    }
    Ok(Some(path))
}

#[op2]
#[serde]
async fn op_peregrust_asset_stat(
    state: Rc<RefCell<OpState>>,
    #[string] request: String,
) -> Result<Option<LocalAssetMetadata>, JsErrorBox> {
    let root = state.borrow().borrow::<SharedHost>().asset_root();
    let Some(path) = resolved_asset(&root, &request).await? else {
        return Ok(None);
    };
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| JsErrorBox::generic(format!("cannot inspect local asset: {error}")))?;
    if !metadata.is_file() {
        return Ok(None);
    }
    if metadata.len() > MAX_ASSET_BYTES {
        return Err(JsErrorBox::range_error("asset exceeds 128 MiB limit"));
    }
    Ok(Some(LocalAssetMetadata {
        size: metadata.len(),
    }))
}

#[op2]
#[buffer]
async fn op_peregrust_fetch_asset(
    state: Rc<RefCell<OpState>>,
    #[string] request: String,
) -> Result<Vec<u8>, JsErrorBox> {
    let root = state.borrow().borrow::<SharedHost>().asset_root();
    let path = resolved_asset(&root, &request)
        .await?
        .ok_or_else(|| JsErrorBox::generic("asset disappeared during fetch"))?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| JsErrorBox::generic(format!("cannot open local asset: {error}")))?;
    let mut bytes = Vec::new();
    file.take(MAX_ASSET_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| JsErrorBox::generic(format!("cannot read local asset: {error}")))?;
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(JsErrorBox::range_error("asset exceeds 128 MiB limit"));
    }
    Ok(bytes)
}

/// Reject oversized or unsupported encoded images before handing a Blob to
/// deno_image, whose decoder otherwise has no application-specific limits.
#[op2(fast)]
fn op_peregrust_validate_image(
    #[buffer] bytes: &[u8],
    crop_width: u32,
    crop_height: u32,
    resize_width: u32,
    resize_height: u32,
) -> Result<(), JsErrorBox> {
    validate_image_bytes(bytes, crop_width, crop_height, resize_width, resize_height)
}

fn validate_image_bytes(
    bytes: &[u8],
    crop_width: u32,
    crop_height: u32,
    resize_width: u32,
    resize_height: u32,
) -> Result<(), JsErrorBox> {
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(JsErrorBox::range_error("image exceeds 128 MiB limit"));
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| JsErrorBox::type_error(format!("unknown image format: {error}")))?;
    let format = reader.format();
    if !matches!(
        format,
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)
    ) {
        return Err(JsErrorBox::type_error(
            "only PNG, JPEG and WebP ImageBitmap sources are supported",
        ));
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| JsErrorBox::type_error(format!("cannot inspect image: {error}")))?;
    validate_dimensions(width, height)?;
    let surface_width = if crop_width == 0 { width } else { crop_width };
    let surface_height = if crop_height == 0 {
        height
    } else {
        crop_height
    };
    validate_dimensions(surface_width, surface_height)?;
    let output_width = if resize_width != 0 {
        u64::from(resize_width)
    } else if resize_height != 0 {
        (u64::from(surface_width) * u64::from(resize_height)).div_ceil(u64::from(surface_height))
    } else {
        u64::from(surface_width)
    };
    let output_height = if resize_height != 0 {
        u64::from(resize_height)
    } else if resize_width != 0 {
        (u64::from(surface_height) * u64::from(resize_width)).div_ceil(u64::from(surface_width))
    } else {
        u64::from(surface_height)
    };
    let output_width = u32::try_from(output_width)
        .map_err(|_| JsErrorBox::range_error("ImageBitmap resize dimensions exceed limit"))?;
    let output_height = u32::try_from(output_height)
        .map_err(|_| JsErrorBox::range_error("ImageBitmap resize dimensions exceed limit"))?;
    validate_dimensions(output_width, output_height)
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), JsErrorBox> {
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(JsErrorBox::range_error(
            "image dimensions exceed the supported limit",
        ));
    }
    Ok(())
}

/// Crop/flip a real deno_image ImageBitmap and convert all supported source
/// pixel formats into RGBA8 for GPUQueue.writeTexture. This is never a fake
/// ImageBitmap: `Ref<ImageBitmap>` is verified by deno_core's WebIDL converter.
#[op2]
#[buffer]
fn op_peregrust_bitmap_rgba(
    #[webidl] bitmap: Ref<ImageBitmap>,
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
    flip_y: bool,
) -> Result<Vec<u8>, JsErrorBox> {
    if bitmap.detached.get().is_some() {
        return Err(JsErrorBox::type_error("ImageBitmap is closed"));
    }
    let image = bitmap.data.borrow();
    rgba_crop(&image, origin_x, origin_y, width, height, flip_y)
}

fn rgba_crop(
    image: &image::DynamicImage,
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
    flip_y: bool,
) -> Result<Vec<u8>, JsErrorBox> {
    validate_dimensions(width, height)?;
    let (image_width, image_height) = image.dimensions();
    validate_dimensions(image_width, image_height)?;
    if origin_x.checked_add(width).is_none_or(|x| x > image_width)
        || origin_y
            .checked_add(height)
            .is_none_or(|y| y > image_height)
    {
        return Err(JsErrorBox::range_error(
            "copyExternalImageToTexture source rectangle is outside ImageBitmap",
        ));
    }
    let source: Cow<'_, [u8]> = match image {
        image::DynamicImage::ImageRgba8(rgba) => Cow::Borrowed(rgba.as_raw()),
        _ => Cow::Owned(image.to_rgba8().into_raw()),
    };
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| JsErrorBox::range_error("image row is too wide"))?;
    let mut output = vec![0_u8; row_bytes * height as usize];
    for row in 0..height as usize {
        let source_y = origin_y as usize
            + if flip_y {
                height as usize - 1 - row
            } else {
                row
            };
        let start = (source_y * image_width as usize + origin_x as usize) * 4;
        let dest = row * row_bytes;
        output[dest..dest + row_bytes].copy_from_slice(&source[start..start + row_bytes]);
    }
    Ok(output)
}

deno_core::extension!(
    peregrust_web_assets,
    deps = [deno_image, peregrust_host],
    ops = [
        op_peregrust_asset_stat,
        op_peregrust_fetch_asset,
        op_peregrust_validate_image,
        op_peregrust_bitmap_rgba,
    ],
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_and_absolute_paths() {
        let root = Path::new("/game");
        assert!(candidate_path(root, "../secrets").is_err());
        assert!(candidate_path(root, "assets/../../secrets").is_err());
        assert!(candidate_path(root, "C:\\Windows\\win.ini").is_err());
        assert!(candidate_path(root, "//server/share").is_err());
        assert!(candidate_path(root, "/assets/texture.png").is_ok());
        assert_eq!(
            candidate_path(
                root,
                "./assets/runtime/ui/tooltip/../character/traits/a.png"
            )
            .unwrap(),
            root.join("assets/runtime/ui/character/traits/a.png")
        );
    }

    #[test]
    fn rejects_oversized_image_dimensions() {
        assert!(validate_dimensions(0, 1).is_err());
        assert!(validate_dimensions(8193, 1).is_err());
        assert!(validate_dimensions(8192, 8192).is_ok());
    }

    #[test]
    fn validates_real_png_dimensions() {
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(2, 3)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(validate_image_bytes(&png, 0, 0, 0, 0).is_ok());
        assert!(validate_image_bytes(b"not an image", 0, 0, 0, 0).is_err());
        assert!(validate_image_bytes(&png, 0, 0, 8192, 0).is_err());
    }

    #[test]
    fn rgba_upload_pixels_respect_crop_and_flip() {
        let mut pixels = image::RgbaImage::new(2, 2);
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        pixels.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        pixels.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        pixels.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let image = image::DynamicImage::ImageRgba8(pixels);
        let flipped = rgba_crop(&image, 0, 0, 2, 2, true).unwrap();
        assert_eq!(&flipped[0..4], &[0, 0, 255, 255]);
        assert_eq!(&flipped[8..12], &[255, 0, 0, 255]);
        assert_eq!(
            rgba_crop(&image, 1, 0, 1, 1, false).unwrap(),
            [0, 255, 0, 255]
        );
        assert!(rgba_crop(&image, 1, 0, 2, 1, false).is_err());
    }

    #[test]
    fn missing_and_outside_root_are_distinct() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let root = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let canonical_root = std::fs::canonicalize(root.path()).unwrap();
                assert!(
                    resolved_asset(&canonical_root, "missing.png")
                        .await
                        .unwrap()
                        .is_none()
                );
                let nested = root.path().join("assets/runtime/ui/character/traits/a.png");
                std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
                std::fs::write(&nested, b"x").unwrap();
                assert_eq!(
                    resolved_asset(
                        &canonical_root,
                        "./assets/runtime/ui/tooltip/../character/traits/a.png"
                    )
                    .await
                    .unwrap(),
                    Some(std::fs::canonicalize(&nested).unwrap())
                );
                let outside_file = outside.path().join("outside.png");
                std::fs::write(&outside_file, b"x").unwrap();
                let outside_url =
                    deno_core::ModuleSpecifier::from_file_path(&outside_file).unwrap();
                assert!(
                    resolved_asset(&canonical_root, outside_url.as_str())
                        .await
                        .is_err()
                );
            });
    }
}
