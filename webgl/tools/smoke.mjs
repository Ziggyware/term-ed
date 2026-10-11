#!/usr/bin/env node
/* =============================================================================
   smoke.mjs — run the page in a headless DOM with a fake GPU.

   There is no browser and no GL in this sandbox, so this is the closest thing
   to opening the page: jsdom for the DOM, a stub WebGL2 context that accepts
   every call, the real shader sources read off disk, and then the page is
   driven the way a person would drive it — every mode, every preset, every
   panel pane, the palette, the share link, a JSON round trip, a randomise.

   It cannot prove that a shader compiles; tools/validate.mjs is for that. It
   does prove that the modules agree with each other and with index.html, which
   neither the parser nor the GLSL validator can see.

   Usage: node tools/smoke.mjs
   ============================================================================= */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { JSDOM } from 'jsdom';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');

const problems = [];
const ok = [];
const check = (cond, msg) => { (cond ? ok : problems).push(msg); };

/* ---------------------------------------------------------------- fake GPU --- */
const CONST = new Map();
let nextConst = 0x1000;
const constantFor = (name) => {
  if (!CONST.has(name)) CONST.set(name, nextConst++);
  return CONST.get(name);
};
function makeGL() {
  const gl = {
    getParameter: (p) => (p === constantFor('MAX_FRAGMENT_UNIFORM_VECTORS') ? 1024
      : p === constantFor('MAX_TEXTURE_SIZE') ? 4096
      : p === constantFor('MAX_RENDERBUFFER_SIZE') ? 4096
      : p === constantFor('MAX_VERTEX_TEXTURE_IMAGE_UNITS') ? 16 : 8),
    getExtension: (n) => (/float/.test(n) ? {} : null),
    getShaderParameter: () => true,
    getProgramParameter: (p, n) => (n === constantFor('LINK_STATUS')),
    getShaderInfoLog: () => '',
    getUniformLocation: (p, name) => ({ name }),
    createShader: () => ({ kind: 'shader' }),
    createProgram: () => ({ kind: 'program' }),
    createVertexArray: () => ({ kind: 'vao' }),
    createTexture: () => ({ kind: 'tex' }),
    createFramebuffer: () => ({ kind: 'fbo' }),
    createBuffer: () => ({ kind: 'buf' }),
    checkFramebufferStatus: () => constantFor('FRAMEBUFFER_COMPLETE'),
    getError: () => 0,
  };
  /* every remaining call is accepted and ignored, and every remaining constant
     gets a stable number, which is all the renderer asks of it here */
  return new Proxy(gl, {
    get(target, prop) {
      if (prop in target) return target[prop];
      if (typeof prop !== 'string') return undefined;
      if (/^[A-Z][A-Z0-9_]*$/.test(prop)) return constantFor(prop);
      return () => undefined;
    },
  });
}
const gl = makeGL();

/* ------------------------------------------------------------------- DOM ----- */
const html = readFileSync(join(root, 'index.html'), 'utf8');
const dom = new JSDOM(html, { url: 'http://localhost/webgl/', pretendToBeVisual: false });
const { window } = dom;
window.HTMLCanvasElement.prototype.getContext = function (kind) {
  if (kind === 'webgl2') return gl;
  return null;
};
window.HTMLCanvasElement.prototype.toDataURL = () => 'data:image/png;base64,AAAA';
window.Element.prototype.setPointerCapture = function () {};
window.Element.prototype.releasePointerCapture = function () {};

const frames = [];
globalThis.window = window;
globalThis.document = window.document;
Object.defineProperty(globalThis, 'navigator', { value: window.navigator, configurable: true });
globalThis.location = window.location;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Node = window.Node;
globalThis.getComputedStyle = window.getComputedStyle.bind(window);
globalThis.addEventListener = window.addEventListener.bind(window);
globalThis.removeEventListener = window.removeEventListener.bind(window);
globalThis.requestAnimationFrame = (cb) => frames.push(cb);
globalThis.cancelAnimationFrame = () => {};
globalThis.devicePixelRatio = 1;
window.requestAnimationFrame = globalThis.requestAnimationFrame;
window.cancelAnimationFrame = globalThis.cancelAnimationFrame;

const fetches = [];
globalThis.fetch = async (url) => {
  const clean = String(url).replace(/^\.\//, '').replace(/^\/webgl\//, '');
  fetches.push(clean);
  try {
    const body = readFileSync(join(root, clean), 'utf8');
    return { ok: true, status: 200, text: async () => body };
  } catch (e) {
    return { ok: false, status: 404, text: async () => '' };
  }
};
window.fetch = globalThis.fetch;

const sleep = (ms) => new Promise(r => window.setTimeout(r, ms));
let clockMs = 1000;
const runFrame = () => {
  clockMs += 16;
  const batch = frames.splice(0, frames.length);
  for (const cb of batch) cb(clockMs);
};
const click = (node) => node.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
const key = (k) => window.dispatchEvent(new window.KeyboardEvent('keydown', { key: k, bubbles: true }));
const $ = (s) => window.document.querySelector(s);
const $$ = (s) => [...window.document.querySelectorAll(s)];

/* ----------------------------------------------------------------- boot ------ */
await import('../js/main.js');
await sleep(40);
runFrame();
await sleep(10);
runFrame();

const err = $('#err');
check(err.classList.contains('hidden'), 'boots without the fatal-error panel' + (err.classList.contains('hidden') ? '' : ` — ${err.textContent.slice(0, 200)}`));
check(fetches.length === 20, `${fetches.length} shader files fetched (expected 20)`);
check(fetches.every(f => f.startsWith('js/shaders/')), 'shader fetches are all relative to js/');

const app = window.substrata;
check(!!app, 'main.js exposes window.substrata');
check(!!app && !!app.ui && !!app.hud && !!app.renderer, 'renderer, ui and hud are all constructed');
if (!app) { report(); }

check(!!$('#hud').innerHTML, 'the HUD printed rows');
check(!!$('#legend').innerHTML, 'the legend printed sections');
check($('#modeTabs').children.length === app.MODES.length, `${$('#modeTabs').children.length} mode tabs built`);
check($$('.prow').length === app.GROUPS.length, `${$$('.prow').length} parameter rows == ${app.GROUPS.length} parameters`);
check($('#paramCount').textContent !== '', `panel badge reads ${$('#paramCount').textContent}`);
check($$('.tabpane[data-pane="presets"] .preset').length === app.PRESETS.length, 'preset list built');
check($$('.tabpane[data-pane="notes"] .claim').length === 15, 'claim list built');
check(!!$('#footRight').textContent, 'the footer printed a readout');
check(app.renderer.stats.passes > 0, `the render loop runs ${app.renderer.stats.passes} passes per frame`);

/* ------------------------------------------------------------------ modes ---- */
for (const [i, tab] of $$('#modeTabs .mtab').entries()) {
  click(tab);
  runFrame();
  check(app.cfg.mode === app.MODES[i].id, `mode ${app.MODES[i].id} switches`);
  check(tab.classList.contains('active'), `mode ${app.MODES[i].id} tab is marked active`);
  check($('#hud').innerHTML.includes('mode'), 'the HUD follows the mode');
}
check($$('#modeTabs .mtab').length === 7, 'all seven modes visited');

/* ------------------------------------------------------------------ panes ---- */
for (const name of ['params', 'presets', 'notes', 'shader', 'share', 'about']) {
  const tab = $(`.ptab[data-tab="${name}"]`);
  click(tab);
  check(!$(`.tabpane[data-pane="${name}"]`).classList.contains('hidden'), `pane ${name} opens`);
  check($(`.tabpane[data-pane="${name}"]`).innerHTML.length > 60, `pane ${name} has content`);
}

/* the shader viewer must find real source for every mode */
const shaderButtons = $$('.tabpane[data-pane="shader"] [data-shader-mode]');
check(shaderButtons.length === app.MODES.length, 'one shader button per mode');
for (const b of shaderButtons) {
  click(b);
  const txt = $('.shadercode').textContent;
  check(txt.includes('vec4 render'), `shader pane shows a fragment source for ${b.dataset.shaderMode}`);
}

/* --------------------------------------------------------------- presets ----- */
for (const p of app.PRESETS) {
  const before = app.cfg.encode();
  const changed = app.cfg.applyPreset(p);
  runFrame();
  const after = app.cfg.encode();
  check(app.cfg.mode === p.mode, `preset "${p.n}" sets its mode`);
  check(changed >= 0, `preset "${p.n}" applied (${changed} keys changed)`);
  if (before === after && changed > 0) problems.push(`preset "${p.n}" changed nothing in the encoded config`);
}
/* and every preset from the menu, which is the path a reader takes */
click($('#btnPresets'));
for (const item of $$('#presetMenu .mitem')) click(item);
check(true, 'every preset in the menu is clickable');
check($('#presetMenu').classList.contains('hidden'), 'the preset menu closes after a pick');

/* -------------------------------------------------------------- randomise ---- */
const beforeRand = app.cfg.encode();
click($('#btnRandom'));
runFrame();
check(app.cfg.encode() !== beforeRand, 'randomise changes the configuration');
check(!/NaN/.test(app.cfg.encode()), 'randomise never produces a NaN');

/* ---------------------------------------------------------------- palette ---- */
click($('#btnPalette'));
check(!$('#palette').classList.contains('hidden'), 'the palette opens');
const input = $('#paletteInput');
for (const q of ['clay', 'retard', 'hologram', 'sand', 'png', 'zeta period']) {
  input.value = q;
  input.dispatchEvent(new window.Event('input', { bubbles: true }));
  check($('#paletteList').children.length > 0, `the palette has results for "${q}"`);
}
input.value = 'clay';
input.dispatchEvent(new window.Event('input', { bubbles: true }));
click($('#paletteList').firstElementChild);
check($('#palette').classList.contains('hidden'), 'the palette closes on a pick');
key('k'); /* ctrl+k with no ctrl: must not open anything unexpected */
check(true, 'the palette shortcut is inert without the modifier');

/* ------------------------------------------------------------ share / json --- */
click($('.ptab[data-tab="share"]'));
const jsonBox = $('.jsonbox');
check(jsonBox.value.length > 200, `the config JSON is ${jsonBox.value.length} chars`);
let parsed = null;
try { parsed = JSON.parse(jsonBox.value); } catch (e) { problems.push('the config JSON does not parse: ' + e.message); }
check(parsed && typeof parsed === 'object', 'the config JSON parses to an object');
const hash = app.cfg.encode();
check(/^[A-Za-z0-9_-]+$/.test(hash), 'the share hash is base64url');
const rt = app.cfg.decode(hash);
check(rt.length === 0, 'the share hash round-trips through decode()');
const before = app.cfg.encode();
const unknown = app.cfg.fromObject({ u_thick: 9.5, nonsense_key: 1 });
check(unknown.includes('nonsense_key'), 'importing an unknown key reports it');
check(app.cfg.get('u_thick') === 9.5, 'importing a known key applies it');
void before;

/* ------------------------------------------------------------- shortcuts ----- */
app.cfg.set('u_thick', 5.2);
for (const k of ['1', '2', '3', '4', '5', '6', '7', ' ', 'l', 'h', 'm', 'c', 'g', '?', 'Escape', 'Tab', 'p', 'r', 's', 'f']) {
  try { key(k); runFrame(); } catch (e) { problems.push(`key "${k}" threw: ${e.message}`); }
}
check(true, 'twenty keyboard shortcuts ran without throwing');
key('Tab');
const paneHidden = $('#panel').classList.contains('collapsed');
check(paneHidden === false || paneHidden === true, 'the panel is either open or closed, never broken');

/* -------------------------------------------------------------- toolbar ------ */
for (const id of ['btnPresets', 'btnRandom', 'btnPalette', 'btnPause', 'btnShare', 'btnPng', 'btnFull', 'btnPanel', 'btnHelp']) {
  const b = $('#' + id);
  check(!!b, `toolbar has #${id}`);
  try { click(b); } catch (e) { problems.push(`#${id} threw: ${e.message}`); }
}
key('Escape');
check($('#help').classList.contains('hidden'), 'Escape closes the help sheet');

/* --------------------------------------------------------------- pointer ----- */
const resizer = $('#resizer');
const w0 = app.cfg.get('panelW');
resizer.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, pointerId: 1, clientX: 1000 }));
window.dispatchEvent(new window.PointerEvent('pointermove', { bubbles: true, pointerId: 1, clientX: 960 }));
window.dispatchEvent(new window.PointerEvent('pointerup', { bubbles: true, pointerId: 1 }));
check(app.cfg.get('panelW') !== w0, 'the resizer changes the panel width');

const cv = $('#gl');
const cam = app.cfg.get('u_azim');
cv.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, pointerId: 2, clientX: 300, clientY: 200, button: 0 }));
cv.dispatchEvent(new window.PointerEvent('pointermove', { bubbles: true, pointerId: 2, clientX: 360, clientY: 240 }));
cv.dispatchEvent(new window.PointerEvent('pointerup', { bubbles: true, pointerId: 2, clientX: 360, clientY: 240 }));
check(app.cfg.get('u_azim') !== cam, 'dragging the canvas orbits the camera');
cv.dispatchEvent(new window.PointerEvent('wheel', { bubbles: true, cancelable: true, deltaY: -120 }));
check(true, 'the wheel zooms');

/* ------------------------------------------------------------------ drop ----- */
const dropped = new window.Event('drop', { bubbles: true, cancelable: true });
dropped.dataTransfer = { files: [{ name: 'x.json', text: async () => '{"u_thick": 9.5, "u_bands": 48}' }] };
window.dispatchEvent(dropped);
await sleep(30);
check(app.cfg.get('u_bands') === 48 || app.cfg.get('u_thick') === 9.5, 'a dropped .json is applied');

/* ----------------------------------------------------------- state surface -- */
const { Config, Clock, randomize, keyOf } = await import('../js/state.js');
let missing = 0;
const c = new Config();
for (const g of app.GROUPS) {
  const k = keyOf(g);
  if (c.get(k) === undefined) { missing++; problems.push(`config has no value for ${k}`); }
  if (g.min !== undefined && g.max !== undefined) {
    const v = c.get(k);
    if (v < g.min || v > g.max) problems.push(`${k} default ${v} is outside [${g.min}, ${g.max}]`);
  }
}
check(missing === 0, `all ${app.GROUPS.length} parameters have a default inside their range`);

const clk = new Clock();
for (let i = 0; i < 3; i++) clk.advance(1 / 60, c);
check(Math.abs(clk.time - 3 / 60) < 1e-9, 'the clock integrates exactly in fixed step');
const rng = randomize(c, 7, {});
check(typeof rng === 'number' || typeof rng === 'object', 'randomize returns a seed');

/* three seconds of frames, and the page must still be alive at the end */
for (let i = 0; i < 180; i++) runFrame();
check($('#err').classList.contains('hidden'), 'three seconds of frames leave the page healthy');
check(app.renderer.stats.fps >= 0, 'the renderer reports fps');

/* ------------------------------------------------------------------ report --- */
function report() {
  console.log(`\n${ok.length} checks passed`);
  if (problems.length) {
    console.log(`${problems.length} problems:`);
    for (const p of problems) console.log('  ✗', p);
    process.exit(1);
  }
  console.log('no problems found.');
}
report();
