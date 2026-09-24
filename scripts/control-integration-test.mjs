import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { inflateSync } from 'node:zlib';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const executable = resolve(root, process.env.PEREGRUST_BINARY ?? `target/debug/peregrust${process.platform === 'win32' ? '.exe' : ''}`);
const artifacts = resolve(root, 'artifacts');
mkdirSync(artifacts, { recursive: true });
const directory = mkdtempSync(resolve(artifacts, 'control-'));
const session = resolve(directory, 'session.json');
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

function run(command, args, timeoutMs = 30000) {
  return new Promise((done, reject) => {
    const child = spawn(command, args, { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '';
    const timer = setTimeout(() => { child.kill(); reject(new Error(`command timed out: ${args.join(' ')}`)); }, timeoutMs);
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', (error) => { clearTimeout(timer); reject(error); });
    child.on('close', (code) => { clearTimeout(timer); done({ code, stdout, stderr }); });
  });
}

// Decode the encoder's RGBA8 PNG output to verify real GPU colors and orientation.
function decodePng(path) {
  const bytes = readFileSync(path);
  assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  const width = bytes.readUInt32BE(16), height = bytes.readUInt32BE(20);
  assert.equal(bytes[24], 8); assert.equal(bytes[25], 6);
  const chunks = [];
  for (let cursor = 8; cursor < bytes.length;) {
    const length = bytes.readUInt32BE(cursor);
    if (bytes.toString('ascii', cursor + 4, cursor + 8) === 'IDAT') chunks.push(bytes.subarray(cursor + 8, cursor + 8 + length));
    cursor += length + 12;
  }
  const raw = inflateSync(Buffer.concat(chunks)), pixels = Buffer.alloc(width * height * 4), stride = width * 4;
  const paeth = (a, b, c) => {
    const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  };
  for (let y = 0; y < height; y++) {
    const type = raw[y * (stride + 1)];
    assert.ok(type <= 4);
    for (let x = 0; x < stride; x++) {
      const i = y * stride + x, left = x >= 4 ? pixels[i - 4] : 0;
      const up = y ? pixels[i - stride] : 0, upperLeft = y && x >= 4 ? pixels[i - stride - 4] : 0;
      const prediction = [0, left, up, Math.floor((left + up) / 2), paeth(left, up, upperLeft)][type];
      pixels[i] = (raw[y * (stride + 1) + 1 + x] + prediction) & 255;
    }
  }
  return { width, height, pixel: (x, y) => [...pixels.subarray((y * width + x) * 4, (y * width + x) * 4 + 4)] };
}

const build = await run(process.execPath, ['scripts/build.mjs', 'examples/control-demo.js', 'dist/control-demo.js']);
assert.equal(build.code, 0, build.stderr);
const game = spawn(executable, ['dist/control-demo.js', '--root', root, '--hidden', '--width', '320', '--height', '240',
  '--fps', '60', '--timeout', '90', '--control', session], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
let log = '';
game.stdout.on('data', (chunk) => { log += chunk; });
game.stderr.on('data', (chunk) => { log += chunk; });
const exited = new Promise((done, reject) => { game.on('close', done); game.on('error', reject); });
let requestId = 0;
async function ctl(method, params = {}, extra = [], expectedCode = 0) {
  const paramsFile = resolve(directory, `params-${requestId++}.json`);
  writeFileSync(paramsFile, JSON.stringify(params));
  const result = await run(executable, ['ctl', '--session', session, method, '--params-file', paramsFile, ...extra]);
  assert.equal(result.code, expectedCode, `${method}: ${result.stdout}\n${result.stderr}\n${log}`);
  return JSON.parse(result.stdout);
}

try {
  const deadline = Date.now() + 30000;
  while (!existsSync(session)) {
    if (game.exitCode !== null || Date.now() > deadline) throw new Error(`control session did not start:\n${log}`);
    await pause(50);
  }
  const description = await ctl('control.describe', {}, ['--timeout-ms', '20000']);
  assert.equal(description.result.protocolVersion, 1);
  const query = await ctl('scene.query', { name: 'player' });
  const id = query.result.objects[0].id;
  assert.equal(query.result.total, 1);
  assert.ok(query.frame > 0);
  const firstCapture = resolve(artifacts, 'control-before.png');
  await ctl('frame.capture', { width: 320, height: 240 }, ['--output', firstCapture]);
  const before = decodePng(firstCapture);
  assert.deepEqual([before.width, before.height], [320, 240]);
  assert.ok(before.pixel(160, 120)[0] > 200, `center must be red: ${before.pixel(160, 120)}`);
  assert.ok(before.pixel(10, 10)[2] > 200, 'background must be blue');
  assert.ok(before.pixel(48, 40)[1] > 200, `top-left marker must be green: ${before.pixel(48, 40)}`);
  assert.ok(before.pixel(48, 200)[2] > 200, 'bottom-left must remain blue (no vertical flip)');
  const updated = await ctl('scene.update', { id, position: [0.5, 0, 0] });
  assert.equal(updated.result.objects[0].position[0], 0.5);
  const observed = await ctl('input.key', { code: 'KeyW', key: 'w', frames: 10,
    observe: { query: { id }, state: 'player', capture: { width: 320, height: 240 } },
  }, ['--output', resolve(artifacts, 'control-after.png')]);
  assert.ok(Math.abs(observed.result.state.position[1] - 0.2) < 1e-8);
  assert.deepEqual(observed.result.state.held, []);
  assert.equal(observed.result.objects.objects[0].position[1], observed.result.state.position[1]);
  const after = decodePng(resolve(artifacts, 'control-after.png'));
  assert.ok(after.pixel(200, 104)[0] > 200, 'moved player must appear at its observed position');
  assert.ok(after.pixel(160, 120)[2] > 200, 'old player position must be empty');
  const again = await ctl('state.get', { name: 'player' });
  assert.deepEqual(again.result.value.position, observed.result.state.position);
  await ctl('input.dispatch', { event: { type: 'pointerdown', clientX: 10, clientY: 10, button: 0, buttons: 1 } });
  assert.equal((await ctl('state.get', { name: 'player' })).result.value.pointerEvents, 1);
  const invalid = await ctl('scene.update', { id, position: [9, 9, 9], visible: 'bad' }, [], 1);
  assert.equal(invalid.error.code, 'INVALID_ARGUMENT');
  assert.deepEqual((await ctl('state.get', { name: 'player' })).result.value.position, observed.result.state.position);
  assert.equal((await ctl('missing', {}, [], 1)).error.code, 'METHOD_NOT_FOUND');
  const timedOut = await ctl('input.key', { code: 'KeyD', key: 'd', frames: 600 }, ['--timeout-ms', '100'], 1);
  assert.equal(timedOut.error.code, 'TIMEOUT');
  assert.deepEqual((await ctl('state.get', { name: 'player' })).result.value.held, []);
  // Close through game input; a stopped response is allowed during shutdown.
  await ctl('input.dispatch', { event: { type: 'keydown', code: 'Escape', key: 'Escape' } }).catch(() => {});
  const code = await Promise.race([exited, pause(10000).then(() => { throw new Error('shutdown timed out'); })]);
  assert.equal(code, 0, log);
  assert.equal(existsSync(session), false, 'normal shutdown removes session credentials');
  console.log('PASS persistent CLI session, scene queries/updates, exact input duration, observations, errors, timeout recovery, native PNG GPU pixels/orientation and cleanup');
  console.log(`Captures: ${firstCapture}, ${resolve(artifacts, 'control-after.png')}`);
} finally {
  if (game.exitCode === null) { game.kill(); await exited; }
}
