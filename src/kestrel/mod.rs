//! Host bridge: cppgc roots, WebGPU error scopes and the Kestrel op ABI.
use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;
use deno_webgpu::{
    device::{GPUDevice, GPUDeviceLostReason},
    error::GPUError,
    texture::GPUTexture,
    wgpu_types as wt,
};
use kestrel_core::{RenderTarget, Renderer};
use std::{collections::HashMap, rc::Rc};

struct HostRenderer {
    renderer: Renderer,
    _device_root: v8::Global<v8::Value>,
}
#[derive(Default)]
struct Registry {
    next: u32,
    renderers: HashMap<u32, HostRenderer>,
}
impl Registry {
    fn renderer_mut(&mut self, id: u32) -> Result<&mut Renderer, JsErrorBox> {
        self.renderers
            .get_mut(&id)
            .map(|r| &mut r.renderer)
            .ok_or_else(invalid)
    }
    fn remove(&mut self, id: u32) -> Result<(), JsErrorBox> {
        self.renderers.remove(&id).map(|_| ()).ok_or_else(invalid)
    }
}
fn invalid() -> JsErrorBox {
    JsErrorBox::type_error("invalid or destroyed Kestrel renderer handle")
}
fn registry(state: &mut OpState) -> Result<&mut Registry, JsErrorBox> {
    state.try_borrow_mut::<Registry>().ok_or_else(invalid)
}
fn core_error(error: kestrel_core::Error) -> JsErrorBox {
    match error.kind() {
        kestrel_core::ErrorKind::Type => JsErrorBox::type_error(error.message().to_owned()),
        kestrel_core::ErrorKind::Range => JsErrorBox::range_error(error.message().to_owned()),
        kestrel_core::ErrorKind::Generic => JsErrorBox::generic(error.message().to_owned()),
    }
}

#[op2(nofast)]
fn op_kestrel_create_renderer(
    state: &mut OpState,
    scope: &mut v8::PinScope<'_, '_>,
    device_value: v8::Local<v8::Value>,
    #[string] format: String,
    #[buffer] settings: &[u32],
) -> Result<u32, JsErrorBox> {
    let device =
        deno_core::cppgc::try_unwrap_cppgc_persistent_object::<GPUDevice>(scope, device_value)
            .ok_or_else(|| JsErrorBox::type_error("device must be a GPUDevice"))?;
    let device_errors = device.error_handler.clone();
    let error_sink: kestrel_core::ErrorHandler = Rc::new(move |kind, message| {
        let error = match kind {
            wt::error::ErrorType::Validation => GPUError::Validation(message),
            wt::error::ErrorType::OutOfMemory => GPUError::OutOfMemory,
            wt::error::ErrorType::Internal => GPUError::Internal,
            wt::error::ErrorType::DeviceLost => GPUError::Lost(GPUDeviceLostReason::Unknown),
        };
        device_errors.push_error(Some(error));
    });
    let renderer = Renderer::new(
        device.instance.clone(),
        device.id,
        device.queue,
        error_sink,
        &format,
        settings,
    )
    .map_err(core_error)?;
    if state.try_borrow_mut::<Registry>().is_none() {
        state.put(Registry::default());
    }
    let registry = state.borrow_mut::<Registry>();
    let id = registry
        .next
        .checked_add(1)
        .ok_or_else(|| JsErrorBox::range_error("Kestrel renderer handle space exhausted"))?;
    registry.next = id;
    registry.renderers.insert(
        id,
        HostRenderer {
            renderer,
            _device_root: v8::Global::new(scope, device_value),
        },
    );
    Ok(id)
}

#[op2(fast)]
fn op_kestrel_create_geometry(
    state: &mut OpState,
    renderer: u32,
    #[buffer] vertices: &[f32],
    #[buffer] indices: &[u32],
    #[buffer] bounds: &[f32],
) -> Result<u32, JsErrorBox> {
    if bounds.len() != 6 {
        return Err(JsErrorBox::type_error(
            "bounds must contain six Float32 values",
        ));
    }
    registry(state)?
        .renderer_mut(renderer)?
        .create_geometry(vertices, indices, bounds)
        .map_err(core_error)
}
#[op2(fast)]
fn op_kestrel_create_shader_geometry(
    state: &mut OpState,
    renderer: u32,
    program: u32,
    #[buffer] vertices: &[f32],
    #[buffer] indices: &[u32],
    #[buffer] bounds: &[f32],
) -> Result<u32, JsErrorBox> {
    if bounds.len() != 6 {
        return Err(JsErrorBox::type_error(
            "bounds must contain six Float32 values",
        ));
    }
    registry(state)?
        .renderer_mut(renderer)?
        .create_shader_geometry(program, vertices, indices, bounds)
        .map_err(core_error)
}
#[op2(fast)]
fn op_kestrel_create_material(
    state: &mut OpState,
    renderer: u32,
    #[buffer] params: &[f32],
    #[buffer] textures: &[u32],
) -> Result<u32, JsErrorBox> {
    registry(state)?
        .renderer_mut(renderer)?
        .create_material(params, textures)
        .map_err(core_error)
}
#[op2(nofast)]
fn op_kestrel_create_shader_program(
    state: &mut OpState,
    renderer: u32,
    #[string] descriptor: String,
    #[string] vertex_wgsl: String,
    #[string] fragment_wgsl: String,
) -> Result<u32, JsErrorBox> {
    registry(state)?
        .renderer_mut(renderer)?
        .create_shader_program(&descriptor, &vertex_wgsl, &fragment_wgsl)
        .map_err(core_error)
}
#[op2(fast)]
fn op_kestrel_create_shader_material(
    state: &mut OpState,
    renderer: u32,
    program: u32,
    #[buffer] params: &[f32],
    #[buffer] textures: &[u32],
) -> Result<u32, JsErrorBox> {
    registry(state)?
        .renderer_mut(renderer)?
        .create_shader_material(program, params, textures)
        .map_err(core_error)
}
#[op2(fast)]
fn op_kestrel_create_texture(
    state: &mut OpState,
    renderer: u32,
    #[buffer] descriptor: &[u32],
    #[buffer] data: &[u8],
    #[buffer] info: &mut [u32],
) -> Result<u32, JsErrorBox> {
    if info.len() < 3 {
        return Err(JsErrorBox::type_error(
            "texture info needs at least three Uint32 values",
        ));
    }
    let (id, dimensions) = registry(state)?
        .renderer_mut(renderer)?
        .create_texture(descriptor, data)
        .map_err(core_error)?;
    info[..3].copy_from_slice(&dimensions);
    Ok(id)
}
#[op2(fast)]
fn op_kestrel_create_compressed_texture_array(
    state: &mut OpState,
    renderer: u32,
    #[buffer] descriptor: &[u32],
    #[buffer] data: &[u8],
    #[buffer] info: &mut [u32],
) -> Result<u32, JsErrorBox> {
    if info.len() < 3 {
        return Err(JsErrorBox::type_error(
            "compressed array info needs at least three Uint32 values",
        ));
    }
    let (id, dimensions) = registry(state)?
        .renderer_mut(renderer)?
        .create_compressed_texture_array(descriptor, data)
        .map_err(core_error)?;
    info[..3].copy_from_slice(&dimensions);
    Ok(id)
}
#[op2(fast)]
fn op_kestrel_create_mesh(
    state: &mut OpState,
    renderer: u32,
    geometry: u32,
    material: u32,
    #[buffer] matrices: &[f32],
    #[buffer] colors: &[f32],
) -> Result<u32, JsErrorBox> {
    registry(state)?
        .renderer_mut(renderer)?
        .create_mesh(geometry, material, matrices, colors)
        .map_err(core_error)
}
#[op2(fast)]
fn op_kestrel_render(
    state: &mut OpState,
    renderer: u32,
    #[cppgc] target: &GPUTexture,
    #[buffer] frame: &[f32],
    #[buffer] dirty_pairs: &[u32],
    #[buffer] dirty_matrices: &[f32],
    #[buffer] stats: &mut [u32],
) -> Result<(), JsErrorBox> {
    if stats.len() < 4 {
        return Err(JsErrorBox::type_error(
            "stats must be a Uint32Array of at least four elements",
        ));
    }
    let target = RenderTarget {
        instance: target.instance.clone(),
        id: target.id,
        device_id: target.device_id,
        format: target.format.clone().into(),
        width: target.size.width,
        height: target.size.height,
    };
    let result = registry(state)?
        .renderer_mut(renderer)?
        .render(&target, frame, dirty_pairs, dirty_matrices)
        .map_err(core_error)?;
    stats[..4].copy_from_slice(&result);
    Ok(())
}
#[op2(fast)]
fn op_kestrel_destroy(
    state: &mut OpState,
    renderer: u32,
    kind: u32,
    id: u32,
) -> Result<(), JsErrorBox> {
    let registry = registry(state)?;
    if kind == 0 {
        if id != 0 {
            return Err(JsErrorBox::type_error("renderer destroy id must be zero"));
        }
        registry.remove(renderer)
    } else {
        registry
            .renderer_mut(renderer)?
            .destroy(kind, id)
            .map_err(core_error)
    }
}

deno_core::extension!(
    peregrust_kestrel,
    ops = [
        op_kestrel_create_renderer,
        op_kestrel_create_geometry,
        op_kestrel_create_shader_geometry,
        op_kestrel_create_material,
        op_kestrel_create_shader_program,
        op_kestrel_create_shader_material,
        op_kestrel_create_texture,
        op_kestrel_create_compressed_texture_array,
        op_kestrel_create_mesh,
        op_kestrel_render,
        op_kestrel_destroy,
    ]
);
