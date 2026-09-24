import { Peregrust } from 'peregrust';

Peregrust.setTitle('Peregrust TypeScript smoke test');
console.log(`Canvas: ${Peregrust.canvas.width} × ${Peregrust.canvas.height}`);

Peregrust.onFrame((timestampMs: number) => {
  console.log(`Frame ${Peregrust.frameCount} at ${timestampMs.toFixed(1)} ms`);
  if (Peregrust.frameCount === 3) Peregrust.exit(0);
});
