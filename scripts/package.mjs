import { copyFile, cp, mkdir, readFile, writeFile, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const extension = process.platform === 'win32' ? '.exe' : '';
const binary = resolve(root, process.env.PEREGRUST_BINARY ?? `target/release/peregrust${extension}`);
await stat(binary);
const version = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).version;
const destination = resolve(root, process.argv[2] ?? `artifacts/peregrust-${version}-${process.platform}-${process.arch}`);
// A package directory is a new artifact. Do not delete or overwrite an older one.
await mkdir(dirname(destination), { recursive: true });
await mkdir(destination, { recursive: false });
const build = spawnSync(process.execPath, ['scripts/build.mjs', 'examples/three-demo.js', 'dist/three-demo.js'], {
  cwd: root, stdio: 'inherit', windowsHide: true,
});
if (build.status !== 0) throw new Error('Could not build the packaged demo');
await mkdir(join(destination, 'dist'));
await copyFile(binary, join(destination, `peregrust${extension}`));
await copyFile(join(root, 'dist/three-demo.js'), join(destination, 'dist/three-demo.js'));
await copyFile(join(root, 'dist/three-demo.js.map'), join(destination, 'dist/three-demo.js.map'));
await cp(join(root, 'assets'), join(destination, 'assets'), { recursive: true });
for (const name of ['README.md', 'LICENSE-MIT', 'Cargo.lock', 'package-lock.json']) {
  await copyFile(join(root, name), join(destination, name));
}
await cp(join(root, 'docs'), join(destination, 'docs'), { recursive: true });
await mkdir(join(destination, 'licenses'));
await copyFile(join(root, 'node_modules/three/LICENSE'), join(destination, 'licenses/three-MIT.txt'));
await writeFile(join(destination, 'NOTICE.txt'),
  'Peregrust includes Rust dependencies listed in Cargo.lock and JavaScript dependencies listed in package-lock.json.\n' +
  'Their licenses remain with their respective authors. The Three.js license and bundle license comments are retained.\n' +
  'This artifact is a local validation package. Review all transitive license notices before external distribution.\n');
if (process.platform === 'win32') {
  await writeFile(join(destination, 'play.cmd'), '@echo off\r\n"%~dp0peregrust.exe" dist/three-demo.js --root "%~dp0." %*\r\n');
} else {
  await writeFile(join(destination, 'play.sh'), '#!/bin/sh\nset -eu\nbase=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$base/peregrust" dist/three-demo.js --root "$base" "$@"\n', { mode: 0o755 });
}
const bytes = await readFile(binary);
await writeFile(join(destination, 'build.json'), JSON.stringify({
  version, platform: process.platform, arch: process.arch,
  binary: `peregrust${extension}`, sha256: createHash('sha256').update(bytes).digest('hex'),
  bytes: bytes.length, packagedAt: new Date().toISOString(),
}, null, 2));
console.log(destination);
