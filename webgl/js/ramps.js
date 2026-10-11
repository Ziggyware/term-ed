/* =============================================================================
   ramps.js — the colour ramps, once.

   lib.glsl defines them for the GPU. Everything on the CPU that needs the same
   gradient — the legend swatch, the palette chart in docs/, the palette source
   in the panel — reads them from here instead of keeping a second copy that
   drifts. tools/validate.mjs compares this table against the literals in
   lib.glsl and complains if the two ever disagree.
   ============================================================================= */
'use strict';

/* Stop lists, in RAMP_* order. turbo … cividis have ten stops; the rest have six,
   exactly as lib.glsl builds them. */
export const RAMP_STOPS = {
  turbo: [[0.19, 0.07, 0.23], [0.25, 0.27, 0.67], [0.27, 0.46, 0.93], [0.22, 0.63, 0.99], [0.11, 0.81, 0.83],
    [0.24, 0.94, 0.55], [0.62, 0.99, 0.24], [0.98, 0.73, 0.22], [0.96, 0.39, 0.12], [0.48, 0.02, 0.01]],
  magma: [[0, 0, 0.02], [0.09, 0.06, 0.24], [0.27, 0.06, 0.46], [0.45, 0.12, 0.51], [0.62, 0.18, 0.5],
    [0.8, 0.25, 0.44], [0.95, 0.38, 0.36], [0.99, 0.59, 0.41], [1, 0.79, 0.55], [0.99, 0.99, 0.75]],
  inferno: [[0, 0, 0.02], [0.11, 0.05, 0.25], [0.29, 0.05, 0.42], [0.47, 0.11, 0.43], [0.65, 0.17, 0.38],
    [0.81, 0.27, 0.27], [0.93, 0.41, 0.15], [0.98, 0.61, 0.02], [0.97, 0.82, 0.24], [0.99, 1, 0.64]],
  plasma: [[0.05, 0.03, 0.53], [0.27, 0.01, 0.62], [0.45, 0, 0.66], [0.61, 0.09, 0.62], [0.74, 0.22, 0.53],
    [0.85, 0.34, 0.42], [0.93, 0.47, 0.33], [0.98, 0.62, 0.23], [0.99, 0.79, 0.15], [0.94, 0.98, 0.13]],
  viridis: [[0.27, 0, 0.33], [0.28, 0.15, 0.47], [0.25, 0.29, 0.54], [0.19, 0.4, 0.56], [0.15, 0.51, 0.56],
    [0.12, 0.62, 0.54], [0.42, 0.79, 0.42], [0.71, 0.87, 0.17], [0.83, 0.9, 0.18], [0.99, 0.91, 0.15]],
  cividis: [[0, 0.13, 0.3], [0, 0.2, 0.44], [0.22, 0.28, 0.42], [0.34, 0.36, 0.43], [0.44, 0.44, 0.45],
    [0.54, 0.53, 0.47], [0.65, 0.62, 0.46], [0.77, 0.71, 0.42], [0.89, 0.81, 0.36], [1, 0.92, 0.27]],
  earth: [[0.1, 0.07, 0.05], [0.29, 0.2, 0.13], [0.44, 0.32, 0.2], [0.61, 0.47, 0.31], [0.79, 0.68, 0.51], [0.94, 0.92, 0.85]],
  holo: [[0.03, 0.05, 0.16], [0.11, 0.35, 0.72], [0.16, 0.72, 0.83], [0.45, 0.95, 0.75], [0.91, 0.86, 0.97], [0.62, 0.35, 0.94]],
  ember: [[0.03, 0.01, 0.01], [0.23, 0.03, 0.02], [0.51, 0.1, 0.02], [0.8, 0.3, 0.04], [0.97, 0.62, 0.15], [1, 0.96, 0.78]],
  patina: [[0.04, 0.09, 0.08], [0.09, 0.28, 0.24], [0.22, 0.52, 0.42], [0.55, 0.74, 0.58], [0.9, 0.8, 0.63], [0.74, 0.51, 0.35]],
  niche: [[0.07, 0.09, 0.28], [0.24, 0.33, 0.55], [0.51, 0.62, 0.71], [0.82, 0.79, 0.66], [0.86, 0.62, 0.52], [0.42, 0.18, 0.3]],
};

/* hsv → rgb, the same formula lib.glsl uses for the spectral ramp and the
   orientation maps. */
export function hsv2rgb(h, s, v) {
  const k = (n) => (n + h * 6) % 6;
  const f = (n) => v - v * s * Math.max(0, Math.min(1, Math.min(k(n), 4 - k(n))));
  return [f(5), f(3), f(1)];
}

/* ramp(name|index, t) → [r,g,b] in 0…1, exactly as lib.glsl computes it:
   smoothstepped between stops, with spectral built from an HSV sweep. */
export function rampRGB(name, t) {
  const x = Math.max(0, Math.min(1, t));
  if (name === 'spectral') {
    const h = 0.68 + (0 - 0.68) * x;
    const v = 0.45 + 0.55 * (0.5 + 0.5 * Math.cos(Math.PI * 2 * (x - 0.25)));
    return hsv2rgb(h, 0.72, Math.max(0, Math.min(1, v)));
  }
  const stops = RAMP_STOPS[name] || RAMP_STOPS.niche;
  const p = x * (stops.length - 1);
  const i = Math.min(stops.length - 2, Math.floor(p));
  const f = p - i;
  const s = f * f * (3 - 2 * f);
  return [0, 1, 2].map(k => stops[i][k] + (stops[i + 1][k] - stops[i][k]) * s);
}
export function rampCss(name, t) {
  const c = rampRGB(name, t);
  return 'rgb(' + c.map(v => Math.round(Math.max(0, Math.min(1, v)) * 255)).join(',') + ')';
}
