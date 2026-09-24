const Peregrust = (globalThis as any).Peregrust;

Promise.reject(new Error('REJECTION_ERROR_SENTINEL'));
Peregrust.onFrame(() => {});
