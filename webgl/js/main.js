/* =============================================================================
   main.js — bootstrap, the render loop, and every way of getting a picture out.

   The page is deliberately a single canvas with one visible control surface.
   Everything else — share links, JSON, PNG, the panel state, the HUD — is a
   view onto the same configuration object.
   ============================================================================= */
'use strict';

import { GROUPS, MODES, PRESETS, CLAIMS } from './params.js';
import { Config, Clock, randomize, keyOf, paramByKey } from './state.js';
import { Renderer, MODE_GLSL } from './renderer.js';
import { UI } from './ui.js';
import { Hud } from './hud.js';

const $ = (s) => document.querySelector(s);
const el = {
  canvas: $('#gl'),
  app: $('#app'),
  panel: $('#panel'),
  main: $('#main'),
  help: $('#help'),
  drop: $('#dropHint'),
  palette: $('#palette'),
  presetMenu: $('#presetMenu'),
  footerLeft: $('#footLeft'),
  footerRight: $('#footRight'),
};

const boot = { t0: performance.now(), errors: [] };
const cfg = new Config();
const clock = new Clock();
let renderer = null;
let ui = null;
let hud = null;
let mouse = null;
let lastPointer = null;
let fpsCapLast = 0;
let lastClaimSwap = 0;
let lastSync = 0;
let pendingPunch = null;

const log = (msg) => {
  if (/^warn/.test(msg)) boot.errors.push(msg);
  if (el.footerLeft) el.footerLeft.textContent = msg;
};

/* ---------------------------------------------------------------- boot ---- */
function readURL() {
  const params = new URLSearchParams(location.search);
  const hash = location.hash.replace(/^#/, '');
  const presetName = params.get('preset') || params.get('p');
  const mode = params.get('mode') || params.get('m');
  if (hash) {
    const errs = cfg.decode(hash);
    if (errs.length) log('hash did not decode cleanly');
  }
  if (presetName) {
    const p = PRESETS.find(x => x.n.toLowerCase() === presetName.toLowerCase());
    if (p) cfg.applyPreset(p);
  }
  if (mode && MODES.some(m => m.id === mode)) cfg.setMode(mode);
  const seed = params.get('seed');
  if (seed) cfg.set('u_globalSeed', parseFloat(seed));
  if (params.get('nohud')) cfg.set('hud', false);
  return params;
}

async function main() {
  const params = readURL();
  try {
    renderer = new Renderer(el.canvas, cfg, { log });
  } catch (e) {
    fatal(e.message + '\n\nThis page needs WebGL2 with floating-point render targets. Everything it draws is a fragment shader: there is no fallback image.');
    return;
  }
  try {
    await renderer.fetchSources('js/');
  } catch (e) {
    fatal('The shader sources could not be loaded: ' + e.message + '\nServe the page over HTTP (python3 -m http.server in the webgl folder) rather than opening the file directly — modules and shaders need a real origin.');
    return;
  }
  renderer.rng = renderer.rngFor(cfg);

  hud = new Hud(cfg, document);
  ui = new UI(cfg, document, hooks);
  ui.setActiveMode(cfg.mode);
  ui.refresh();
  ui.setPane(params.get('pane') || 'params');
  applyTheme();
  el.panel.style.width = cfg.get('panelW') + 'px';

  bindEvents();
  ui.syncJson();
  bootLine();
  requestAnimationFrame(loop);

  /* a handle for the console: everything the page is made of, in one place */
  window.substrata = { cfg, clock, renderer, ui, hud, MODES, PRESETS, GROUPS, hooks, randomize };

  const hash = cfg.encode();
  log(`ready · ${GROUPS.length} parameters · ${MODES.length} modes · link ${hash.length} chars`);
  if (boot.errors.length) hud.error(boot.errors.map(e => `<div>${e}</div>`).join(''));
}
function fatal(msg) {
  const box = $('#err');
  box.classList.remove('hidden');
  box.innerHTML = `<b>Substrata cannot run here.</b><div>${String(msg).replace(/\n/g, '<br>')}</div>`;
  const c = $('#gl');
  c.style.background = 'radial-gradient(circle at 40% 30%, #1b2430, #0a0d12)';
}

/* --------------------------------------------------------------- loop ----- */
function loop(now) {
  requestAnimationFrame(loop);
  const cap = cfg.get('fpsCap');
  if (cap !== 'display') {
    const ms = 1000 / parseInt(cap, 10);
    if (now - fpsCapLast < ms - 0.5) return;
    fpsCapLast = now;
  }
  const rawDt = Math.min(0.05, (now - (loop.last || now)) / 1000);
  loop.last = now;
  clock.advance(cfg.get('determinism') ? 1 / 60 : rawDt, cfg);
  try {
    const stats = renderer.renderFrame(clock, cfg, pendingPunch);
    pendingPunch = null;
    stats.scale = renderer.scaleBoost;
    if (now - lastClaimSwap > 4200) { hud.lastClaim = (hud.lastClaim + 1) | 0; lastClaimSwap = now; }
    hud.update(stats, clock);
    el.footerRight.textContent = `${stats.fps.toFixed(0)} fps · ${stats.ms.toFixed(1)} ms · ${cfg.mode} · Γ ${(( cfg.get('u_ne')-cfg.get('u_no'))*cfg.get('u_dnScale')*cfg.get('u_thick')*1000).toFixed(0)} nm`;
    if (now - lastSync > 1000) { lastSync = now; ui.syncJsonIfVisible(); }
  } catch (e) {
    fatal('Shader error: ' + e.message);
    throw e;
  }
}

/* --------------------------------------------------------------- hooks ---- */
const hooks = {
  onChange(key, fromInput) {
    if (key === 'panelW') el.panel.style.width = cfg.get('panelW') + 'px';
    if (key === 'theme') applyTheme();
    if (key === 'quality' || key === 'renderScale' || key === 'dprCap') renderer.scaleBoost = 1;
    if (key === 'u_memoryRes') renderer.ensureMemory(cfg);
    if (key === 'u_globalSeed') renderer.rng = renderer.rngFor(cfg);
    if (key === 'paused' || key === 'u_anim') updatePauseButton();
  },
  onMode(id) {
    if (cfg.mode === id) return;
    cfg.setMode(id);
    ui.setActiveMode(id);
    ui.refresh();
    try { renderer.programFor('mode', id); } catch (e) { fatal('Shader error in mode ' + id + ': ' + e.message); return; }
    const m = MODES.find(x => x.id === id);
    ui.toast(`${m.name} — ${m.blurb}`);
    const pane = document.querySelector('.ptab.active');
    if (pane && pane.dataset.tab === 'shader') ui.setPane('shader');
  },
  onPreset(p) {
    const changed = cfg.applyPreset(p);
    ui.setActiveMode(cfg.mode);
    ui.refresh();
    renderer.rng = renderer.rngFor(cfg);
    ui.toast(`${p.n} · ${changed} parameters changed`);
    document.querySelector('#footLeft').textContent = p.note || p.n;
  },
  onRandom() {
    randomize(cfg, (Math.random() * 1e9) | 0, { maxLevel: 1, flipBools: false });
    ui.refresh();
    ui.toast('randomised within the plausible range');
  },
  onCopyLink() {
    const text = location.origin + location.pathname + '#' + cfg.encode();
    /* clipboard access is a permission: when it is absent or refused, say so and
       put the link somewhere the reader can still take it from */
    const fallback = () => {
      const box = $('#shareFallback');
      if (box) { box.classList.remove('hidden'); box.querySelector('input').value = text; box.querySelector('input').select(); }
      ui.toast('the clipboard is not available — the link is in the side panel');
      showPanel(true);
      ui.setPane('share');
    };
    if (!navigator.clipboard || !navigator.clipboard.writeText) { fallback(); return; }
    navigator.clipboard.writeText(text).then(
      () => ui.toast(`share link copied (${text.length} chars)`),
      fallback);
  },
  onDownload() {
    const blob = new Blob([JSON.stringify(cfg.toObject(), null, 2)], { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `substrata-${cfg.mode}-${(cfg.get('u_globalSeed')).toFixed(2)}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  },
  onOpenFile() {
    const inp = document.createElement('input');
    inp.type = 'file';
    inp.accept = '.json,application/json';
    inp.onchange = () => {
      const f = inp.files[0];
      if (!f) return;
      const r = new FileReader();
      r.onload = () => {
        try {
          cfg.fromObject(JSON.parse(r.result));
          ui.setActiveMode(cfg.mode); ui.refresh(); renderer.rng = renderer.rngFor(cfg);
          ui.toast('configuration loaded');
        } catch (e) { ui.toast('that file is not a configuration'); }
      };
      r.readAsText(f);
    };
    inp.click();
  },
  onReset() {
    cfg.reset();
    cfg.setMode('substrata');
    ui.setActiveMode(cfg.mode); ui.refresh(); renderer.rng = renderer.rngFor(cfg);
    ui.toast('reset');
  },
  onShot() { shoot(); },
  onTogglePanel() { togglePanel(); },
  onLoad() { ui.setActiveMode(cfg.mode); renderer.rng = renderer.rngFor(cfg); },
  onReveal(key) {
    const g = paramByKey.get(key);
    if (!g) return;
    ui.setPane('params');
    ui.filterByTab(g.tab);
    const r = ui.rows.get(key);
    if (r) {
      r.row.scrollIntoView({ block: 'center', behavior: 'smooth' });
      r.row.classList.add('flash');
      setTimeout(() => r.row.classList.remove('flash'), 1400);
    }
  },
  onClaim(c) {
    cfg.set('u_epoch', (c.band[0] + c.band[1]) / 2);
    cfg.set('u_span', Math.max(2, c.band[1] - c.band[0] + 3));
    cfg.set('u_mosaic', cfg.get('u_mosaic') > 0.01 ? cfg.get('u_mosaic') : 0.8);
    ui.refresh();
    ui.toast(c.t);
  },
  sourceFor(modeId) {
    const m = MODE_GLSL[modeId];
    return m ? renderer.sources.get(m.src) : null;
  },
};

/* --------------------------------------------------------------- events ---- */
function bindEvents() {
  /* toolbar */
  $('#btnPanel').addEventListener('click', togglePanel);
  $('#btnFull').addEventListener('click', () => {
    if (document.fullscreenElement) document.exitFullscreen();
    else el.app.requestFullscreen && el.app.requestFullscreen();
  });
  $('#btnPause').addEventListener('click', () => { cfg.set('paused', !cfg.get('paused')); updatePauseButton(); });
  $('#btnRandom').addEventListener('click', hooks.onRandom);
  $('#btnPng').addEventListener('click', () => shoot());
  $('#btnShare').addEventListener('click', hooks.onCopyLink);
  $('#btnPresets').addEventListener('click', () => ui.togglePresetMenu());
  $('#btnPalette').addEventListener('click', () => ui.openPalette());
  $('#btnHelp').addEventListener('click', () => toggleHelp());
  $('#brand').addEventListener('click', () => { ui.setPane('about'); showPanel(true); });
  document.addEventListener('click', (e) => {
    if (!e.target.closest('#presetMenu') && !e.target.closest('#btnPresets')) el.presetMenu.classList.add('hidden');
    if (!e.target.closest('#palette') && !e.target.closest('#btnPalette')) ui.closePalette();
  });

  /* dragging the panel edge */
  const resizer = $('#resizer');
  if (resizer) {
    resizer.addEventListener('pointerdown', (e) => {
      resizer.setPointerCapture(e.pointerId);
      const startX = e.clientX, startW = el.panel.getBoundingClientRect().width;
      const move = (ev) => {
        const w = Math.max(240, Math.min(720, startW + (startX - ev.clientX)));
        el.panel.style.width = w + 'px';
        cfg.values.panelW = w;
      };
      const up = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); ui.syncJson(); };
      window.addEventListener('pointermove', move);
      window.addEventListener('pointerup', up);
    });
  }

  /* the picture: orbit, zoom, pan, and pressing a trace into the ground */
  const cv = el.canvas;
  cv.addEventListener('pointerdown', (e) => {
    cv.setPointerCapture(e.pointerId);
    lastPointer = { x: e.clientX, y: e.clientY, moved: false, az: cfg.get('u_azim'), el: cfg.get('u_elev'), tX: cfg.get('u_targetX'), tY: cfg.get('u_targetY') };
  });
  cv.addEventListener('pointermove', (e) => {
    const r = cv.getBoundingClientRect();
    mouse = [(e.clientX - r.left) / r.width * 2 - 1, -((e.clientY - r.top) / r.height * 2 - 1)];
    renderer.mouse = mouse;
    if (!lastPointer) return;
    const dx = e.clientX - lastPointer.x, dy = e.clientY - lastPointer.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) lastPointer.moved = true;
    if (e.shiftKey || e.buttons === 4) {
      cfg.values.u_targetX = lastPointer.tX + dx * 0.004 * cfg.get('u_dist');
      cfg.values.u_targetY = lastPointer.tY - dy * 0.004 * cfg.get('u_dist');
      ui.refresh();
    } else {
      cfg.values.u_azim = clamp180(lastPointer.az - dx * 0.3);
      cfg.values.u_elev = Math.max(-88, Math.min(88, lastPointer.el + dy * 0.25));
      ui.refresh();
    }
  });
  cv.addEventListener('pointerup', (e) => {
    if (lastPointer && !lastPointer.moved) {
      /* a click is a trace: press something into the ground */
      const r = cv.getBoundingClientRect();
      const u = (e.clientX - r.left) / r.width, v = 1 - (e.clientY - r.top) / r.height;
      if (cfg.get('u_memoryOn')) { pendingPunch = [u, v]; ui.toast('a trace, pressed in at ' + u.toFixed(2) + ', ' + v.toFixed(2)); }
    }
    lastPointer = null;
  });
  cv.addEventListener('wheel', (e) => {
    e.preventDefault();
    const d = cfg.get('u_dist') * Math.exp(Math.sign(e.deltaY) * 0.08);
    cfg.set('u_dist', d);
    ui.refresh();
  }, { passive: false });
  cv.addEventListener('dblclick', () => {
    cfg.set('u_dist', 3.2); cfg.set('u_azim', -26); cfg.set('u_elev', 9);
    ui.refresh();
  });

  /* keyboard */
  window.addEventListener('keydown', (e) => {
    const t = e.target;
    const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT');
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); ui.openPalette(); return; }
    if (e.key === 'Escape') { ui.closePalette(); el.presetMenu.classList.add('hidden'); toggleHelp(false); return; }
    if (typing) return;
    const digits = MODES.map(m => m.id);
    if (/^[1-7]$/.test(e.key)) {
      const id = digits[parseInt(e.key, 10) - 1];
      if (id) hooks.onMode(id);
      return;
    }
    switch (e.key.toLowerCase()) {
      case ' ': e.preventDefault(); cfg.set('paused', !cfg.get('paused')); updatePauseButton(); break;
      case 'tab': e.preventDefault(); togglePanel(); break;
      case 'p': ui.togglePresetMenu(); break;
      case 'r': hooks.onRandom(); break;
      case 's': shoot(); break;
      case 'f': $('#btnFull').click(); break;
      case '?': toggleHelp(); break;
      case 'l': cfg.set('legend', !cfg.get('legend')); ui.refresh(); break;
      case 'h': cfg.set('hud', !cfg.get('hud')); ui.refresh(); break;
      case 'm': cfg.set('u_mosaic', cfg.get('u_mosaic') > 0.01 ? 0 : 0.85); ui.refresh(); break;
      case 'c': cfg.set('u_splitMode', cfg.get('u_splitMode') === 'off' ? 'vertical' : 'off'); cfg.set('u_slotB', cfg.get('u_splitMode') !== 'off'); ui.refresh(); break;
      case 'g': {
        const opts = ['off', 'blocks', 'ascii', 'dots', 'dots2', 'hex'];
        const i = (opts.indexOf(cfg.get('u_glyph')) + 1) % opts.length;
        cfg.set('u_glyph', opts[i]); ui.refresh(); ui.toast('glyph pass: ' + opts[i]);
        break;
      }
      default: break;
    }
  });

  /* drag and drop a configuration */
  window.addEventListener('dragover', (e) => { e.preventDefault(); el.drop.classList.remove('hidden'); });
  window.addEventListener('dragleave', () => el.drop.classList.add('hidden'));
  window.addEventListener('drop', (e) => {
    e.preventDefault(); el.drop.classList.add('hidden');
    const f = e.dataTransfer.files[0];
    if (!f) return;
    const apply = (text) => {
      try { const bad = cfg.fromObject(JSON.parse(text)); ui.setActiveMode(cfg.mode); ui.refresh(); ui.syncJson(); ui.toast(bad.length ? `configuration dropped in, ignoring ${bad.length} unknown keys` : 'configuration dropped in'); }
      catch (err) { ui.toast('that file is not a configuration'); }
    };
    if (typeof f.text === 'function') f.text().then(apply, () => ui.toast('that file could not be read'));
    else {
      const r = new FileReader();
      r.onload = () => apply(r.result);
      r.readAsText(f);
    }
  });
  window.addEventListener('resize', () => ui.applyFilters());
}
function clamp180(v) { return Math.max(-180, Math.min(180, v)); }
function updatePauseButton() {
  const b = $('#btnPause');
  const paused = cfg.get('paused') || !cfg.get('u_anim');
  b.querySelector('.ico').textContent = paused ? '▶' : '❚❚';
  b.classList.toggle('on', paused);
}
function togglePanel(force) {
  const hidden = el.panel.classList.contains('collapsed');
  const want = force === undefined ? hidden : force;
  el.panel.classList.toggle('collapsed', !want);
  $('#btnPanel').classList.toggle('on', !want);
  setTimeout(() => window.dispatchEvent(new window.Event('resize')), 260);
}
function showPanel(want) { el.panel.classList.toggle('collapsed', !want); $('#btnPanel').classList.toggle('on', !want); }
function toggleHelp(force) {
  const hidden = el.help.classList.contains('hidden');
  const want = force === undefined ? hidden : force;
  el.help.classList.toggle('hidden', !want);
  if (want) {
    el.help.innerHTML = `
      <h3>What this is</h3>
      <p>Seven fragment-shader instruments about substrates, memory and light. The picture is the argument:
      clay keeps impressions because Δn, water content and time let it; sand does not; a liquid crystal makes
      the same decision with light instead of sediment.</p>
      <h4>The modes</h4>
      <ul>${MODES.map((m, i) => `<li><b>${i + 1}</b> ${m.name} — ${m.tag}. ${m.blurb}</li>`).join('')}</ul>
      <h4>Keys and gestures</h4>
      <ul>
        <li><kbd>1</kbd>–<kbd>7</kbd> modes · <kbd>Tab</kbd> panel · <kbd>Ctrl</kbd>+<kbd>K</kbd> search everything</li>
        <li><kbd>Space</kbd> pause · <kbd>P</kbd> presets · <kbd>R</kbd> randomise · <kbd>S</kbd> save PNG</li>
        <li><kbd>L</kbd> legend · <kbd>H</kbd> HUD · <kbd>M</kbd> evidence overlay · <kbd>C</kbd> split compare · <kbd>G</kbd> glyph pass</li>
        <li>drag to orbit · shift+drag to pan · wheel to zoom · double-click to reset the camera</li>
        <li>click the picture to press a trace into the ground (Substrata, Weathering, Hologram)</li>
      </ul>
      <h4>Honest limits</h4>
      <ul>
        <li>Jones calculus, not a 4×4 Berreman solve: the modes are exact where they say exact and approximate where they say ≈.</li>
        <li>Nothing is to scale, including the molecules, which are wrong by four orders of magnitude on purpose.</li>
        <li>The 2020–2026 claims came out of a chat and are drawn with their stated confidence, not endorsed.</li>
      </ul>
      <p class="hint">Press <kbd>Esc</kbd> or <kbd>?</kbd> to close.</p>`;
  }
}
function shoot() {
  renderer.wantShot = (url) => {
    if (!url) { ui.toast('the browser refused to hand over the frame'); return; }
    const a = document.createElement('a');
    a.href = url;
    a.download = `substrata-${cfg.mode}-${Date.now()}.png`;
    a.click();
    ui.toast(`PNG saved at ${renderer.canvas.width}×${renderer.canvas.height} — ${cfg.encode().length}-char link copied to the console`);
    console.log('share link:', location.origin + location.pathname + '#' + cfg.encode());
  };
}
function applyTheme() {
  document.documentElement.dataset.theme = cfg.get('theme');
}
function bootLine() {
  const d = document.querySelector('.tabpane[data-pane="presets"]');
  if (cfg.get('bootLine') && d) {
    const list = [];
    list.push(`<div class="boot">Boot · ${MODES.find(m => m.id === cfg.mode).name} · ${GROUPS.length} parameters live · WebGL2 · ${renderer.limits.uniformVectors} uniform vectors available, ${renderer.layout.floats.length + renderer.layout.ints.length + renderer.layout.cols.length} used for the knobs</div>`);
    d.insertAdjacentHTML('afterbegin', list.join(''));
  }
}

/* ui.syncJsonIfVisible is a tiny optimisation for the loop */
UI.prototype.syncJsonIfVisible = function () {
  const active = this.el.querySelector('.ptab.active');
  if (active && active.dataset.tab === 'share') this.syncJson();
};

main();
