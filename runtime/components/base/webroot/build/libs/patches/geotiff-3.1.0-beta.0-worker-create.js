// Replaces geotiff 3.1.0-beta.0's dist-module/worker/create.js in the OpenLayers bundle (build/libs/refresh.mjs,
// GEOTIFF_WORKER_CREATE; plans/Maps.md P4c). The original makes geotiff's decoder worker from a string of every decoder
// again (LERC's and Zstandard's WebAssembly as base64 among them) through a Blob URL, and imports web-worker for Node. Here
// the worker is the one m-map sets through the raster part's setWorkerFactory: ol-tiff-decoder, a module worker of this
// server whose decoders are the chunks the page shares, each loaded only when a tile of its compression is decoded.
let factory = null;

// The function that makes a decoder worker (the raster part sets it when it loads).
export function setWorkerFactory(make) {
  factory = make;
}

export default function create() {
  if (!factory) throw new Error('geotiff: no decoder worker is set');
  return factory();
}
