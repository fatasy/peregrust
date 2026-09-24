import { importedValue } from './imported.ts';

const Peregrust = (globalThis as any).Peregrust;

await new Promise<void>((resolve) => setTimeout(resolve, 10));
if (importedValue !== 42) throw new Error('module import returned wrong value');
console.log('TLA_IMPORT_OK');
Peregrust.onFrame(() => {
  if (Peregrust.frameCount >= 2) Peregrust.exit(0);
});
