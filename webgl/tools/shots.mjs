#!/usr/bin/env node
/* =============================================================================
   shots.mjs — the pictures that go in docs/, made without a browser.

   This sandbox has no browser and no GL, so there is no screenshot to take.
   What it does have is the tables: the claims list, the palette ramps and the
   exact post-processing the composite pass applies to the evidence overlay.
   This tool draws those with the same numbers, at the same coordinates, and
   writes real PNGs — plus it checks that the JS mirror of the ramps still
   matches the GLSL function it is supposed to mirror.

   It is not a screenshot and does not pretend to be: it is the overlay and the
   palettes, drawn by a tool, for the README.

   Usage: node tools/shots.mjs
   ============================================================================= */
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { deflateSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { CLAIMS, RAMPS } from '../js/params.js';
import { rampRGB as sharedRampRGB, RAMP_STOPS } from '../js/ramps.js';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const docs = join(root, '..', 'docs');

/* ------------------------------------------------------------------ PNG ------ */
const CRC = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
    t[n] = c;
  }
  return t;
})();
function crc32(buf) {
  let c = -1;
  for (let i = 0; i < buf.length; i++) c = CRC[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}
function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, 'ascii');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
}
export function writePNG(path, w, h, rgb) {
  const stride = w * 3;
  const raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0;
    for (let x = 0; x < w; x++) {
      const s = (y * w + x) * 3, d = y * (stride + 1) + 1 + x * 3;
      raw[d] = rgb[s]; raw[d + 1] = rgb[s + 1]; raw[d + 2] = rgb[s + 2];
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; ihdr[9] = 2; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
  writeFileSync(path, png);
  return png.length;
}

/* the ramp lives in js/ramps.js so the legend and this tool cannot disagree;
   what is checked here is that the JS stops still match the GLSL literals */
function rampRGB(name, t) { return sharedRampRGB(name, t); }
const smoothstep = (a, b, x) => {
  const t = Math.max(0, Math.min(1, (x - a) / (b - a || 1e-6)));
  return t * t * (3 - 2 * t);
};

function checkRamps() {
  const src = readFileSync(join(root, 'js', 'shaders', 'lib.glsl'), 'utf8');
  const body = src.slice(src.indexOf('vec3 ramp(int id, float t) {'));
  let checked = 0, drift = 0;
  for (const name of RAMPS) {
    if (name === 'spectral') { continue; }   /* built from hsv2rgb, not from stops */
    const re = new RegExp(`RAMP_${name.toUpperCase()}\\b[^;]*?ramp(?:10|6)\\(t,([^;]*)\\);`, 's');
    const m = body.match(re);
    if (!m) { console.log(`  · ${name}: no literal stop list in lib.glsl (skipped)`); continue; }
    const glsl = (m[1].match(/vec3\(([^)]*)\)/g) || []).map(v => v.slice(5, -1).split(',').map(Number));
    const js = RAMP_STOPS[name];
    if (!js || glsl.length !== js.length) { console.log(`  ✗ ${name}: ${glsl.length} GLSL stops vs ${js ? js.length : 0} JS stops`); drift++; continue; }
    for (let i = 0; i < glsl.length; i++) {
      for (let k = 0; k < 3; k++) if (Math.abs(glsl[i][k] - js[i][k]) > 1e-6) {
        console.log(`  ✗ ${name}[${i}][${k}]: GLSL ${glsl[i][k]} vs JS ${js[i][k]}`); drift++;
      }
    }
    checked++;
  }
  console.log(`ramp mirror: ${checked}/${RAMPS.length} ramps compared against lib.glsl, ${drift} value(s) differ`);
  return drift;
}

/* ---------------------------------------------------- the evidence mosaic --- */
/* Mirrors evidenceMosaic() in js/shaders/composite.glsl, parameter for
   parameter. Defaults come from js/params.js (u_mosaic, u_mosaicCells, ...). */
function drawMosaic(W, H, opts) {
  const {
    cols = 29, gap = 0.06, mosaic = 0.86, rampName = 'magma', epoch = 2023, span = 8, band = 0.6,
  } = opts || {};
  const rows = Math.max(1, Math.round(cols * 9 / 16));
  const px = new Uint8Array(W * H * 3);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      /* the base is the scene the overlay sits on: a clay ground under a cool
         sky, with a low relief running across it */
      const u = x / W, v = y / H;
      const relief = 0.5 + 0.5 * Math.sin(u * 7.0 + 1.3 * Math.sin(v * 5.0)) * Math.sin(v * 4.0 + 0.7);
      const sky = Math.pow(Math.max(0, 1 - v), 1.6);
      const base = [
        0.055 + 0.10 * sky + 0.05 * relief * (1 - sky),
        0.045 + 0.11 * sky + 0.038 * relief * (1 - sky),
        0.045 + 0.17 * sky + 0.026 * relief * (1 - sky),
      ];
      /* the overlay works in uv space with y up, as GLSL does */
      const gu = u, gv = 1 - v;
      const gx = gu * cols, gy = gv * rows;
      const ciX = Math.floor(gx), ciY = Math.floor(gy);
      const cfX = gx - ciX, cfY = gy - ciY;
      const idx = ((ciX + ciY * cols) % CLAIMS.length + CLAIMS.length) % CLAIMS.length;
      const c = CLAIMS[idx];
      const catIdx = ['material', 'bio', 'geo', 'cognitive', 'tech', 'eco'].indexOf(c.cat);
      const cat = Math.max(0, catIdx) / 5;
      const col = rampRGB(rampName, cat);
      const conf = /measured|observed/.test(c.lvl) ? 2 : (/contested|mixed|emerging|early|suggestive|single|anecdotal/.test(c.lvl) ? 0 : 1);
      const hatch = 0.5 + 0.5 * Math.sin((cfX + cfY) * 26.0);
      const fill = conf === 2 ? 0.85 : (conf === 1 ? 0.45 + 0.25 * hatch : 0.10 + 0.42 * hatch);
      const mid = (c.band[0] + c.band[1]) * 0.5;
      const f = Math.max(0, Math.min(1, (mid - (epoch - span * 0.5)) / Math.max(span, 0.001)));
      const bandLine = smoothstep(0.035, 0.0, Math.abs(cfY - (0.08 + 0.84 * f)));
      const spread = Math.max(0, Math.min(1, (c.band[1] - c.band[0]) / Math.max(span, 0.001)));
      const inside = (cfX >= gap && cfX <= 1 - gap && cfY >= gap && cfY <= 1 - gap) ? 1 : 0;
      const line = (cfX <= gap * 0.35 || cfX >= 1 - gap * 0.35 || cfY <= gap * 0.35 || cfY >= 1 - gap * 0.35) ? 1 : 0;
      const cell = [0, 1, 2].map(k => Math.max(
        col[k] * fill * inside + col[k] * bandLine * inside * (0.6 + 1.4 * band) + col[k] * inside * spread * 0.07,
        0.35 * line * 0.5));
      const px3 = [0, 1, 2].map(k => base[k] * 0.25 + cell[k] * 1.15);
      const mixed = [0, 1, 2].map(k => base[k] * (1 - mosaic) + px3[k] * mosaic);
      const s = (y * W + x) * 3;
      for (let k = 0; k < 3; k++) px[s + k] = Math.max(0, Math.min(255, Math.round(mixed[k] * 255)));
    }
  }
  return px;
}

/* -------------------------------------------------------- the ramp chart --- */
function drawRamps(W, H) {
  const px = new Uint8Array(W * H * 3);
  const pad = 14, rowH = Math.floor((H - pad * 2) / RAMPS.length);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      const s = (y * W + x) * 3;
      const row = Math.floor((y - pad) / rowH);
      px[s] = 10; px[s + 1] = 12; px[s + 2] = 16;
      if (row >= 0 && row < RAMPS.length && x >= pad && x < W - pad) {
        const t = (x - pad) / (W - pad * 2 - 1);
        const c = rampRGB(RAMPS[row], t);
        px[s] = Math.round(c[0] * 255); px[s + 1] = Math.round(c[1] * 255); px[s + 2] = Math.round(c[2] * 255);
      }
      /* a hairline between rows, so the twelve are countable in a screenshot */
      if (row >= 0 && row < RAMPS.length && y - pad - row * rowH < 2) { px[s] = 40; px[s + 1] = 44; px[s + 2] = 52; }
    }
  }
  return px;
}

/* ------------------------------------------------------------------ main ---- */
mkdirSync(docs, { recursive: true });
const drift = checkRamps();
const mosaic = drawMosaic(1200, 675, {});
const mSize = writePNG(join(docs, 'webgl-mosaic.png'), 1200, 675, mosaic);
console.log(`docs/webgl-mosaic.png — ${mSize} bytes, 1200×675, ${CLAIMS.length} claims tiled`);
const ramps = drawRamps(760, 500);
const rSize = writePNG(join(docs, 'webgl-ramps.png'), 760, 500, ramps);
console.log(`docs/webgl-ramps.png — ${rSize} bytes, 760×500, ${RAMPS.length} ramps`);
for (const [i, c] of CLAIMS.entries()) {
  console.log(`  ${String(i + 1).padStart(2)}  ${c.band[0]}–${c.band[1]}  ${c.cat.padEnd(10)} ${c.lvl.padEnd(18)} ${c.t.slice(0, 88)}`);
}
process.exit(drift ? 1 : 0);
