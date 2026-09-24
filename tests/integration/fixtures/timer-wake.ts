const Peregrust = (globalThis as any).Peregrust;

setTimeout(() => {
  console.log('TIMER_WAKE_OK');
  Peregrust.exit(0);
}, 80);
