import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const executable = resolve(projectRoot,
  process.env.PEREGRUST_BINARY ?? `target/debug/peregrust${process.platform === 'win32' ? '.exe' : ''}`);
if (!existsSync(executable)) {
  throw new Error(`Peregrust binary is missing: ${executable}. Build it with cargo build first.`);
}

function run(command, args, deadlineMs) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, {
      cwd: projectRoot,
      env: process.env,
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill();
    }, deadlineMs);
    child.stdout.setEncoding('utf8').on('data', (chunk) => { output += chunk; });
    child.stderr.setEncoding('utf8').on('data', (chunk) => { output += chunk; });
    child.on('error', (error) => {
      clearTimeout(timer);
      rejectRun(error);
    });
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      resolveRun({ code, signal, timedOut, output });
    });
  });
}

const build = await run(process.execPath,
  ['scripts/build.mjs', 'examples/gpu-proof.js', 'dist/gpu-proof.js'], 30000);
if (build.code !== 0 || build.timedOut) {
  throw new Error(`GPU proof bundle failed:\n${build.output}`);
}

const cases = [
  { name: 'native upload batches, source snapshots, readback and validation scopes', entry: 'tests/integration/fixtures/gpu-upload-batch.js', marker: 'GPU_UPLOAD_BATCH_OK', success: true, frames: null },
  { name: 'creation-mapped buffers start zeroed and write back', entry: 'tests/integration/fixtures/gpu-mapped-at-creation.js', marker: 'GPU_MAPPED_AT_CREATION_OK', success: true, frames: null },
  { name: 'shutdown with an acquired unpresented GPU frame', entry: 'tests/integration/fixtures/exit-acquired-frame.ts', marker: 'EXIT_ACQUIRED_FRAME_OK', success: true, frames: null },
  { name: 'TypeScript imports and top-level await', entry: 'tests/integration/fixtures/tla-import.ts', marker: 'TLA_IMPORT_OK', success: true, frames: 3 },
  { name: 'timer wake without animation frames', entry: 'tests/integration/fixtures/timer-wake.ts', marker: 'TIMER_WAKE_OK', success: true, frames: null },
  { name: 'asynchronous frame completion', entry: 'tests/integration/fixtures/async-frame.ts', marker: 'ASYNC_FRAME_OK', success: true, frames: 3 },
  { name: 'unresponsive script timeout', entry: 'tests/integration/fixtures/infinite-loop.ts', marker: 'INFINITE_LOOP_START', expectedCode: 124, frames: null, timeoutSeconds: 0.5, deadlineMs: 10000 },
  { name: 'asynchronous asset loading', entry: 'tests/integration/fixtures/asset-load.ts', marker: 'ASSET_LOAD_OK', success: true, frames: 2 },
  { name: 'audio decoding without an output device', entry: 'tests/integration/fixtures/audio-load.ts', marker: 'AUDIO_LOAD_OK', success: true, frames: 2 },
  { name: 'gamepad polling', entry: 'tests/integration/fixtures/gamepad-poll.ts', marker: 'GAMEPAD_POLL_OK', success: true, frames: 2 },
  { name: 'asset root enforcement', entry: 'tests/integration/fixtures/asset-traversal.ts', marker: 'ASSET_TRAVERSAL_OK', success: true, frames: 2 },
  { name: 'Three.js WebGPU and unmodified GLTFLoader pixel readback', entry: 'dist/gpu-proof.js', marker: 'GPU_PROOF_OK', extraMarker: 'GLTF_NATIVE_LOADER_OK', success: true, frames: 3 },
  { name: 'native image upload and canvas presentation', entry: 'examples/web-assets-gpu-proof.js', marker: 'canvas 320x180 frame submitted', success: true, frames: null, presentedFrames: 1 },
  { name: 'empty callbacks do not satisfy presentation limit', entry: 'tests/integration/fixtures/empty-frames.ts', marker: 'EMPTY_FRAMES_START', expectedCode: 124, frames: null, presentedFrames: 1, timeoutSeconds: 0.5, deadlineMs: 10000 },
  { name: 'startup error propagation', entry: 'tests/integration/fixtures/startup-error.ts', marker: 'STARTUP_ERROR_SENTINEL', success: false, frames: 1 },
  { name: 'frame error propagation', entry: 'tests/integration/fixtures/frame-error.ts', marker: 'FRAME_ERROR_SENTINEL', success: false, frames: 2 },
  { name: 'unhandled rejection propagation', entry: 'tests/integration/fixtures/rejection-error.ts', marker: 'REJECTION_ERROR_SENTINEL', success: false, frames: 2 },
];

let failed = 0;
for (const entry of cases) {
  const args = [
    entry.entry,
    '--root', projectRoot,
    '--hidden',
    '--timeout', String(entry.timeoutSeconds ?? 60),
  ];
  if (entry.frames !== null) args.push('--frames', String(entry.frames));
  if (entry.presentedFrames) args.push('--presented-frames', String(entry.presentedFrames));
  const result = await run(executable, args, entry.deadlineMs ?? 75000);
  const rightExit = entry.expectedCode !== undefined
    ? result.code === entry.expectedCode
    : entry.success ? result.code === 0 : result.code !== 0 && result.code !== null;
  const passed = !result.timedOut && rightExit && result.output.includes(entry.marker)
    && (!entry.extraMarker || result.output.includes(entry.extraMarker));
  if (passed) {
    console.log(`PASS ${entry.name}`);
  } else {
    failed++;
    console.error(`FAIL ${entry.name}: exit=${result.code} signal=${result.signal} timeout=${result.timedOut}`);
    if (result.output.length <= 6000) console.error(result.output);
    else console.error(`${result.output.slice(0, 3500)}\n... backtrace truncated ...\n${result.output.slice(-2500)}`);
  }
}
if (failed) process.exitCode = 1;
