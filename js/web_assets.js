// Installed after the deno_web, deno_webgpu, deno_image and Peregrust bootstraps.
// This provides local-file fetch and the missing WebGPU external-image transfer.
((root) => {
  const core = Deno.core;
  const ops = core.ops;
  const streams = core.loadExtScript('ext:deno_web/06_streams.js');
  const events = core.loadExtScript('ext:deno_web/02_event.js');
  const { blobFromObjectUrl } = core.loadExtScript('ext:deno_web/09_file.js');
  const image = core.createLazyLoader('ext:deno_image/01_image.js')();
  const MAX_DATA_BYTES = 128 * 1024 * 1024;

  root.ReadableStream = streams.ReadableStream;
  root.ProgressEvent = events.ProgressEvent;
  root.ImageBitmap = image.ImageBitmap;

  const nativeCreateImageBitmap = image.createImageBitmap;
  root.createImageBitmap = async function createImageBitmap(source, ...options) {
    const bitmapOptions = options.length >= 5 ? options[4] : options[0];
    const resizeWidth = bitmapOptions?.resizeWidth;
    const resizeHeight = bitmapOptions?.resizeHeight;
    for (const dimension of [resizeWidth, resizeHeight]) {
      if (dimension !== undefined && (!Number.isSafeInteger(dimension) || dimension < 1 || dimension > 8192)) {
        throw new RangeError('ImageBitmap resize dimensions exceed the supported limit');
      }
    }
    if (resizeWidth && resizeHeight && resizeWidth * resizeHeight > 67_108_864) {
      throw new RangeError('ImageBitmap resize dimensions exceed the supported limit');
    }
    let cropWidth = 0;
    let cropHeight = 0;
    if (options.length >= 4) {
      cropWidth = Math.abs(Number(options[2]));
      cropHeight = Math.abs(Number(options[3]));
      if (!Number.isSafeInteger(cropWidth) || !Number.isSafeInteger(cropHeight)
        || cropWidth > 8192 || cropHeight > 8192 || cropWidth * cropHeight > 67_108_864) {
        throw new RangeError('ImageBitmap crop dimensions exceed the supported limit');
      }
    }
    if (source instanceof root.HTMLImageElement) {
      if (!source.__peregrustBitmap) throw new DOMException('Image is not loaded', 'InvalidStateError');
      source = source.__peregrustBitmap;
    }
    if (source instanceof root.Blob) {
      const encoded = new Uint8Array(await source.arrayBuffer());
      ops.op_peregrust_validate_image(encoded, cropWidth, cropHeight,
        Number(resizeWidth ?? 0), Number(resizeHeight ?? 0));
    }
    return nativeCreateImageBitmap(source, ...options);
  };

  const validHeaderName = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
  function normalizeHeaderName(name) {
    const normalized = String(name).toLowerCase();
    if (!validHeaderName.test(normalized)) throw new TypeError(`invalid header name: ${name}`);
    return normalized;
  }
  function normalizeHeaderValue(value) {
    const normalized = String(value).trim();
    if (/[\0\r\n]/.test(normalized)) throw new TypeError('invalid header value');
    return normalized;
  }
  class Headers {
    #values = new Map();
    constructor(init) {
      if (init == null) return;
      if (init instanceof Headers) {
        for (const [key, value] of init) this.append(key, value);
      } else if (Array.isArray(init) || typeof init[Symbol.iterator] === 'function') {
        for (const pair of init) {
          if (!Array.isArray(pair) || pair.length !== 2) throw new TypeError('header pair must contain two values');
          this.append(pair[0], pair[1]);
        }
      } else if (typeof init === 'object') {
        for (const [key, value] of Object.entries(init)) this.append(key, value);
      } else {
        throw new TypeError('invalid Headers initializer');
      }
    }
    append(name, value) {
      const key = normalizeHeaderName(name);
      const next = normalizeHeaderValue(value);
      this.#values.set(key, this.#values.has(key) ? `${this.#values.get(key)}, ${next}` : next);
    }
    set(name, value) { this.#values.set(normalizeHeaderName(name), normalizeHeaderValue(value)); }
    get(name) { return this.#values.get(normalizeHeaderName(name)) ?? null; }
    has(name) { return this.#values.has(normalizeHeaderName(name)); }
    delete(name) { this.#values.delete(normalizeHeaderName(name)); }
    forEach(callback, thisArg) { for (const [key, value] of this) callback.call(thisArg, value, key, this); }
    *entries() { yield* [...this.#values.entries()].sort(([a], [b]) => a.localeCompare(b)); }
    *keys() { for (const [key] of this) yield key; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
  }

  class Request {
    constructor(input, init = {}) {
      const previous = input instanceof Request ? input : null;
      this.url = String(previous?.url ?? input);
      this.method = String(init.method ?? previous?.method ?? 'GET').toUpperCase();
      this.headers = new Headers(init.headers ?? previous?.headers);
      this.signal = init.signal ?? previous?.signal ?? null;
      this.credentials = init.credentials ?? previous?.credentials ?? 'same-origin';
      this.mode = init.mode ?? previous?.mode ?? 'cors';
      this.cache = init.cache ?? previous?.cache ?? 'default';
      this.redirect = init.redirect ?? previous?.redirect ?? 'follow';
      if (init.body != null) throw new TypeError('local Request bodies are unsupported');
      if (!/^[A-Z]+$/.test(this.method)) throw new TypeError('invalid request method');
    }
    clone() { return new Request(this); }
  }

  function bytesFromBody(body) {
    if (body == null) return null;
    if (body instanceof Uint8Array) return body;
    if (body instanceof ArrayBuffer) return new Uint8Array(body);
    if (ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
    if (typeof body === 'string') return new TextEncoder().encode(body);
    return null;
  }
  async function collectStream(stream) {
    const reader = stream.getReader();
    const chunks = [];
    let length = 0;
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        const chunk = value instanceof Uint8Array ? value : new Uint8Array(value);
        length += chunk.byteLength;
        if (length > MAX_DATA_BYTES) throw new RangeError('response body exceeds 128 MiB limit');
        chunks.push(chunk);
      }
    } finally {
      reader.releaseLock();
    }
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return bytes;
  }
  class Response {
    #bytes;
    #stream;
    #used = false;
    constructor(body = null, init = {}) {
      this.status = Number(init.status ?? 200);
      if (!Number.isInteger(this.status) || this.status < 200 || this.status > 599) {
        throw new RangeError('invalid response status');
      }
      this.statusText = String(init.statusText ?? (this.status === 200 ? 'OK' : this.status === 404 ? 'Not Found' : ''));
      this.headers = new Headers(init.headers);
      this.url = String(init.url ?? '');
      this.redirected = false;
      this.type = 'basic';
      if (body instanceof root.Blob) {
        this.#stream = body.stream();
        if (body.type && !this.headers.has('content-type')) this.headers.set('content-type', body.type);
      } else if (body instanceof streams.ReadableStream) {
        this.#stream = body;
      } else {
        this.#bytes = bytesFromBody(body);
        if (this.#bytes === null && body !== null) throw new TypeError('unsupported response body');
        if (typeof body === 'string' && !this.headers.has('content-type')) {
          this.headers.set('content-type', 'text/plain;charset=UTF-8');
        }
      }
    }
    get ok() { return this.status >= 200 && this.status <= 299; }
    get bodyUsed() { return this.#used || Boolean(this.#stream?.locked); }
    get body() {
      if (this.#bytes === null && !this.#stream) return null;
      if (!this.#stream) {
        const bytes = this.#bytes;
        this.#stream = new streams.ReadableStream({
          start(controller) { controller.enqueue(bytes); controller.close(); },
        });
      }
      return this.#stream;
    }
    async #consume() {
      if (this.bodyUsed) throw new TypeError('response body has already been consumed');
      this.#used = true;
      if (this.#stream) return collectStream(this.#stream);
      return this.#bytes ?? new Uint8Array();
    }
    async arrayBuffer() {
      const bytes = await this.#consume();
      return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    }
    async blob() { return new root.Blob([await this.#consume()], { type: this.headers.get('content-type') ?? '' }); }
    async text() { return new TextDecoder().decode(await this.#consume()); }
    async json() { return JSON.parse(await this.text()); }
    clone() {
      if (this.bodyUsed) throw new TypeError('response body has already been consumed');
      const init = { status: this.status, statusText: this.statusText, headers: this.headers, url: this.url };
      if (this.#stream) {
        const [left, right] = this.#stream.tee();
        this.#stream = left;
        return new Response(right, init);
      }
      return new Response(this.#bytes, init);
    }
    static json(value, init = {}) {
      const headers = new Headers(init.headers);
      if (!headers.has('content-type')) headers.set('content-type', 'application/json');
      return new Response(JSON.stringify(value), { ...init, headers });
    }
  }

  function mimeType(path) {
    const extension = path.split(/[?#]/, 1)[0].split('.').at(-1)?.toLowerCase();
    return ({ json: 'application/json', glb: 'model/gltf-binary', gltf: 'model/gltf+json',
      bin: 'application/octet-stream', png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
      webp: 'image/webp', gif: 'image/gif', svg: 'image/svg+xml', txt: 'text/plain',
      css: 'text/css', js: 'text/javascript', wasm: 'application/wasm', ktx2: 'image/ktx2',
    })[extension] ?? 'application/octet-stream';
  }
  function localAssetPath(url) {
    if (url.startsWith('file:')) return new URL(url).href;
    if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(url)) throw new TypeError(`fetch does not support ${new URL(url).protocol} URLs`);
    const path = url.split(/[?#]/, 1)[0];
    if (path.startsWith('//')) throw new TypeError('network-relative URLs are unsupported');
    try { return decodeURIComponent(path); }
    catch { throw new TypeError('invalid percent encoding in asset URL'); }
  }
  function abortReason(signal) {
    return signal?.reason ?? new DOMException('The operation was aborted', 'AbortError');
  }
  async function withAbort(operation, signal) {
    if (!signal) return operation;
    if (signal.aborted) throw abortReason(signal);
    let onAbort;
    const aborted = new Promise((_, reject) => {
      onAbort = () => reject(abortReason(signal));
      signal.addEventListener('abort', onAbort, { once: true });
    });
    try { return await Promise.race([operation, aborted]); }
    finally { signal.removeEventListener('abort', onAbort); }
  }
  function dataUrlResponse(url) {
    const comma = url.indexOf(',');
    if (comma < 0) throw new TypeError('invalid data URL');
    const metadata = url.slice(5, comma);
    const isBase64 = /;base64$/i.test(metadata);
    const mediaType = (isBase64 ? metadata.slice(0, -7) : metadata) || 'text/plain;charset=US-ASCII';
    const payload = url.slice(comma + 1);
    let bytes;
    if (isBase64) {
      if (payload.length > Math.ceil(MAX_DATA_BYTES * 4 / 3) + 4) {
        throw new RangeError('data URL exceeds 128 MiB limit');
      }
      const decoded = atob(payload.replace(/\s/g, ''));
      bytes = Uint8Array.from(decoded, (char) => char.charCodeAt(0));
    } else {
      const chunks = [];
      for (let i = 0; i < payload.length; i++) {
        if (payload[i] === '%') {
          if (!/^[0-9a-fA-F]{2}$/.test(payload.slice(i + 1, i + 3))) throw new TypeError('invalid data URL escape');
          chunks.push(parseInt(payload.slice(i + 1, i + 3), 16));
          i += 2;
        } else {
          const point = payload.codePointAt(i);
          const encoded = new TextEncoder().encode(String.fromCodePoint(point));
          for (const byte of encoded) chunks.push(byte);
          if (point > 0xffff) i++;
        }
        if (chunks.length > MAX_DATA_BYTES) throw new RangeError('data URL exceeds 128 MiB limit');
      }
      bytes = Uint8Array.from(chunks);
    }
    if (bytes.byteLength > MAX_DATA_BYTES) throw new RangeError('data URL exceeds 128 MiB limit');
    return new Response(bytes, { headers: { 'content-type': mediaType, 'content-length': String(bytes.byteLength) }, url });
  }
  async function fetch(input, init = {}) {
    const request = new Request(input, init);
    if (request.signal?.aborted) throw abortReason(request.signal);
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      return new Response(null, { status: 405, statusText: 'Method Not Allowed', url: request.url });
    }
    if (request.url.startsWith('data:')) return dataUrlResponse(request.url);
    if (request.url.startsWith('blob:')) {
      const blob = blobFromObjectUrl(request.url);
      return blob ? new Response(request.method === 'HEAD' ? null : blob,
        { headers: { 'content-type': blob.type || 'application/octet-stream', 'content-length': String(blob.size) }, url: request.url })
        : new Response(null, { status: 404, url: request.url });
    }
    const path = localAssetPath(request.url);
    const metadata = await withAbort(ops.op_peregrust_asset_stat(path), request.signal);
    if (!metadata) return new Response(null, { status: 404, url: request.url });
    const headers = { 'content-type': mimeType(path), 'content-length': String(metadata.size) };
    if (request.method === 'HEAD') return new Response(null, { headers, url: request.url });
    const bytes = await withAbort(ops.op_peregrust_fetch_asset(path), request.signal);
    headers['content-length'] = String(bytes.byteLength);
    return new Response(bytes, { headers, url: request.url });
  }

  // ImageLoader/TextureLoader use an <img> element. This object contains a real
  // ImageBitmap; only image loading is emulated, not general DOM construction.
  class HTMLImageElement {
    #src = '';
    #bitmap = null;
    #listeners = new Map();
    #generation = 0;
    #loadPromise = null;
    complete = false;
    crossOrigin = null;
    onload = null;
    onerror = null;
    get src() { return this.#src; }
    set src(url) {
      this.#src = String(url);
      this.complete = false;
      this.#bitmap?.close();
      this.#bitmap = null;
      const generation = ++this.#generation;
      this.#loadPromise = (async () => {
        const response = await fetch(this.#src);
        if (!response.ok) throw new Error(`image asset ${this.#src} returned ${response.status}`);
        return root.createImageBitmap(await response.blob());
      })();
      this.#loadPromise.then((bitmap) => {
        if (generation !== this.#generation) { bitmap.close(); return; }
        this.#bitmap = bitmap;
        this.complete = true;
        this.#emit('load');
      }, (error) => { if (generation === this.#generation) this.#emit('error', error); });
    }
    get width() { return this.#bitmap?.width ?? 0; }
    get height() { return this.#bitmap?.height ?? 0; }
    get naturalWidth() { return this.width; }
    get naturalHeight() { return this.height; }
    get __peregrustBitmap() { return this.#bitmap; }
    async decode() {
      if (!this.#loadPromise) throw new DOMException('Image source is empty', 'InvalidStateError');
      await this.#loadPromise;
    }
    addEventListener(type, callback) {
      const list = this.#listeners.get(String(type)) ?? new Set();
      list.add(callback); this.#listeners.set(String(type), list);
    }
    removeEventListener(type, callback) { this.#listeners.get(String(type))?.delete(callback); }
    #emit(type, error) {
      const event = { type, target: this, error };
      for (const callback of [...(this.#listeners.get(type) ?? [])]) callback.call(this, event);
      const handler = this[`on${type}`];
      if (typeof handler === 'function') handler.call(this, event);
    }
  }
  const previousCreateElement = root.document.createElement.bind(root.document);
  root.document.createElement = (tag) => String(tag).toLowerCase() === 'img'
    ? new HTMLImageElement()
    : previousCreateElement(tag);
  root.Image = HTMLImageElement;
  root.HTMLImageElement = HTMLImageElement;
  root.window.Image = HTMLImageElement;
  root.window.HTMLImageElement = HTMLImageElement;

  // deno_webgpu 0.226 has writeTexture but no copyExternalImageToTexture.
  // Upload actual decoded ImageBitmap pixels through the same GPUQueue.
  Object.defineProperty(root.GPUQueue.prototype, 'copyExternalImageToTexture', {
    configurable: true,
    writable: true,
    value(source, destination, copySize) {
      const bitmap = source?.source instanceof image.ImageBitmap
        ? source.source : source?.source?.__peregrustBitmap;
      if (!(bitmap instanceof image.ImageBitmap)) {
        throw new TypeError('copyExternalImageToTexture requires a loaded ImageBitmap');
      }
      const width = Number(copySize?.width ?? copySize?.[0]);
      const height = Number(copySize?.height ?? copySize?.[1]);
      const depth = Number(copySize?.depthOrArrayLayers ?? copySize?.depth ?? copySize?.[2] ?? 1);
      const x = Number(source.origin?.x ?? 0);
      const y = Number(source.origin?.y ?? 0);
      for (const value of [width, height, depth, x, y]) {
        if (!Number.isSafeInteger(value) || value < 0) throw new RangeError('invalid external image copy rectangle');
      }
      if (depth !== 1) throw new RangeError('external ImageBitmap copies require one destination layer');
      if (width === 0 || height === 0) return;
      const pixels = ops.op_peregrust_bitmap_rgba(bitmap, x, y, width, height, Boolean(source.flipY));
      const format = destination.texture?.format;
      if (!['rgba8unorm', 'rgba8unorm-srgb', 'bgra8unorm', 'bgra8unorm-srgb'].includes(format)) {
        throw new TypeError(`external ImageBitmap copy does not support texture format ${format}`);
      }
      if (destination.colorSpace != null && destination.colorSpace !== 'srgb') {
        throw new TypeError(`external ImageBitmap copy does not support color space ${destination.colorSpace}`);
      }
      if (destination.premultipliedAlpha || format.startsWith('bgra')) {
        for (let index = 0; index < pixels.length; index += 4) {
          if (destination.premultipliedAlpha) {
            const alpha = pixels[index + 3] / 255;
            pixels[index] = Math.round(pixels[index] * alpha);
            pixels[index + 1] = Math.round(pixels[index + 1] * alpha);
            pixels[index + 2] = Math.round(pixels[index + 2] * alpha);
          }
          if (format.startsWith('bgra')) {
            const red = pixels[index];
            pixels[index] = pixels[index + 2];
            pixels[index + 2] = red;
          }
        }
      }
      this.writeTexture(
        { texture: destination.texture, mipLevel: destination.mipLevel ?? 0,
          origin: destination.origin ?? { x: 0, y: 0, z: 0 }, aspect: destination.aspect ?? 'all' },
        pixels,
        { offset: 0, bytesPerRow: width * 4, rowsPerImage: height },
        { width, height, depthOrArrayLayers: 1 },
      );
    },
  });

  root.Headers = Headers;
  root.Request = Request;
  root.Response = Response;
  root.fetch = fetch;
  root.window.ReadableStream = streams.ReadableStream;
  root.window.ProgressEvent = events.ProgressEvent;
  root.window.Blob = root.Blob;
  root.window.ImageBitmap = image.ImageBitmap;
  root.window.createImageBitmap = root.createImageBitmap;
  root.window.Headers = Headers;
  root.window.Request = Request;
  root.window.Response = Response;
  root.window.fetch = fetch;
})(globalThis);
