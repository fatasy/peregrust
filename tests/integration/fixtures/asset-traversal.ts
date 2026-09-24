const Peregrust = (globalThis as any).Peregrust;

let rejected = false;
try {
  await Peregrust.assets.read('../Cargo.toml');
} catch (error) {
  rejected = /inside the project|outside the project|asset path/.test(String(error));
}
if (!rejected) throw new Error('asset traversal was accepted');
console.log('ASSET_TRAVERSAL_OK');
Peregrust.onFrame(() => Peregrust.exit(0));
