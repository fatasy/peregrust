import { build } from 'esbuild';
import { mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';

const entryPoint = resolve(process.argv[2] ?? 'examples/three-demo.js');
const outfile = resolve(process.argv[3] ?? 'dist/three-demo.js');
await mkdir(dirname(outfile), { recursive: true });

await build({
  entryPoints: [entryPoint],
  outfile,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  target: 'es2022',
  conditions: ['browser', 'module'],
  plugins: [{
    name: 'three-webgpu-single-instance',
    setup(pluginBuild) {
      pluginBuild.onResolve({ filter: /^three$/ }, () => ({
        path: resolve('node_modules/three/build/three.webgpu.js'),
      }));
    },
  }],
  sourcemap: 'external',
  legalComments: 'eof',
  logLevel: 'info',
});
