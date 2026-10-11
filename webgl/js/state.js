/* =============================================================================
   state.js — the configuration, its defaults, its share links, and the clock.

   Nothing here touches the DOM or the GPU, so the whole configuration model can
   be exercised from node (tools/validate.mjs does exactly that).
   ============================================================================= */
'use strict';

import { GROUPS, MODES, MODE_DEFAULTS, PRESETS, CLAIMS, RAMPS } from './params.js';

export const keyOf = (g) => g.k || g.u;
export const paramByKey = new Map(GROUPS.map(g => [keyOf(g), g]));
export const paramsByUniform = new Map(GROUPS.filter(g => g.u).map(g => [g.u, g]));

export const DEFAULTS = (() => {
  const d = {};
  for (const g of GROUPS) d[keyOf(g)] = g.v;
  return d;
})();

/* ---------------------------------------------------------------- the state -- */
export class Config {
  constructor(init) {
    this.values = Object.assign({}, DEFAULTS, init || {});
    this.mode = (init && init.__mode) || MODES[0].id;
    this.prevMode = null;
    this.pulse = 0.0;
    this.pulsePrev = 0.0;
    this.dirty = true;
  }
  get(key) { return this.values[key]; }
  set(key, v) {
    const g = paramByKey.get(key);
    if (!g) return false;
    if (typeof v === 'string' && (g.t === 'F' || g.t === 'I')) v = parseFloat(v);
    if (g.t === 'F' || g.t === 'I') {
      const n = Number(v);
      if (!isFinite(n)) return false;
      v = Math.min(g.max !== undefined ? g.max : 1e9, Math.max(g.min !== undefined ? g.min : -1e9, n));
    }
    if (g.t === 'B') v = !!v;
    if (g.t === 'S' && g.o && g.o.indexOf(v) < 0) return false;
    if (this.values[key] === v) return false;
    this.values[key] = v;
    this.dirty = true;
    return true;
  }
  reset() {
    this.values = Object.assign({}, DEFAULTS);
    this.dirty = true;
  }
  setMode(id, opts) {
    if (!MODES.some(m => m.id === id)) return false;
    if (id === this.mode && !(opts && opts.force)) return false;
    this.prevMode = this.mode;
    this.mode = id;
    const d = MODE_DEFAULTS[id];
    if (d) for (const k in d) if (paramByKey.has(k)) this.values[k] = d[k];
    this.dirty = true;
    return true;
  }
  applyPreset(p) {
    let changed = 0;
    if (p.mode) { this.setMode(p.mode); changed++; }
    for (const k in (p.p || {})) {
      /* the preset may name either the uniform or the plain key */
      const key = paramByKey.has(k) ? k : (paramsByUniform.has(k) ? paramsByUniform.get(k).u : null);
      if (key && this.set(key, p.p[k])) changed++;
    }
    this.dirty = true;
    return changed;
  }
  /* ---- serialisation ---- */
  toObject() {
    const o = { __mode: this.mode, __v: 2 };
    for (const g of GROUPS) {
      const k = keyOf(g);
      const v = this.values[k];
      if (v !== DEFAULTS[k]) o[k] = v;
    }
    return o;
  }
  fromObject(o) {
    const errs = [];
    this.values = Object.assign({}, DEFAULTS);
    if (o.__mode) this.mode = MODES.some(m => m.id === o.__mode) ? o.__mode : MODES[0].id;
    for (const k in o) {
      if (k === '__mode' || k === '__v') continue;
      const g = paramByKey.get(k);
      if (!g) { errs.push(k); continue; }
      this.values[k] = o[k];
    }
    this.dirty = true;
    return errs;
  }
  /* the same thing, in the URL: base64url of the JSON, gzip-free and short
     enough that a link with two dozen changed knobs stays pastable */
  encode() {
    const json = JSON.stringify(this.toObject());
    const bytes = new TextEncoder().encode(json);
    let bin = '';
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }
  decode(token) {
    try {
      const b64 = token.replace(/-/g, '+').replace(/_/g, '/');
      const bin = atob(b64);
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      const json = new TextDecoder().decode(bytes);
      return this.fromObject(JSON.parse(json));
    } catch (e) {
      return ['decode failed'];
    }
  }
  /* what did that preset actually change? (for the boot line) */
  diffFrom(other) {
    const out = [];
    for (const g of GROUPS) {
      const k = keyOf(g);
      if (other.values[k] !== this.values[k]) out.push(k);
    }
    if (other.mode !== this.mode) out.push('mode:' + this.mode);
    return out;
  }
}

/* ------------------------------------------------------------------ random -- */
export function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0; a = (a + 0x6D2B79F5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
export function hashString(s) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
  return h >>> 0;
}

/* Randomise, but only within the valleys that are worth visiting: optics stay
   in the range where a cell actually works, geology stays plausible. */
const RANDOM_WINDOWS = {
  u_volt: [0.0, 1.8], u_polP: [-90, 90], u_polA: [-90, 90], u_thick: [1.5, 16],
  u_na: [0.25, 1.3], u_bands: [5, 16], u_lcSlices: [8, 48], u_lcTwist: [-180, 270],
  u_lcTilt: [0, 40], u_disp: [0.002, 0.05], u_ne: [1.5, 1.85], u_no: [1.42, 1.62],
  u_epoch: [2020, 2026], u_span: [3, 9], u_decay: [0.0, 0.9], u_terraces: [3, 14],
  u_vScale: [0.8, 6], u_vOct: [3, 8], u_traceCount: [3, 24], u_xtlTwins: [0, 6],
  u_xtlFaces: [4, 9], u_xtlSize: [0.4, 1.4], u_layer: [2, 9], u_traceRate: [0.05, 2.5],
};
export function randomize(cfg, seed, opts) {
  const rnd = mulberry32(seed >>> 0);
  const o = opts || {};
  const skip = new Set(o.skip || []);
  const mode = MODES.find(m => m.id === cfg.mode) || MODES[0];
  for (const g of GROUPS) {
    const k = keyOf(g);
    if (skip.has(k)) continue;
    if (g.u && !g.u.match(/^u_/) && k === g.u) continue;
    if (g.tab === 'System' || g.tab === 'Overlay') continue;   /* keep the instrument */
    if (g.u && mode.id !== 'conoscope' && (k === 'u_polP' || k === 'u_polA')) { /* keep, optics matter */ }
    const lvl = o.maxLevel === undefined ? 9 : g.lvl;
    if (g.lvl > lvl && !o.everything) continue;
    if (o.onlyMode && g.m && g.m.indexOf(mode.id) < 0) continue;
    if (g.t === 'B' && !o.flipBools) {
      if (['u_memoryOn', 'u_lcShowRods', 'u_lcShowRays', 'u_mosaic'].indexOf(k) >= 0) { /* leave on */ }
      else continue;
    }
    if (g.t === 'F' || g.t === 'I') {
      const win = RANDOM_WINDOWS[k] || [g.min, g.max];
      const lo = win[0] !== undefined ? win[0] : g.min;
      const hi = win[1] !== undefined ? win[1] : g.max;
      let v = lo + (hi - lo) * rnd();
      if (g.t === 'I') v = Math.round(v);
      else if (g.st) v = Math.round(v / g.st) * g.st;
      cfg.set(k, v);
    } else if (g.t === 'S') {
      cfg.set(k, g.o[Math.floor(rnd() * g.o.length)]);
    } else if (g.t === 'C') {
      const h = Math.floor(rnd() * 360);
      const s = 0.18 + 0.55 * rnd();
      const l = 0.25 + 0.4 * rnd();
      cfg.set(k, hslToHex(h, s, l));
    }
  }
  cfg.dirty = true;
  return cfg;
}
export function hslToHex(h, s, l) {
  const k = (n) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n) => l - a * Math.max(-1, Math.min(Math.min(k(n) - 3, 9 - k(n)), 1));
  const to = (x) => Math.round(255 * x).toString(16).padStart(2, '0');
  return '#' + to(f(0)) + to(f(8)) + to(f(4));
}

/* ------------------------------------------------------------------- clock -- */
/* One phase variable drives everything, so "what is happening" has a single
   answer at any instant. The waveform shapes are the voltage envelopes a real
   cell would be driven with. */
export class Clock {
  constructor() {
    this.time = 0;
    this.phase = 0;
    this.pulse = 0;
    this.pulsePrev = 0;
    this.frame = 0;
    this.dt = 1 / 60;
    this.cycle = 0;
    this.smoothed = 0;
    this.growth = 0;
  }
  wave(kind, p) {
    switch (kind) {
      case 'sine': return 0.5 + 0.5 * Math.sin(p * Math.PI * 2 - Math.PI / 2);
      case 'triangle': return Math.abs(((p * 2) % 2) - 1);
      case 'square': return p < 0.5 ? 1 : 0;
      case 'saw': return 1 - p;
      case 'double': return Math.pow(0.5 + 0.5 * Math.sin(p * Math.PI * 2 - Math.PI / 2), 2);
      default: { /* gaussian heartbeat */
        const x = p - 0.18;
        const y = x - Math.floor(x);
        return Math.exp(-Math.pow((y - 0.5) * 6.0, 2)) + 0.35 * Math.exp(-Math.pow((y - 0.02) * 9.0, 2));
      }
    }
  }
  advance(dt, cfg) {
    const span = cfg.get('u_period') || 4;
    const duty = cfg.get('u_duty');
    this.dt = dt;
    if (cfg.get('u_anim') && !cfg.get('paused')) {
      this.time += dt;
      const dir = cfg.get('u_dir') >= 0 ? 1 : -1;
      const jitter = cfg.get('u_jitter');
      const speed = cfg.get('u_speed') * dir;
      let rate = speed / Math.max(span, 0.02);
      if (jitter > 0) rate *= 1 + jitter * (Math.sin(this.time * 0.7) * 0.5 + Math.sin(this.time * 2.3) * 0.25);
      this.phase = (this.phase + rate * dt + 1) % 1;
      this.frame++;
      this.growth = Math.min(1, this.growth + dt / Math.max(span * 6, 1));
    }
    const clockMode = cfg.get('u_clock');
    let p = this.phase;
    if (clockMode === 'clock') p = 0.5 + 0.5 * Math.sin(this.time * 0.15);
    else if (clockMode === 'growth') p = this.growth;
    else if (clockMode === 'tectonic') p = 0.5 + 0.5 * Math.sin(this.time * 0.02);
    else if (clockMode === 'flood') p = (this.phase * 1.6) % 1;
    else if (clockMode === 'freeze') p = cfg.get('u_phase');
    else if (clockMode === 'lfo') p = (0.5 + 0.5 * Math.sin(this.time * 0.9)) * 0.5 + this.phase * 0.5;
    p = (p + cfg.get('u_phase')) % 1;
    if (duty !== 0.5) p = Math.pow(p, duty > 0.5 ? 1 / (2 * duty) : 2 * duty);
    this.pulsePrev = this.pulse;
    const raw = this.wave(cfg.get('u_wave'), p);
    const settle = Math.min(0.999, Math.max(0, cfg.get('u_settle')));
    const k = 1 - Math.exp(-dt / Math.max(0.004, (1 - settle) * 0.6 + 0.004));
    this.smoothed += (raw - this.smoothed) * k;
    this.pulse = this.smoothed;
    this.cycle = p;
    return this;
  }
}

export { GROUPS, MODES, MODE_DEFAULTS, PRESETS, CLAIMS, RAMPS };
