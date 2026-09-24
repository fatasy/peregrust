const Peregrust = (globalThis as any).Peregrust;

const clip = await Peregrust.audio.load('assets/tone.wav');
if (!(clip.durationSeconds > 0.2 && clip.durationSeconds < 0.3)) {
  throw new Error(`audio duration is wrong: ${clip.durationSeconds}`);
}
clip.unload();
console.log('AUDIO_LOAD_OK');
Peregrust.onFrame(() => Peregrust.exit(0));
