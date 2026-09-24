/** The host installs the runtime before evaluating an application bundle. */
export const Peregrust = globalThis.Peregrust;
export const canvas = Peregrust?.canvas;
export const window = Peregrust?.window;

if (!Peregrust) {
  throw new Error('Peregrust is available only inside the native runtime');
}
