// Node.js client for agents and tests. Never imported by the embedded game runtime.
import { readFile, writeFile } from 'node:fs/promises';
import { createConnection } from 'node:net';
import { resolve } from 'node:path';

export class ControlError extends Error {
  constructor(response) {
    super(response.error?.message ?? 'Control request failed');
    this.name = 'ControlError';
    this.code = response.error?.code ?? 'CONTROL_ERROR';
    this.response = response;
  }
}

export class PeregrustClient {
  #session;
  #port;
  constructor(session) {
    const address = /^127\.0\.0\.1:(\d+)$/.exec(session.address ?? '');
    if (session.version !== 1 || !address || !session.token || !session.sessionId) throw new Error('Invalid local Peregrust session');
    const port = Number(address[1]);
    if (port < 1 || port > 65535) throw new Error('Invalid session port');
    this.#session = { ...session };
    this.#port = port;
  }

  static async connect(sessionFile) {
    return new PeregrustClient(JSON.parse(await readFile(sessionFile, 'utf8')));
  }

  get sessionId() { return this.#session.sessionId; }

  async call(method, params = {}, { timeoutMs = 10000 } = {}) {
    if (typeof method !== 'string' || !method || !params || typeof params !== 'object' || Array.isArray(params)) throw new TypeError('Expected method and object params');
    if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 60000) throw new RangeError('timeoutMs must be 1–60000');
    const request = JSON.stringify({ token: this.#session.token, method, params, timeoutMs }) + '\n';
    if (Buffer.byteLength(request) > 1024 * 1024) throw new RangeError('Request exceeds 1 MiB');
    const response = await new Promise((done, reject) => {
      const socket = createConnection({ host: '127.0.0.1', port: this.#port });
      let size = 0, complete = false;
      const chunks = [];
      const timer = setTimeout(() => finish(new ControlError({ ok: false, error: {
        code: 'TIMEOUT', message: 'Client timeout; an action may already have taken effect',
      } })), timeoutMs + 5000);
      const finish = (error, value) => {
        if (complete) return;
        complete = true;
        clearTimeout(timer); socket.destroy();
        if (error) reject(error); else done(value);
      };
      socket.on('connect', () => socket.end(request));
      socket.on('error', (error) => finish(error));
      socket.on('data', (bytes) => {
        const newline = bytes.indexOf(10);
        const part = newline < 0 ? bytes : bytes.subarray(0, newline);
        size += part.length;
        if (size > 32 * 1024 * 1024) return finish(new RangeError('Response exceeds 32 MiB'));
        chunks.push(part);
        if (newline >= 0) {
          try { finish(null, JSON.parse(Buffer.concat(chunks).toString('utf8'))); }
          catch (error) { finish(error); }
        }
      });
      socket.on('close', () => { if (!complete) finish(new Error('Control connection closed without a complete response')); });
    });
    if (response.sessionId !== this.sessionId) throw new Error('Response session mismatch');
    if (response.ok !== true) throw new ControlError(response);
    return response;
  }

  /** Ordered operations, stopping at the first error. This is not a transaction. */
  async batch(operations, options) {
    const results = [];
    for (const { method, params } of operations) results.push(await this.call(method, params, options));
    return results;
  }

  async capture(params = {}, output, options) {
    const response = await this.call('frame.capture', params, options);
    if (output) {
      const capture = response.result.capture;
      await writeFile(output, Buffer.from(capture.data, 'base64'));
      delete capture.data;
      capture.path = resolve(output);
    }
    return response;
  }
}

export const connect = (sessionFile) => PeregrustClient.connect(sessionFile);
