// Optional standalone check; requires a working audio output device.
const Peregrust = (globalThis as any).Peregrust;

const clip = await Peregrust.audio.load('assets/tone.wav');
const voice = clip.play({ volume: 0, loop: true });
await new Promise<void>((resolve) => setTimeout(resolve, 100));
const position = voice.info().positionSeconds;
if (!(position > 0.01)) throw new Error(`audio position did not advance: ${position}`);
voice.pause();
voice.resume();
voice.stop();
voice.dispose();
clip.unload();
console.log('AUDIO_OUTPUT_OK');
Peregrust.exit(0);
