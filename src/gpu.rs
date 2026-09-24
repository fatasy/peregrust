//! Presentable WebGPU context backed by deno_webgpu's own wgpu-core instance.
//!
//! This follows Deno's public `ext/canvas/byow.rs` integration. No second
//! wgpu device is created: GPUCanvasContext.configure receives the same JS
//! GPUDevice that draws the frame, and `surface_present` runs on its Instance.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{JsRuntime, OpState, op2, v8};
use deno_error::JsErrorBox;
use deno_webgpu::Instance;
use deno_webgpu::canvas::{self, ContextData, GPUCanvasContext, SurfaceData};
use deno_webgpu::wgpu_core::id::SurfaceId;
use deno_webgpu::wgpu_core::present::SurfaceError;
use deno_webgpu::wgpu_types::SurfaceStatus;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

use crate::host::SharedHost;

/// Create the native surface on winit's main thread. Some platforms only
/// expose raw native handles there, while the JS worker owns the Deno WebGPU
/// instance that must back this exact surface.
pub fn create_surface_on_main(window: &Window, instance: Instance) -> Result<SurfaceId, String> {
    let display = window
        .display_handle()
        .map_err(|error| format!("cannot get display handle: {error}"))?;
    let native_window = window
        .window_handle()
        .map_err(|error| format!("cannot get window handle: {error}"))?;
    // SAFETY: the caller retains its Arc<Window> while the returned surface
    // lives in GpuPresentation. The handles remain valid throughout that time.
    unsafe {
        instance.instance_create_surface(Some(display.as_raw()), native_window.as_raw(), None)
    }
    .map_err(|error| format!("cannot create WebGPU surface: {error}"))
}

pub struct GpuPresentation {
    // Field order deliberately keeps the native window alive until after the
    // canvas context and wgpu surface have dropped.
    context: v8::Global<v8::Value>,
    canvas: v8::Global<v8::Object>,
    surface: Rc<RefCell<SurfaceData>>,
    _window: Arc<Window>,
}

#[op2]
pub fn op_peregrust_get_canvas_context<'s>(
    state: &mut OpState,
    scope: &mut v8::PinScope<'s, '_>,
    canvas: v8::Local<'s, v8::Object>,
) -> Result<v8::Global<v8::Value>, JsErrorBox> {
    if let Some(presentation) = state.try_borrow::<GpuPresentation>() {
        let original_canvas = v8::Local::new(scope, &presentation.canvas);
        if !original_canvas.strict_equals(canvas.into()) {
            return Err(JsErrorBox::type_error(
                "the native window already has a WebGPU canvas",
            ));
        }
        return Ok(presentation.context.clone());
    }

    let host = state.borrow::<SharedHost>().clone();
    let window = host.window();
    let size = host.canvas_size();
    if size.width == 0 || size.height == 0 {
        return Err(JsErrorBox::generic(
            "cannot create a WebGPU canvas while the window has zero area",
        ));
    }

    // Deno's own BYOW surface requests the existing OpState Instance or
    // inserts one. `navigator.gpu.requestAdapter()` will subsequently reuse it.
    let (_, instance) = deno_webgpu::get_or_init_instance(
        state,
        &deno_webgpu::adapter::GPURequestAdapterOptions {
            feature_level: "core".to_string(),
            power_preference: None,
            force_fallback_adapter: false,
        },
    )
    .ok_or_else(|| JsErrorBox::generic("could not initialize WebGPU"))?;

    let surface_id = host.create_surface(instance.clone())?;

    let surface = Rc::new(RefCell::new(SurfaceData {
        width: size.width,
        height: size.height,
        id: surface_id,
        instance: instance.clone(),
    }));
    let options = v8::undefined(scope).into();
    let canvas_global = v8::Global::new(scope, canvas);
    let context = canvas::create(
        Some(instance),
        canvas_global.clone(),
        ContextData::Surface(surface.clone()),
        scope,
        options,
        "Failed to execute 'getContext' on 'PeregrustCanvas'",
        "Argument 2",
    )?;
    state.put(GpuPresentation {
        context: context.clone(),
        canvas: canvas_global,
        surface,
        _window: window,
    });
    Ok(context)
}

#[op2(nofast)]
pub fn op_peregrust_present(
    state: &OpState,
    scope: &mut v8::PinScope<'_, '_>,
) -> Result<bool, JsErrorBox> {
    present_inner(state, scope)
}

fn present_inner(state: &OpState, scope: &mut v8::PinScope<'_, '_>) -> Result<bool, JsErrorBox> {
    let Some(presentation) = state.try_borrow::<GpuPresentation>() else {
        return Ok(false);
    };
    let context = unwrap_context(scope, &presentation.context)?;
    if context.current_texture.borrow().is_none() {
        // A frame callback may update game state without drawing.
        return Ok(false);
    }
    let status = {
        let configuration = context.configuration.borrow();
        let configuration = configuration
            .as_ref()
            .ok_or_else(|| JsErrorBox::type_error("GPUCanvasContext has not been configured"))?;
        let surface_id = presentation.surface.borrow().id;
        configuration.device.instance.surface_present(surface_id)
    };
    // This is the same step as Deno's UnsafeWindowSurface.present: the next
    // getCurrentTexture call must acquire a fresh swapchain texture.
    context.current_texture.borrow_mut().take();
    match status {
        Ok(SurfaceStatus::Good) => Ok(true),
        Ok(SurfaceStatus::Suboptimal) => {
            context.resize(scope);
            Ok(true)
        }
        Ok(SurfaceStatus::Outdated | SurfaceStatus::Lost) => {
            context.resize(scope);
            Ok(false)
        }
        Ok(SurfaceStatus::Timeout | SurfaceStatus::Occluded) => Ok(false),
        Ok(SurfaceStatus::Validation) => Err(JsErrorBox::generic(
            "WebGPU surface failed validation while presenting",
        )),
        Err(
            SurfaceError::Invalid | SurfaceError::NotConfigured | SurfaceError::TextureDestroyed,
        ) => {
            context.resize(scope);
            Ok(false)
        }
        Err(error) => Err(JsErrorBox::generic(format!(
            "WebGPU present failed: {error}"
        ))),
    }
}

fn unwrap_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: &v8::Global<v8::Value>,
) -> Result<deno_core::cppgc::Ref<GPUCanvasContext>, JsErrorBox> {
    let local = v8::Local::new(scope, global);
    deno_core::cppgc::try_unwrap_cppgc_persistent_object::<GPUCanvasContext>(scope, local)
        .ok_or_else(|| JsErrorBox::generic("WebGPU canvas context is unavailable"))
}

/// Called on `WindowEvent::Resized` and after DPI changes, before the next JS
/// frame. Zero-sized windows are suspended; wgpu-core rejects zero-area config.
pub fn resize_surface(runtime: &mut JsRuntime, width: u32, height: u32) -> Result<(), JsErrorBox> {
    let op_state = runtime.op_state();
    let host = op_state.borrow().borrow::<SharedHost>().clone();
    host.set_physical_size(winit::dpi::PhysicalSize::new(width, height));
    if width == 0 || height == 0 {
        return Ok(());
    }
    host.set_canvas_size(width, height);
    deno_core::scope!(scope, runtime);
    resize_canvas(&op_state.borrow(), scope, width, height)
}

pub fn resize_canvas(
    state: &OpState,
    scope: &mut v8::PinScope<'_, '_>,
    width: u32,
    height: u32,
) -> Result<(), JsErrorBox> {
    let presentation = {
        state
            .try_borrow::<GpuPresentation>()
            .map(|gpu| (gpu.surface.clone(), gpu.context.clone()))
    };
    let Some((surface, context)) = presentation else {
        return Ok(());
    };
    {
        let mut data = surface.borrow_mut();
        if data.width == width && data.height == height {
            return Ok(());
        }
        data.width = width;
        data.height = height;
    }
    let context = unwrap_context(scope, &context)?;
    context.resize(scope);
    Ok(())
}

/// Used by the native event loop after all asynchronous JS frame work settles.
pub fn present(runtime: &mut JsRuntime) -> Result<bool, JsErrorBox> {
    let op_state = runtime.op_state();
    deno_core::scope!(scope, runtime);
    present_inner(&op_state.borrow(), scope)
}

/// Release an acquired frame before cppgc destroys the canvas/swapchain.
/// Exit can interrupt an async renderer between acquire and presentation.
pub fn shutdown(runtime: &mut JsRuntime) -> Result<(), JsErrorBox> {
    let op_state = runtime.op_state();
    let presentation = op_state
        .borrow()
        .try_borrow::<GpuPresentation>()
        .map(|gpu| (gpu.context.clone(), gpu.surface.clone()));
    let Some((context, surface)) = presentation else {
        return Ok(());
    };
    deno_core::scope!(scope, runtime);
    let context = unwrap_context(scope, &context)?;
    let surface = surface.borrow();
    // Drain submissions while the window and surface are still alive. This is
    // teardown only; the frame loop must never wait for the entire GPU queue.
    let drain = surface.instance.poll_all_devices(true);
    if context.current_texture.borrow().is_some() {
        match surface.instance.surface_texture_discard(surface.id) {
            Ok(())
            | Err(
                SurfaceError::NotConfigured
                | SurfaceError::AlreadyAcquired
                | SurfaceError::TextureDestroyed,
            ) => {}
            Err(error) => {
                return Err(JsErrorBox::generic(format!(
                    "discarding final WebGPU frame: {error}"
                )));
            }
        }
    }
    context.current_texture.borrow_mut().take();
    context.configuration.borrow_mut().take();
    context.configuration_obj.borrow_mut().take();
    context.texture_descriptor.borrow_mut().take();
    drain.map_err(|error| JsErrorBox::generic(format!("draining WebGPU on shutdown: {error}")))?;
    Ok(())
}

/// Reconfigure after a transient `getCurrentTexture` acquisition failure.
/// The native loop may invoke this before trying the next frame.
pub fn recover_surface(runtime: &mut JsRuntime) -> Result<(), JsErrorBox> {
    let op_state = runtime.op_state();
    let presentation = {
        let state = op_state.borrow();
        state
            .try_borrow::<GpuPresentation>()
            .map(|gpu| gpu.context.clone())
    };
    let Some(context) = presentation else {
        return Ok(());
    };
    deno_core::scope!(scope, runtime);
    let context = unwrap_context(scope, &context)?;
    context.resize(scope);
    Ok(())
}
