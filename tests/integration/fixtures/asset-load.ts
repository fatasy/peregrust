const Peregrust = (globalThis as any).Peregrust;

const image = await Peregrust.assets.decodeImage('assets/checker.png');
const glb = await Peregrust.assets.read('assets/spinner.glb');
if (image.width !== 32 || image.height !== 32 || image.data.length !== 32 * 32 * 4) {
  throw new Error('decoded image shape is wrong');
}
if (new TextDecoder().decode(glb.subarray(0, 4)) !== 'glTF') {
  throw new Error('GLB magic is wrong');
}
console.log('ASSET_LOAD_OK');
Peregrust.onFrame(() => Peregrust.exit(0));
