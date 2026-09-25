//! Batched uploads through the same device/queue as deno_webgpu.

use std::sync::Arc;

use deno_core::{op2, v8};
use deno_error::JsErrorBox;
use deno_webgpu::{buffer::GPUBuffer, device::GPUDevice, error::GPUError};

fn operation_error(message: &'static str) -> JsErrorBox {
    // The JS boundary turns range failures into a DOMException OperationError.
    // Use a built-in exception here: the standalone host has no Deno CLI
    // registry entry for the DOMExceptionOperationError class.
    JsErrorBox::range_error(message)
}

fn integer(value: v8::Local<v8::Value>, default: Option<u64>) -> Result<u64, JsErrorBox> {
    if value.is_undefined() {
        return default.ok_or_else(|| JsErrorBox::type_error("missing upload offset"));
    }
    let number = value
        .try_cast::<v8::Number>()
        .map_err(|_| JsErrorBox::type_error("upload offsets and sizes must be numbers"))?
        .value();
    if !number.is_finite()
        || number < 0.0
        || number.fract() != 0.0
        || number > 9_007_199_254_740_991.0
    {
        return Err(JsErrorBox::type_error(
            "upload offsets and sizes must be nonnegative safe integers",
        ));
    }
    Ok(number as u64)
}

/// Flat groups of [GPUBuffer, bufferOffset, source, dataOffset, size]. Source
/// offsets/sizes are elements for TypedArrays, bytes for ArrayBuffer/DataView.
/// Each write snapshots its source synchronously before the next is processed.
#[op2(nofast)]
pub fn op_peregrust_write_buffer_batch<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[cppgc] device: &GPUDevice,
    operations: v8::Local<'s, v8::Array>,
) -> Result<(), JsErrorBox> {
    if !operations.length().is_multiple_of(5) {
        return Err(JsErrorBox::type_error(
            "writeBufferBatch expects groups of five values",
        ));
    }
    for index in (0..operations.length()).step_by(5) {
        // Get all entries before borrowing source bytes: array getters may run JS.
        let mut values = [v8::undefined(scope).into(); 5];
        for (offset, value) in values.iter_mut().enumerate() {
            *value = operations
                .get_index(scope, index + offset as u32)
                .ok_or_else(|| JsErrorBox::type_error("cannot read upload operation"))?;
        }
        let buffer =
            deno_core::cppgc::try_unwrap_cppgc_persistent_object::<GPUBuffer>(scope, values[0])
                .ok_or_else(|| JsErrorBox::type_error("upload destination must be a GPUBuffer"))?;
        let destination = integer(values[1], None)?;
        let source_offset = integer(values[3], Some(0))?;
        let requested_size = if values[4].is_undefined() {
            None
        } else {
            Some(integer(values[4], None)?)
        };
        let source = values[2];
        let (backing, view_offset, view_length, unit) =
            if let Ok(view) = source.try_cast::<v8::ArrayBufferView>() {
                let array = view
                    .buffer(scope)
                    .ok_or_else(|| operation_error("source buffer is unavailable"))?;
                if array.was_detached() {
                    return Err(operation_error("source buffer is detached"));
                }
                let unit = source
                    .try_cast::<v8::TypedArray>()
                    .ok()
                    .map(|typed| typed.byte_length().checked_div(typed.length()).unwrap_or(1))
                    .unwrap_or(1);
                (
                    array.get_backing_store(),
                    view.byte_offset(),
                    view.byte_length(),
                    unit,
                )
            } else if let Ok(array) = source.try_cast::<v8::ArrayBuffer>() {
                if array.was_detached() {
                    return Err(operation_error("source buffer is detached"));
                }
                (array.get_backing_store(), 0, array.byte_length(), 1)
            } else {
                return Err(JsErrorBox::type_error(
                    "upload source must be an ArrayBuffer or ArrayBufferView",
                ));
            };
        // Concurrent mutation of shared backing stores needs atomic copying;
        // this extension deliberately accepts only non-shared sources.
        if backing.is_shared() {
            return Err(JsErrorBox::type_error(
                "shared upload sources are unsupported",
            ));
        }
        let offset = usize::try_from(source_offset)
            .ok()
            .and_then(|n| n.checked_mul(unit))
            .filter(|&n| n <= view_length)
            .ok_or_else(|| operation_error("source offset is out of bounds"))?;
        let length = match requested_size {
            Some(n) => usize::try_from(n)
                .ok()
                .and_then(|n| n.checked_mul(unit))
                .filter(|&n| n <= view_length - offset)
                .ok_or_else(|| operation_error("source size is out of bounds"))?,
            None => view_length - offset,
        };
        if !length.is_multiple_of(4) {
            return Err(operation_error(
                "upload byte length must be a multiple of four",
            ));
        }
        let start = view_offset
            .checked_add(offset)
            .ok_or_else(|| operation_error("source offset overflow"))?;
        if start
            .checked_add(length)
            .is_none_or(|end| end > backing.byte_length())
        {
            return Err(operation_error("source range is out of bounds"));
        }
        if !Arc::ptr_eq(&device.instance, &buffer.instance) || device.id != buffer.device {
            device.error_handler.push_error(Some(GPUError::Validation(
                "upload buffer belongs to another device".into(),
            )));
            continue;
        }
        let data = if length == 0 {
            &[][..]
        } else {
            let ptr = backing
                .data()
                .ok_or_else(|| operation_error("source backing store is unavailable"))?;
            // SAFETY: non-shared backing is retained above, checked range is in
            // bounds, and no V8/JS operation occurs while this slice is borrowed.
            unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast::<u8>().add(start), length) }
        };
        let error = device
            .instance
            .queue_write_buffer(device.queue, buffer.id, destination, data)
            .err();
        device.error_handler.push_error(error);
    }
    Ok(())
}

deno_core::extension!(
    peregrust_gpu_upload,
    ops = [op_peregrust_write_buffer_batch]
);
