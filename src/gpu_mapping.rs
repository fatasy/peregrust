//! `getMappedRange` for buffers still mapped from creation.
//!
//! deno_webgpu hands JavaScript a V8-owned copy of the mapped range and writes
//! it back on `unmap()`. It fills that copy by reading the mapping, which for a
//! `mappedAtCreation` buffer is usually an upload heap in write-combined memory:
//! reads there run near 50 MB/s, so a 5 MB vertex buffer cost ~100 ms of the
//! frame that first drew it. WebGPU defines those contents as zero, so this op
//! starts the copy zeroed and keeps deno_webgpu's writeback and detach paths.

use deno_core::{op2, v8};
use deno_error::JsErrorBox;
use deno_webgpu::buffer::{GPUBuffer, MappedJsBuffer};

fn integer(value: v8::Local<v8::Value>, name: &str) -> Result<Option<u64>, JsErrorBox> {
    if value.is_undefined() {
        return Ok(None);
    }
    let number = value
        .try_cast::<v8::Number>()
        .map_err(|_| JsErrorBox::type_error(format!("{name} must be a number")))?
        .value();
    if !number.is_finite() || number < 0.0 || number > 9_007_199_254_740_991.0 {
        return Err(JsErrorBox::type_error(format!("{name} is out of range")));
    }
    // WebIDL [EnforceRange] truncates toward zero.
    Ok(Some(number.trunc() as u64))
}

/// Only call this while the buffer is still in its creation mapping: the range
/// is returned zeroed without reading the mapped memory.
#[op2]
pub fn op_peregrust_buffer_zeroed_mapped_range<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[cppgc] buffer: &GPUBuffer,
    offset: v8::Local<'s, v8::Value>,
    size: v8::Local<'s, v8::Value>,
) -> Result<v8::Local<'s, v8::ArrayBuffer>, JsErrorBox> {
    let offset = integer(offset, "offset")?.unwrap_or(0);
    let size = integer(size, "size")?;
    // Validates the range against the live mapping; the pointer is not read.
    let (_, range_size) = buffer
        .instance
        .buffer_get_mapped_range(buffer.id, offset, size)
        .map_err(|error| JsErrorBox::range_error(error.to_string()))?;
    let length = usize::try_from(range_size)
        .map_err(|_| JsErrorBox::range_error("mapped range does not fit in memory"))?;
    let store = v8::ArrayBuffer::new_backing_store_from_vec(vec![0u8; length]).make_shared();
    let array = v8::ArrayBuffer::with_backing_store(scope, &store);
    buffer.mapped_js_buffers.borrow_mut().push(MappedJsBuffer {
        buffer: v8::Global::new(scope, array),
        offset,
        size: range_size,
        copy_on_unmap: true,
    });
    Ok(array)
}

deno_core::extension!(
    peregrust_gpu_mapping,
    ops = [op_peregrust_buffer_zeroed_mapped_range]
);
