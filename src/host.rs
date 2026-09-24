//! Native window state and the JavaScript-facing host services.
//!
//! The winit window is retained by both `SharedHost` and the GPU presentation
//! state. A wgpu surface must never outlive the native handles it was made from.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Waker;

use deno_core::{JsBuffer, OpState, op2, v8};
use deno_error::JsErrorBox;
use deno_webgpu::Instance;
use deno_webgpu::wgpu_core::id::SurfaceId;
use serde::Serialize;
use tokio::io::AsyncReadExt;
use winit::dpi::PhysicalSize;
use winit::window::{CursorGrabMode, Fullscreen, Window};

use crate::gpu;

const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8192;
const MAX_IMAGE_PIXELS: u64 = 67_108_864;
static IMAGE_DECODE_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Clone)]
pub struct SharedHost(Arc<Mutex<NativeHost>>);

pub type SurfaceFactory = dyn Fn(Instance) -> Result<SurfaceId, JsErrorBox> + Send + Sync + 'static;

struct NativeHost {
    window: Arc<Window>,
    asset_root: PathBuf,
    physical_size: PhysicalSize<u32>,
    canvas_size: PhysicalSize<u32>,
    scale_factor: f64,
    focused: bool,
    visible: bool,
    fullscreen: bool,
    redraw_requested: bool,
    exit_code: Option<i32>,
    title: String,
    args: Vec<String>,
    cursor_visible: bool,
    pointer_capture: String,
    surface_factory: Option<Arc<SurfaceFactory>>,
    waker: Option<Waker>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    pub device_pixel_ratio: f64,
    pub focused: bool,
    pub visible: bool,
    pub title: String,
    pub args: Vec<String>,
    pub fullscreen: bool,
    pub cursor_visible: bool,
    pub pointer_capture: String,
}

impl SharedHost {
    /// `asset_root` must exist. It is canonicalized once so relative asset
    /// requests can be confined to the project even in the presence of links.
    pub fn new(window: Arc<Window>, asset_root: impl AsRef<Path>) -> std::io::Result<Self> {
        let asset_root = std::fs::canonicalize(asset_root)?;
        if !asset_root.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "asset root is not a directory",
            ));
        }
        let physical_size = window.inner_size();
        let scale_factor = window.scale_factor();
        Ok(Self(Arc::new(Mutex::new(NativeHost {
            window,
            asset_root,
            physical_size,
            canvas_size: physical_size,
            scale_factor,
            focused: true,
            visible: true,
            fullscreen: false,
            redraw_requested: true,
            exit_code: None,
            title: "Peregrust".to_string(),
            args: Vec::new(),
            cursor_visible: true,
            pointer_capture: "none".to_string(),
            surface_factory: None,
            waker: None,
        }))))
    }

    fn lock(&self) -> MutexGuard<'_, NativeHost> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn window(&self) -> Arc<Window> {
        self.lock().window.clone()
    }

    /// The callback sends a surface request to winit's main thread. It must
    /// not capture this host or the window: the presentation state already
    /// retains the window and a capture would form a reference cycle.
    pub fn set_surface_factory(&self, factory: Arc<SurfaceFactory>) {
        self.lock().surface_factory = Some(factory);
    }

    pub fn create_surface(&self, instance: Instance) -> Result<SurfaceId, JsErrorBox> {
        let factory = self
            .lock()
            .surface_factory
            .clone()
            .ok_or_else(|| JsErrorBox::generic("native surface factory is unavailable"))?;
        factory(instance)
    }

    pub fn set_waker(&self, waker: Waker) {
        let should_wake = {
            let mut host = self.lock();
            let pending = host.redraw_requested || host.exit_code.is_some();
            host.waker = Some(waker.clone());
            pending
        };
        if should_wake {
            waker.wake_by_ref();
        }
    }

    pub fn physical_size(&self) -> PhysicalSize<u32> {
        self.lock().physical_size
    }

    pub fn canvas_size(&self) -> PhysicalSize<u32> {
        self.lock().canvas_size
    }

    pub fn set_canvas_size(&self, width: u32, height: u32) {
        self.lock().canvas_size = PhysicalSize::new(width, height);
    }

    pub fn scale_factor(&self) -> f64 {
        self.lock().scale_factor
    }

    pub fn set_physical_size(&self, size: PhysicalSize<u32>) {
        self.lock().physical_size = size;
    }

    pub fn set_scale_factor(&self, factor: f64) {
        if factor.is_finite() && factor > 0.0 {
            self.lock().scale_factor = factor;
        }
    }

    pub fn set_focused(&self, focused: bool) {
        let window = {
            let mut host = self.lock();
            host.focused = focused;
            if !focused && host.pointer_capture != "none" {
                host.pointer_capture = "none".to_string();
                Some(host.window.clone())
            } else {
                None
            }
        };
        if let Some(window) = window
            && let Err(error) = window.set_cursor_grab(CursorGrabMode::None)
        {
            tracing::warn!(%error, "could not release pointer capture after focus loss");
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.lock().visible = visible;
    }

    pub fn set_title(&self, title: impl Into<String>) {
        let title = title.into();
        let window = {
            let mut host = self.lock();
            host.title = title.clone();
            host.window.clone()
        };
        window.set_title(&title);
    }

    pub fn set_args(&self, args: Vec<String>) {
        self.lock().args = args;
    }

    pub fn focus_window(&self) -> bool {
        let window = self.window();
        window.focus_window();
        self.lock().focused
    }

    pub fn set_fullscreen(&self, fullscreen: bool) -> bool {
        let window = self.window();
        window.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
        self.lock().fullscreen = fullscreen;
        fullscreen
    }

    pub fn set_cursor_visible(&self, visible: bool) {
        self.window().set_cursor_visible(visible);
        self.lock().cursor_visible = visible;
    }

    /// Returns the mode actually acquired. Winit's lock/confine support varies
    /// by OS, so try the requested mode first and then its useful fallback.
    pub fn set_pointer_capture(&self, requested: &str) -> Result<String, JsErrorBox> {
        let window = self.window();
        let (primary, fallback) = match requested {
            "none" => (CursorGrabMode::None, None),
            "confined" => (CursorGrabMode::Confined, Some(CursorGrabMode::Locked)),
            "locked" => (CursorGrabMode::Locked, Some(CursorGrabMode::Confined)),
            _ => {
                return Err(JsErrorBox::type_error(
                    "pointer capture must be 'none', 'confined', or 'locked'",
                ));
            }
        };
        let acquired = match window.set_cursor_grab(primary) {
            Ok(()) => primary,
            Err(primary_error) => {
                let Some(fallback) = fallback else {
                    return Err(JsErrorBox::generic(format!(
                        "cannot release pointer capture: {primary_error}"
                    )));
                };
                window.set_cursor_grab(fallback).map_err(|fallback_error| {
                    JsErrorBox::generic(format!(
                        "cannot capture pointer: {primary_error}; fallback failed: {fallback_error}"
                    ))
                })?;
                fallback
            }
        };
        let mode = match acquired {
            CursorGrabMode::None => "none",
            CursorGrabMode::Confined => "confined",
            CursorGrabMode::Locked => "locked",
        };
        self.lock().pointer_capture = mode.to_string();
        Ok(mode.to_string())
    }

    pub fn request_redraw(&self) {
        let (window, waker) = {
            let mut host = self.lock();
            host.redraw_requested = true;
            (host.window.clone(), host.waker.clone())
        };
        window.request_redraw();
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
    }

    pub fn take_redraw_requested(&self) -> bool {
        std::mem::take(&mut self.lock().redraw_requested)
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.lock().exit_code
    }

    pub fn request_exit(&self, code: i32) {
        let waker = {
            let mut host = self.lock();
            host.exit_code = Some(code);
            host.waker.clone()
        };
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
    }

    pub fn state(&self) -> WindowState {
        let host = self.lock();
        WindowState {
            width: host.physical_size.width,
            height: host.physical_size.height,
            device_pixel_ratio: host.scale_factor,
            focused: host.focused,
            visible: host.visible,
            title: host.title.clone(),
            args: host.args.clone(),
            fullscreen: host.fullscreen,
            cursor_visible: host.cursor_visible,
            pointer_capture: host.pointer_capture.clone(),
        }
    }

    pub(crate) fn asset_root(&self) -> PathBuf {
        self.lock().asset_root.clone()
    }
}

#[op2]
#[serde]
fn op_peregrust_get_window_state(state: &OpState) -> WindowState {
    state.borrow::<SharedHost>().state()
}

#[op2(fast)]
fn op_peregrust_set_title(state: &OpState, #[string] title: String) {
    state.borrow::<SharedHost>().set_title(title);
}

#[op2(fast)]
fn op_peregrust_request_redraw(state: &OpState) {
    state.borrow::<SharedHost>().request_redraw();
}

#[op2(fast)]
fn op_peregrust_exit(state: &OpState, code: i32) {
    state.borrow::<SharedHost>().request_exit(code);
}

#[op2(fast)]
fn op_peregrust_focus_window(state: &OpState) -> bool {
    state.borrow::<SharedHost>().focus_window()
}

#[op2(fast)]
fn op_peregrust_set_fullscreen(state: &OpState, fullscreen: bool) -> bool {
    state.borrow::<SharedHost>().set_fullscreen(fullscreen)
}

#[op2(fast)]
fn op_peregrust_set_cursor_visible(state: &OpState, visible: bool) {
    state.borrow::<SharedHost>().set_cursor_visible(visible);
}

#[op2]
#[string]
fn op_peregrust_set_pointer_capture(
    state: &OpState,
    #[string] mode: String,
) -> Result<String, JsErrorBox> {
    state.borrow::<SharedHost>().set_pointer_capture(&mode)
}

/// The drawing buffer can differ from the window size (Three.js render scale).
/// Only the GPU surface is reconfigured; the native window stays the same size.
#[op2(nofast)]
fn op_peregrust_set_canvas_size(
    state: &OpState,
    scope: &mut v8::PinScope<'_, '_>,
    width: u32,
    height: u32,
) -> Result<(), JsErrorBox> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(JsErrorBox::range_error(
            "canvas dimensions must be between 1 and 16384",
        ));
    }
    let host = state.borrow::<SharedHost>();
    host.set_canvas_size(width, height);
    gpu::resize_canvas(state, scope, width, height)?;
    host.request_redraw();
    Ok(())
}

async fn resolve_asset(root: PathBuf, path: String) -> Result<PathBuf, JsErrorBox> {
    let relative = Path::new(&path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(JsErrorBox::type_error(
            "asset path must stay inside the project",
        ));
    }
    let resolved = tokio::fs::canonicalize(root.join(relative))
        .await
        .map_err(|e| JsErrorBox::generic(format!("cannot resolve asset '{path}': {e}")))?;
    if !resolved.starts_with(&root) {
        return Err(JsErrorBox::type_error("asset path is outside the project"));
    }
    Ok(resolved)
}

pub(crate) async fn read_asset(root: PathBuf, path: String) -> Result<Vec<u8>, JsErrorBox> {
    read_asset_limited(root, path, MAX_ASSET_BYTES).await
}

pub(crate) async fn read_asset_limited(
    root: PathBuf,
    path: String,
    limit: u64,
) -> Result<Vec<u8>, JsErrorBox> {
    let resolved = resolve_asset(root, path).await?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(|e| JsErrorBox::generic(format!("cannot inspect asset: {e}")))?;
    if !metadata.is_file() {
        return Err(JsErrorBox::type_error("asset path is not a file"));
    }
    if metadata.len() > limit {
        return Err(JsErrorBox::range_error(format!(
            "asset exceeds {} MiB limit",
            limit / (1024 * 1024)
        )));
    }
    let file = tokio::fs::File::open(resolved)
        .await
        .map_err(|e| JsErrorBox::generic(format!("cannot open asset: {e}")))?;
    let mut bounded = file.take(limit + 1);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    bounded
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| JsErrorBox::generic(format!("cannot read asset: {e}")))?;
    if bytes.len() as u64 > limit {
        return Err(JsErrorBox::range_error(format!(
            "asset exceeds {} MiB limit",
            limit / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

/// Returns a Uint8Array. JS loaders may pass its underlying ArrayBuffer to
/// Three.js GLTFLoader.parseAsync or to other binary parsers.
#[op2]
#[buffer]
async fn op_peregrust_read_asset(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<Vec<u8>, JsErrorBox> {
    let root = state.borrow().borrow::<SharedHost>().asset_root();
    read_asset(root, path).await
}

/// Returns `[width:u32le, height:u32le, rgba8 pixels...]` as one Uint8Array.
/// Header and pixels share one allocation, avoiding a per-byte serde walk.
#[op2]
#[buffer]
async fn op_peregrust_decode_image(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<Vec<u8>, JsErrorBox> {
    let root = state.borrow().borrow::<SharedHost>().asset_root();
    let bytes = read_asset(root, path).await?;
    let permit = IMAGE_DECODE_LIMIT
        .acquire()
        .await
        .map_err(|error| JsErrorBox::generic(format!("image decoder unavailable: {error}")))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        decode_image(bytes)
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("image decoder task failed: {e}")))?
}

/// Decode an in-memory image (for example an embedded GLB bufferView) using
/// the same bounded RGBA path as a project file.
#[op2]
#[buffer]
async fn op_peregrust_decode_image_bytes(#[buffer] bytes: JsBuffer) -> Result<Vec<u8>, JsErrorBox> {
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(JsErrorBox::range_error("image exceeds 128 MiB limit"));
    }
    let permit = IMAGE_DECODE_LIMIT
        .acquire()
        .await
        .map_err(|error| JsErrorBox::generic(format!("image decoder unavailable: {error}")))?;
    let owned = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        decode_image(owned)
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("image decoder task failed: {e}")))?
}

fn decode_image(bytes: Vec<u8>) -> Result<Vec<u8>, JsErrorBox> {
    use image::GenericImageView;
    use std::io::Cursor;

    let probe = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| JsErrorBox::generic(format!("cannot identify image: {e}")))?;
    let (width, height) = probe
        .into_dimensions()
        .map_err(|e| JsErrorBox::generic(format!("cannot inspect image: {e}")))?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || (width as u64) * (height as u64) > MAX_IMAGE_PIXELS
    {
        return Err(JsErrorBox::range_error(
            "image dimensions exceed the supported limit",
        ));
    }

    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| JsErrorBox::generic(format!("cannot identify image: {e}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| JsErrorBox::generic(format!("cannot decode image: {e}")))?;
    let (actual_width, actual_height) = image.dimensions();
    if actual_width != width || actual_height != height {
        return Err(JsErrorBox::generic(
            "image dimensions changed while decoding",
        ));
    }
    let pixels = image.into_rgba8().into_raw();
    let mut output = Vec::with_capacity(8 + pixels.len());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    output.extend_from_slice(&pixels);
    Ok(output)
}

deno_core::extension!(
    peregrust_host,
    ops = [
        op_peregrust_get_window_state,
        op_peregrust_set_title,
        op_peregrust_request_redraw,
        op_peregrust_exit,
        op_peregrust_focus_window,
        op_peregrust_set_fullscreen,
        op_peregrust_set_cursor_visible,
        op_peregrust_set_pointer_capture,
        op_peregrust_set_canvas_size,
        gpu::op_peregrust_get_canvas_context,
        gpu::op_peregrust_present,
        op_peregrust_read_asset,
        op_peregrust_decode_image,
        op_peregrust_decode_image_bytes,
    ],
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_host_can_cross_worker_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedHost>();
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn asset_paths_stay_within_project() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("inside.bin"), b"inside").unwrap();
        std::fs::write(outside.path().join("outside.bin"), b"outside").unwrap();
        let root = std::fs::canonicalize(project.path()).unwrap();

        let bytes = run(read_asset(root.clone(), "inside.bin".into())).unwrap();
        assert_eq!(bytes, b"inside");
        assert!(run(read_asset(root.clone(), "../outside.bin".into())).is_err());
        #[cfg(windows)]
        assert!(run(read_asset(root.clone(), "C:inside.bin".into())).is_err());
        assert!(
            run(read_asset(
                root.clone(),
                outside.path().join("outside.bin").display().to_string()
            ))
            .is_err()
        );

        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(
            outside.path().join("outside.bin"),
            project.path().join("link.bin"),
        );
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(
            outside.path().join("outside.bin"),
            project.path().join("link.bin"),
        );
        if link_result.is_ok() {
            assert!(run(read_asset(root, "link.bin".into())).is_err());
        }
    }

    #[test]
    fn oversized_asset_is_rejected_before_allocating() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("huge.bin");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ASSET_BYTES + 1)
            .unwrap();
        let root = std::fs::canonicalize(project.path()).unwrap();
        assert!(run(read_asset(root, "huge.bin".into())).is_err());
    }

    #[test]
    fn image_decode_rejects_corruption_and_packs_rgba() {
        assert!(decode_image(b"not an image".to_vec()).is_err());

        let image = image::RgbaImage::from_pixel(2, 1, image::Rgba([7, 11, 13, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_image(encoded.into_inner()).unwrap();
        assert_eq!(&decoded[..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&decoded[8..], &[7, 11, 13, 255, 7, 11, 13, 255]);
    }
}
