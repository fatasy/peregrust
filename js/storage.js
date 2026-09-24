// Evaluated after js/bootstrap.js and before the game entry module.
// Every operation below reaches the native durable store; no memory fallback.
((root) => {
  const ops = root.Deno?.core?.ops;
  const host = root.Peregrust;
  if (!host || !ops?.op_peregrust_storage_text_get || !ops?.op_peregrust_storage_binary_get) {
    throw new Error('Peregrust durable storage extension is unavailable');
  }

  const localStorage = Object.freeze({
    get length() { return ops.op_peregrust_storage_text_length(); },
    key(index) {
      const number = Number(index);
      return Number.isInteger(number) && number >= 0 && number <= 0xffff_ffff
        ? ops.op_peregrust_storage_text_key(number) : null;
    },
    getItem(key) { return ops.op_peregrust_storage_text_get(String(key)); },
    setItem(key, value) { ops.op_peregrust_storage_text_set(String(key), String(value)); },
    removeItem(key) { ops.op_peregrust_storage_text_remove(String(key)); },
    clear() { ops.op_peregrust_storage_text_clear(); },
  });

  const storage = Object.freeze({
    has(key) { return ops.op_peregrust_storage_binary_has(String(key), false); },
    get(key) {
      const name = String(key);
      if (!ops.op_peregrust_storage_binary_has(name, false)) return null;
      const bytes = ops.op_peregrust_storage_binary_get(name, false);
      return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    },
    getBackup(key) {
      const name = String(key);
      if (!ops.op_peregrust_storage_binary_has(name, true)) return null;
      const bytes = ops.op_peregrust_storage_binary_get(name, true);
      return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    },
    set(key, value) {
      if (!(value instanceof ArrayBuffer) && !ArrayBuffer.isView(value)) {
        throw new TypeError('Peregrust.storage.set requires an ArrayBuffer or ArrayBufferView');
      }
      const bytes = value instanceof ArrayBuffer
        ? new Uint8Array(value)
        : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
      ops.op_peregrust_storage_binary_set(String(key), bytes);
    },
    remove(key) { ops.op_peregrust_storage_binary_remove(String(key)); },
  });

  Object.defineProperty(root, 'localStorage', {
    configurable: false, enumerable: true, value: localStorage,
  });
  Object.defineProperty(host, 'storage', {
    configurable: false, enumerable: true, value: storage,
  });
})(globalThis);
