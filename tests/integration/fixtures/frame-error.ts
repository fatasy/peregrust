const Peregrust = (globalThis as any).Peregrust;

Peregrust.onFrame(() => {
  throw new Error('FRAME_ERROR_SENTINEL');
});
