(function (root) {
  'use strict';
  const runtime = root.Peregrust;
  const ops = root.Deno?.core?.ops;
  const native = root.__peregrustControlNative ?? {
    enabled: () => ops.op_peregrust_control_enabled(),
    poll: () => ops.op_peregrust_control_poll(),
    reply: (value) => ops.op_peregrust_control_reply(value),
    cancelled: () => ops.op_peregrust_control_cancelled(),
    redraw: () => ops.op_peregrust_request_redraw(),
    png: (width, height, pixels) => ops.op_peregrust_control_png(width, height, pixels),
  };
  const enabled = native.enabled();
  const scenes = new Map();
  const states = new Map();
  let active = null;
  const error = (code, message) => Object.assign(new Error(message), { code });
  const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
  const integer = (value, min, max, label) => {
    if (!Number.isInteger(value) || value < min || value > max) {
      throw error('INVALID_ARGUMENT', `${label} must be an integer between ${min} and ${max}`);
    }
    return value;
  };
  const schema = (description, properties = {}, required = []) => ({
    description, inputSchema: { type: 'object', properties, required, additionalProperties: false },
  });
  const string = { type: 'string' };
  const sceneProperty = { scene: { type: 'string', description: 'Registered scene name; defaults to main.' } };
  const vector = { type: 'array', items: { type: 'number' }, minItems: 3, maxItems: 3 };
  const captureProperties = { ...sceneProperty,
    width: { type: 'integer', minimum: 1, maximum: 2048 },
    height: { type: 'integer', minimum: 1, maximum: 2048 },
  };
  const queryProperties = {
    ...sceneProperty, id: string, name: string, type: string, tag: string,
    fields: { type: 'array', items: { enum: ['id', 'name', 'type', 'parent', 'position', 'rotation', 'scale', 'visible', 'worldPosition', 'tags'] } },
    offset: { type: 'integer', minimum: 0 }, limit: { type: 'integer', minimum: 1, maximum: 500 },
  };
  const methods = {
    'control.describe': schema('List operations and JSON parameter schemas.'),
    'runtime.info': schema('Inspect the running game at a completed callback boundary.'),
    'scene.list': schema('List explicitly registered scene adapters.'),
    'scene.query': schema('Find objects by exact name, ID, type or tag; results are bounded.', queryProperties),
    'scene.update': schema('Update local transform, visibility or name before the next game frame.', {
      ...sceneProperty, id: string, position: vector, rotation: vector, scale: vector,
      visible: { type: 'boolean' }, name: string,
    }, ['id']),
    'state.list': schema('List game-defined state providers.'),
    'state.get': schema('Read a game-defined JSON state.', { name: string }, ['name']),
    'input.dispatch': schema('Dispatch one keyboard or mouse event before game callbacks.', {
      event: { type: 'object', description: 'keydown/up, pointerdown/up/move, wheel or click. Keyboard uses code/key; mouse coordinates are logical pixels.' },
    }, ['event']),
    'input.key': schema('Hold a key for N completed game callbacks, release it, then observe.', {
      code: string, key: string, frames: { type: 'integer', minimum: 1, maximum: 600 },
      observe: { type: 'object', properties: {
        query: { type: 'object', properties: queryProperties, additionalProperties: false }, state: string,
        capture: { type: 'object', properties: captureProperties, additionalProperties: false },
      }, additionalProperties: false },
    }, ['code', 'key']),
    'frame.capture': schema('Render the registered Three.js scene/camera into an RGBA8 PNG; excludes custom postprocessing.', {
      ...captureProperties,
    }),
  };

  // Validate the subset used by these built-in schemas, including nested
  // observations, before dispatching input or applying a scene mutation.
  function validate(definition, value, path = 'params') {
    const { type } = definition;
    const valid = !type || (type === 'object' ? object(value)
      : type === 'array' ? Array.isArray(value)
      : type === 'integer' ? Number.isSafeInteger(value)
      : type === 'number' ? Number.isFinite(value)
      : typeof value === type);
    if (!valid || (definition.enum && !definition.enum.includes(value))) throw error('INVALID_ARGUMENT', `${path} has an invalid value`);
    if (typeof value === 'number' && ((definition.minimum !== undefined && value < definition.minimum)
      || (definition.maximum !== undefined && value > definition.maximum))) throw error('INVALID_ARGUMENT', `${path} is out of range`);
    if (Array.isArray(value)) {
      if ((definition.minItems !== undefined && value.length < definition.minItems)
        || (definition.maxItems !== undefined && value.length > definition.maxItems)) throw error('INVALID_ARGUMENT', `${path} has an invalid length`);
      if (definition.items) value.forEach((item, index) => validate(definition.items, item, `${path}[${index}]`));
    }
    if (object(value)) {
      for (const key of definition.required ?? []) {
        if (!Object.hasOwn(value, key)) throw error('INVALID_ARGUMENT', `${path}.${key} is required`);
      }
      for (const key of Object.keys(value)) {
        if (Object.hasOwn(definition.properties ?? {}, key)) validate(definition.properties[key], value[key], `${path}.${key}`);
        else if (definition.additionalProperties === false) throw error('INVALID_ARGUMENT', `unknown parameter: ${path}.${key}`);
      }
    }
  }

  function register(map, name, value) {
    if (typeof name !== 'string' || !name || name.length > 128) throw new TypeError('registration needs a name of 1–128 characters');
    if (map.has(name)) throw new Error(`already registered: ${name}`);
    map.set(name, value);
    return () => { if (map.get(name) === value) map.delete(name); };
  }
  function getScene(params = {}) {
    const name = params.scene ?? 'main';
    const adapter = scenes.get(name);
    if (!adapter) throw error('SCENE_NOT_FOUND', `scene is not registered: ${name}`);
    return adapter;
  }
  function getState(name) {
    const provider = states.get(name);
    if (!provider) throw error('STATE_NOT_FOUND', `state is not registered: ${name}`);
    const value = provider();
    if (value?.then) throw error('INVALID_STATE', 'state providers must return synchronous JSON');
    const text = JSON.stringify(value);
    if (text === undefined || text.length > 1024 * 1024) throw error('INVALID_STATE', 'state must be JSON smaller than 1 MiB');
    return JSON.parse(text);
  }
  function validateEvent(event) {
    if (!object(event) || !['keydown', 'keyup', 'pointerdown', 'pointerup', 'pointermove', 'wheel', 'click'].includes(event.type)) {
      throw error('INVALID_ARGUMENT', 'unsupported input event type');
    }
    if (event.type.startsWith('key') && (typeof event.code !== 'string' || typeof event.key !== 'string')) {
      throw error('INVALID_ARGUMENT', 'keyboard events require code and key strings');
    }
    for (const name of ['clientX', 'clientY', 'movementX', 'movementY', 'deltaX', 'deltaY', 'deltaZ', 'button', 'buttons', 'pointerId']) {
      if (event[name] !== undefined && !Number.isFinite(event[name])) throw error('INVALID_ARGUMENT', `${name} must be finite`);
    }
    return event;
  }
  function dispatch(event) { root.__peregrustDispatchEvent(event); }
  function release() {
    if (active?.held) {
      const event = active.held;
      active.held = null;
      dispatch({ ...event, type: 'keyup', repeat: false });
    }
  }
  function finish(value, failure) {
    const frame = runtime.frameCount;
    active = null;
    let response;
    try {
      response = failure
        ? { ok: false, frame, error: { code: failure.code ?? 'OPERATION_FAILED', message: String(failure.message ?? failure) } }
        : JSON.parse(JSON.stringify({ ok: true, frame, result: value }));
    } catch (cause) {
      response = { ok: false, frame, error: { code: 'INVALID_RESULT', message: String(cause) } };
    }
    native.reply(response);
  }
  async function capture(params = {}) {
    const adapter = getScene(params);
    if (typeof adapter.capture !== 'function') throw error('UNSUPPORTED', 'scene adapter cannot capture');
    const image = await adapter.capture(params);
    return { mimeType: 'image/png', width: image.width, height: image.height,
      data: native.png(image.width, image.height, image.pixels) };
  }
  async function observe(params = {}) {
    const result = {};
    // Read state before asynchronous GPU readback so it corresponds to submission.
    if (params.query !== undefined) result.objects = getScene(params.query).query(params.query);
    if (params.state !== undefined) result.state = getState(params.state);
    if (params.capture !== undefined) result.capture = await capture(params.capture);
    return result;
  }

  runtime.control = Object.freeze({
    enabled,
    registerScene(name, adapter) {
      if (!adapter || typeof adapter.query !== 'function' || typeof adapter.update !== 'function') {
        throw new TypeError('scene adapter needs query and update functions');
      }
      return register(scenes, name, adapter);
    },
    registerState(name, provider) {
      if (typeof provider !== 'function') throw new TypeError('state provider must be a function');
      return register(states, name, provider);
    },
  });
  if (!enabled) return;

  root.__peregrustControlBeforeFrame = async () => {
    try {
      if (active && native.cancelled()) {
        release();
        finish(null, error('TIMEOUT', 'request expired'));
      }
      if (active) return;
      const request = native.poll();
      if (!request) return;
      active = { ...request, params: request.params ?? {} };
      const { method, params } = active;
      if (!Object.hasOwn(methods, method)) throw error('METHOD_NOT_FOUND', `unknown operation: ${method}`);
      validate(methods[method].inputSchema, params);
      if (method === 'scene.update') getScene(params).update(params);
      if (method === 'input.dispatch') dispatch(validateEvent(params.event));
      if (method === 'input.key') {
        active.remaining = integer(params.frames ?? 1, 1, 600, 'frames');
        if (params.observe?.query) getScene(params.observe.query);
        if (params.observe?.capture) getScene(params.observe.capture);
        if (params.observe?.state !== undefined && !states.has(params.observe.state)) throw error('STATE_NOT_FOUND', `state is not registered: ${params.observe.state}`);
        const event = validateEvent({ type: 'keydown', code: params.code, key: params.key, repeat: false });
        active.held = event;
        dispatch(event);
      }
    } catch (cause) {
      try { release(); } catch { /* Preserve the original request error. */ }
      finish(null, cause);
    }
  };
  root.__peregrustControlAfterFrame = async () => {
    if (!active) return;
    try {
      const { method, params } = active;
      let result;
      switch (method) {
        case 'control.describe': result = { protocolVersion: 1, methods }; break;
        case 'runtime.info': result = { frame: runtime.frameCount, width: runtime.width, height: runtime.height,
          scenes: [...scenes.keys()], states: [...states.keys()], clock: 'realtime' }; break;
        case 'scene.list': result = { scenes: [...scenes.keys()] }; break;
        case 'scene.query': result = getScene(params).query(params); break;
        case 'scene.update': result = getScene(params).query({ id: params.id }); break;
        case 'state.list': result = { states: [...states.keys()] }; break;
        case 'state.get': result = { name: params.name, value: getState(params.name) }; break;
        case 'input.dispatch': result = { dispatched: true }; break;
        case 'input.key':
          if (--active.remaining > 0) { native.redraw(); return; }
          release();
          result = { frames: params.frames ?? 1, ...await observe(params.observe) };
          break;
        case 'frame.capture': result = { capture: await capture(params) }; break;
      }
      finish(result);
    } catch (cause) {
      try { release(); } catch { /* Preserve the original request error. */ }
      finish(null, cause);
    }
  };
})(globalThis);
