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
  const actions = new Map();
  let active = null;
  let paused = false, simulationTime = null, lastHostTime = null, resetClock = false;
  let frameStarted = 0;
  const durations = [];
  const logs = [];
  let logSequence = 0;
  const now = () => root.performance?.now() ?? Date.now();
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
  const observation = { type: 'object', properties: {
    query: { type: 'object', properties: queryProperties, additionalProperties: false }, state: string,
    capture: { type: 'object', properties: captureProperties, additionalProperties: false },
  }, additionalProperties: false };
  const methods = {
    'control.describe': schema('List operations and JSON parameter schemas.'),
    'runtime.info': schema('Inspect the running game at a completed callback boundary.'),
    'runtime.pause': schema('Pause animation callbacks. Timers, I/O and wall clocks continue.'),
    'runtime.resume': schema('Resume animation callbacks without adding the paused interval to animation time.'),
    'runtime.step': schema('While paused, advance N callbacks with fixed animation timestamps, then observe. Does not virtualize timers or randomness.', {
      frames: { type: 'integer', minimum: 1, maximum: 600 },
      dtMs: { type: 'number', minimum: 0.001, maximum: 1000 }, observe: observation,
    }),
    'runtime.metrics': schema('Callback duration samples in milliseconds; excludes capture readback and does not measure GPU time.'),
    'runtime.logs': schema('Read a bounded console log ring using a sequence cursor.', {
      after: { type: 'integer', minimum: 0 }, limit: { type: 'integer', minimum: 1, maximum: 200 },
      level: { enum: ['debug', 'info', 'warn', 'error'] },
    }),
    'scene.list': schema('List explicitly registered scene adapters.'),
    'scene.query': schema('Find objects by exact name, ID, type or tag; results are bounded.', queryProperties),
    'scene.update': schema('Update local transform, visibility or name before the next game frame.', {
      ...sceneProperty, id: string, position: vector, rotation: vector, scale: vector,
      visible: { type: 'boolean' }, name: string,
    }, ['id']),
    'state.list': schema('List game-defined state providers.'),
    'state.get': schema('Read a game-defined JSON state.', { name: string }, ['name']),
    'action.list': schema('Discover game-defined actions and their parameter schemas.', {
      offset: { type: 'integer', minimum: 0 }, limit: { type: 'integer', minimum: 1, maximum: 200 },
    }),
    'action.call': schema('Validate and execute a synchronous game action, optionally observing its result.', {
      name: string, input: { type: 'object' }, observe: observation,
    }, ['name']),
    'input.dispatch': schema('Dispatch one keyboard or mouse event before game callbacks.', {
      event: { type: 'object', description: 'keydown/up, pointerdown/up/move, wheel or click. Keyboard uses code/key; mouse coordinates are logical pixels.' },
    }, ['event']),
    'input.key': schema('Hold a key for N completed game callbacks, release it, then observe.', {
      code: string, key: string, frames: { type: 'integer', minimum: 1, maximum: 600 },
      observe: observation,
    }, ['code', 'key']),
    'frame.capture': schema('Capture an RGBA8 PNG through the registered scene adapter; custom pipeline support depends on the adapter.', {
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
    if (typeof value === 'string' && ((definition.minLength !== undefined && [...value].length < definition.minLength)
      || (definition.maxLength !== undefined && [...value].length > definition.maxLength))) throw error('INVALID_ARGUMENT', `${path} has an invalid length`);
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

  function checkSchema(definition, depth = 0) {
    if (!object(definition) || depth > 16) throw new TypeError('action schema must be an object with at most 16 levels');
    const allowed = ['type', 'description', 'title', 'properties', 'required', 'additionalProperties', 'enum', 'items',
      'minimum', 'maximum', 'minItems', 'maxItems', 'minLength', 'maxLength', 'default'];
    for (const key of Object.keys(definition)) if (!allowed.includes(key)) throw new TypeError(`unsupported action schema keyword: ${key}`);
    if (definition.type !== undefined && !['object', 'array', 'string', 'number', 'integer', 'boolean'].includes(definition.type)) throw new TypeError('unsupported action schema type');
    if (definition.additionalProperties !== undefined && typeof definition.additionalProperties !== 'boolean') throw new TypeError('additionalProperties must be boolean');
    if (definition.properties !== undefined && !object(definition.properties)) throw new TypeError('properties must be an object');
    if (definition.required !== undefined && (!Array.isArray(definition.required) || definition.required.some(key => typeof key !== 'string'))) throw new TypeError('required must be a string array');
    if (definition.enum !== undefined && (!Array.isArray(definition.enum) || definition.enum.some(value => object(value) || Array.isArray(value)))) throw new TypeError('enum must contain primitive values');
    for (const key of ['minimum', 'maximum', 'minItems', 'maxItems', 'minLength', 'maxLength']) {
      if (definition[key] !== undefined && !Number.isFinite(definition[key])) throw new TypeError(`${key} must be finite`);
    }
    for (const child of Object.values(definition.properties ?? {})) checkSchema(child, depth + 1);
    if (definition.items) checkSchema(definition.items, depth + 1);
  }

  function preflightObservation(params) {
    if (params?.query) getScene(params.query);
    if (params?.capture) {
      if (typeof getScene(params.capture).capture !== 'function') throw error('UNSUPPORTED', 'scene adapter cannot capture');
    }
    if (params?.state !== undefined && !states.has(params.state)) throw error('STATE_NOT_FOUND', `state is not registered: ${params.state}`);
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
    registerAction(name, definition, handler) {
      if (!definition || typeof definition.description !== 'string' || typeof handler !== 'function') throw new TypeError('action needs a description and synchronous handler');
      checkSchema(definition.inputSchema);
      if (definition.inputSchema.type !== 'object') throw new TypeError('action inputSchema must have type object');
      return register(actions, name, { description: definition.description,
        inputSchema: JSON.parse(JSON.stringify(definition.inputSchema)), handler });
    },
  });
  if (!enabled) return;

  root.__peregrustRecordLog = (message, level) => {
    logs.push({ sequence: ++logSequence, frame: runtime.frameCount,
      level: ['debug', 'info', 'warn', 'error'][Math.min(3, Math.max(0, level))] ?? 'info',
      message: String(message).slice(0, 4096) });
    if (logs.length > 1024) logs.shift();
  };
  root.__peregrustControlShouldSchedule = () => !paused;
  root.__peregrustControlClock = (timestamp, advance) => {
    if (advance) {
      if (paused) simulationTime = (simulationTime ?? 0) + (active?.params.dtMs ?? 1000 / 60);
      else if (lastHostTime === null || resetClock) { simulationTime ??= timestamp; resetClock = false; }
      else simulationTime = (simulationTime ?? 0) + Math.max(0, timestamp - lastHostTime);
    }
    lastHostTime = timestamp;
    return simulationTime ?? 0;
  };
  root.__peregrustControlFrameStarted = () => { frameStarted = now(); };

  root.__peregrustControlBeforeFrame = async () => {
    try {
      if (active && native.cancelled()) {
        release();
        finish(null, error('TIMEOUT', 'request expired'));
      }
      if (active) return !paused || (active.remaining ?? 0) > 0;
      const request = native.poll();
      if (!request) return !paused;
      active = { ...request, params: request.params ?? {} };
      const { method, params } = active;
      if (!Object.hasOwn(methods, method)) throw error('METHOD_NOT_FOUND', `unknown operation: ${method}`);
      validate(methods[method].inputSchema, params);
      preflightObservation(params.observe);
      if (method === 'runtime.pause') paused = true;
      if (method === 'runtime.resume') { paused = false; resetClock = true; }
      if (method === 'runtime.step') {
        if (!paused) throw error('INVALID_STATE', 'pause the runtime before stepping');
        active.remaining = params.frames ?? 1;
      }
      if (method === 'action.call') {
        const action = actions.get(params.name);
        if (!action) throw error('ACTION_NOT_FOUND', `action is not registered: ${params.name}`);
        validate(action.inputSchema, params.input ?? {}, 'input');
        const value = action.handler(params.input ?? {});
        if (value?.then) throw error('INVALID_RESULT', 'action handlers must return synchronous JSON; do not await another frame');
        const serialized = JSON.stringify(value ?? null);
        if (serialized.length > 1024 * 1024) throw error('INVALID_RESULT', 'action result exceeds 1 MiB');
        active.actionValue = JSON.parse(serialized);
      }
      if (method === 'scene.update') getScene(params).update(params);
      if (method === 'input.dispatch') dispatch(validateEvent(params.event));
      if (method === 'input.key') {
        active.remaining = integer(params.frames ?? 1, 1, 600, 'frames');
        const event = validateEvent({ type: 'keydown', code: params.code, key: params.key, repeat: false });
        active.held = event;
        dispatch(event);
      }
      return !paused || (active.remaining ?? 0) > 0;
    } catch (cause) {
      try { release(); } catch { /* Preserve the original request error. */ }
      finish(null, cause);
      return !paused;
    }
  };
  root.__peregrustControlAfterFrame = async (advanced) => {
    if (advanced) {
      durations.push(Math.max(0, now() - frameStarted));
      if (durations.length > 240) durations.shift();
    }
    if (!active) return;
    try {
      const { method, params } = active;
      let result;
      switch (method) {
        case 'control.describe': result = { protocolVersion: 1, methods }; break;
        case 'runtime.info': result = { frame: runtime.frameCount, width: runtime.width, height: runtime.height,
          scenes: [...scenes.keys()], states: [...states.keys()], actions: actions.size,
          paused, clock: 'animation', animationTimeMs: simulationTime ?? 0,
          wallClock: 'realtime', timers: 'realtime' }; break;
        case 'runtime.pause': case 'runtime.resume': result = { paused, animationTimeMs: simulationTime ?? 0 }; break;
        case 'runtime.step':
          if (--active.remaining > 0) { native.redraw(); return; }
          result = { frames: params.frames ?? 1, dtMs: params.dtMs ?? 1000 / 60, paused, ...await observe(params.observe) };
          break;
        case 'runtime.metrics': {
          const sorted = [...durations].sort((a, b) => a - b);
          result = { frames: runtime.frameCount, paused, samples: durations.length, animationTimeMs: simulationTime ?? 0,
            callbackMs: { last: durations.at(-1) ?? 0,
              mean: durations.reduce((a, b) => a + b, 0) / Math.max(1, durations.length),
              p95: sorted[Math.max(0, Math.ceil(sorted.length * 0.95) - 1)] ?? 0 } };
          break;
        }
        case 'runtime.logs': {
          const entries = logs.filter(entry => entry.sequence > (params.after ?? 0) && (!params.level || entry.level === params.level)).slice(0, params.limit ?? 50);
          result = { entries, nextCursor: entries.at(-1)?.sequence ?? logSequence, oldestSequence: logs[0]?.sequence ?? logSequence + 1 };
          break;
        }
        case 'scene.list': result = { scenes: [...scenes.keys()] }; break;
        case 'scene.query': result = getScene(params).query(params); break;
        case 'scene.update': result = getScene(params).query({ id: params.id }); break;
        case 'state.list': result = { states: [...states.keys()] }; break;
        case 'state.get': result = { name: params.name, value: getState(params.name) }; break;
        case 'action.list': {
          const all = [...actions].map(([name, { description, inputSchema }]) => ({ name, description, inputSchema }));
          const offset = params.offset ?? 0;
          const entries = all.slice(offset, offset + (params.limit ?? 50));
          result = { actions: entries, total: all.length, nextOffset: offset + entries.length < all.length ? offset + entries.length : null };
          break;
        }
        case 'action.call': result = { name: params.name, value: active.actionValue, ...await observe(params.observe) }; break;
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
