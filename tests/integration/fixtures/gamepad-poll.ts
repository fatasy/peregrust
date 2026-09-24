const Peregrust = (globalThis as any).Peregrust;

const pads = navigator.getGamepads();
if (!Array.isArray(pads)) throw new Error('gamepad poll did not return an array');
if (!Array.isArray(Peregrust.gamepads.poll())) throw new Error('SDK gamepad poll did not return an array');
console.log('GAMEPAD_POLL_OK');
Peregrust.onFrame(() => Peregrust.exit(0));
