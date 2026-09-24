const Peregrust = (globalThis as any).Peregrust;

let inFlight = false;
let completed = 0;
Peregrust.onFrame(async () => {
  if (inFlight) throw new Error('asynchronous frames overlapped');
  inFlight = true;
  try {
    const [image] = await Promise.all([
      Peregrust.assets.decodeImage('assets/checker.png'),
      new Promise<void>((resolve) => setTimeout(resolve, 20)),
    ]);
    if (image.width !== 32 || image.height !== 32) throw new Error('async frame image decode failed');
    completed++;
    if (completed !== Peregrust.frameCount) throw new Error('frame count advanced before async work completed');
    if (completed === 3) {
      console.log('ASYNC_FRAME_OK');
      Peregrust.exit(0);
    }
  } finally {
    inFlight = false;
  }
});
