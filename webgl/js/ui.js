/* =============================================================================
   ui.js — the control panel, built from params.js and from nothing else.

   Every row, every range, every default and every tooltip in this file comes
   out of the parameter registry, which is why the page can have 250 knobs
   without 250 hand-written controls (and why the tooltips can carry the physics
   without a separate document).
   ============================================================================= */
'use strict';

import { GROUPS, MODES, PRESETS, CLAIMS, HINGE, ABOUT } from './params.js';
import { keyOf } from './state.js';

const TAB_ORDER = ['Time', 'Camera', 'Light', 'Optics', 'Matter', 'LC', 'Crystal', 'Mode', 'Image', 'Overlay', 'System'];
const TAB_TITLES = {
  Time: 'Time', Camera: 'Camera', Light: 'Light', Optics: 'Optics', Matter: 'Matter',
  LC: 'Liquid crystal', Crystal: 'Crystal', Mode: 'Mode', Image: 'Image',
  Overlay: 'Overlay', System: 'System',
};

export class UI {
  constructor(cfg, el, hooks) {
    this.cfg = cfg;
    this.el = el;
    this.hooks = hooks || {};
    this.rows = new Map();       /* key -> {input, readout, row, def} */
    this.search = '';
    this.modeFilter = true;
    this.expanded = new Set();
    this.build();
  }

  /* --------------------------------------------------------------- helpers -- */
  h(tag, cls, txt) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (txt !== undefined) e.textContent = txt;
    return e;
  }

  /* ----------------------------------------------------------------- build -- */
  build() {
    this.buildTabs();
    this.buildParams();
    this.buildPresets();
    this.buildNotes();
    this.buildShader();
    this.buildShare();
    this.buildAbout();
    this.bindPanelChrome();
  }
  buildTabs() {
    const tabs = this.el.querySelector('#modeTabs');
    tabs.innerHTML = '';
    for (const m of MODES) {
      const b = this.h('button', 'mtab');
      b.dataset.mode = m.id;
      b.title = m.blurb;
      b.innerHTML = `<span class="ico">${m.ico}</span><span class="nm">${m.name}</span>`;
      b.addEventListener('click', () => this.hooks.onMode && this.hooks.onMode(m.id));
      tabs.appendChild(b);
    }
  }
  setActiveMode(id) {
    this.el.querySelectorAll('.mtab').forEach(b => b.classList.toggle('active', b.dataset.mode === id));
    const m = MODES.find(x => x.id === id);
    const title = this.el.querySelector('#brand .sub');
    if (m && title) title.textContent = `${m.tag} · ${m.blurb}`;
  }

  buildParams() {
    const pane = this.el.querySelector('.tabpane[data-pane="params"]');
    pane.innerHTML = '';
    this.paramPane = pane;
    const byTab = new Map();
    for (const g of GROUPS) {
      if (!byTab.has(g.tab)) byTab.set(g.tab, []);
      byTab.get(g.tab).push(g);
    }
    const order = TAB_ORDER.filter(t => byTab.has(t)).concat([...byTab.keys()].filter(t => TAB_ORDER.indexOf(t) < 0));
    for (const tab of order) {
      const sec = this.h('section', 'psection');
      sec.dataset.tab = tab;
      const byGrp = new Map();
      for (const g of byTab.get(tab)) {
        if (!byGrp.has(g.grp)) byGrp.set(g.grp, []);
        byGrp.get(g.grp).push(g);
      }
      for (const [grp, list] of byGrp) {
        const key = `${tab}/${grp}`;
        const gh = this.h('button', 'pgroup');
        gh.type = 'button';
        gh.innerHTML = `<span class="caret">▾</span><span class="gname">${grp}</span><span class="gcount">${list.length}</span>`;
        gh.setAttribute('aria-expanded', 'true');
        gh.addEventListener('click', () => {
          const collapsed = body.classList.toggle('collapsed');
          gh.setAttribute('aria-expanded', String(!collapsed));
          gh.classList.toggle('collapsed', collapsed);
          if (collapsed) this.expanded.delete(key); else this.expanded.add(key);
        });
        const body = this.h('div', 'pgroupbody');
        for (const g of list) body.appendChild(this.makeRow(g));
        sec.appendChild(gh);
        sec.appendChild(body);
      }
      this.lastGrp = null;
      pane.appendChild(sec);
    }
    /* the tab strip inside the panel */
    const strip = this.h('div', 'pfilter');
    strip.innerHTML = `<button class="pchip active" data-tab="all">All</button>` +
      order.map(t => `<button class="pchip" data-tab="${t}">${TAB_TITLES[t] || t}</button>`).join('');
    strip.addEventListener('click', (e) => {
      const b = e.target.closest('.pchip');
      if (!b) return;
      strip.querySelectorAll('.pchip').forEach(x => x.classList.toggle('active', x === b));
      this.filterByTab(b.dataset.tab);
    });
    pane.insertBefore(strip, pane.firstChild);
    const count = this.el.querySelector('#paramCount');
    if (count) count.textContent = GROUPS.length;
  }
  makeRow(g) {
    const key = keyOf(g);
    const row = this.h('div', 'prow');
    row.dataset.key = key;
    row.dataset.lvl = g.lvl !== undefined ? g.lvl : 1;
    row.dataset.tab = g.tab;
    if (g.m && MODES.length) row.dataset.modes = g.m.join(' ');
    const help = [g.a, g.tab + ' · ' + g.grp, `level ${g.lvl || 0}`, g.u ? `uniform ${g.u}` : 'cpu'].filter(Boolean).join('\n');
    row.title = help;
    const lab = this.h('label', 'plab');
    lab.textContent = g.l || key.replace(/^u_/, '').replace(/_/g, ' ');
    const fl = paramFlag(g);
    if (fl) {
      const flag = this.h('span', 'pflag', fl.text);
      flag.title = fl.why;
      lab.appendChild(flag);
    }
    row.appendChild(lab);

    const wrap = this.h('div', 'pctl');
    let input, readout = null;
    const set = (v, fromInput) => {
      const ok = this.cfg.set(key, v);
      if (ok) { this.readoutFor(g, readout); if (this.hooks.onChange) this.hooks.onChange(key, fromInput); }
      return ok;
    };
    if (g.t === 'B') {
      input = this.h('input', 'ptoggle');
      input.type = 'checkbox';
      input.checked = !!this.cfg.get(key);
      input.addEventListener('change', () => set(input.checked, true));
      wrap.appendChild(input);
    } else if (g.t === 'S') {
      input = this.h('select', 'pselect');
      for (const o of (g.o || [])) {
        const op = document.createElement('option');
        op.value = o; op.textContent = String(o).replace(/_/g, ' ');
        input.appendChild(op);
      }
      input.value = this.cfg.get(key);
      input.addEventListener('change', () => set(input.value, true));
      wrap.appendChild(input);
    } else if (g.t === 'C') {
      input = this.h('input', 'pcolor');
      input.type = 'color';
      input.value = this.cfg.get(key);
      const hex = this.h('input', 'phex');
      hex.type = 'text';
      hex.value = this.cfg.get(key);
      hex.spellcheck = false;
      input.addEventListener('input', () => { set(input.value, true); hex.value = input.value; });
      hex.addEventListener('change', () => { if (set(hex.value, true)) input.value = hex.value; });
      wrap.appendChild(input); wrap.appendChild(hex);
    } else {
      input = this.h('input', 'prange');
      input.type = 'range';
      input.min = g.min; input.max = g.max; input.step = g.st ?? 0.01;
      input.value = this.cfg.get(key);
      const num = this.h('input', 'pnum');
      num.type = 'text';
      num.spellcheck = false;
      const dec = decimals(g.st || 0.01);
      num.value = fmt(this.cfg.get(key), dec);
      input.addEventListener('input', () => { set(parseFloat(input.value), true); num.value = fmt(this.cfg.get(key), dec); });
      num.addEventListener('change', () => {
        if (set(parseFloat(num.value), true)) input.value = this.cfg.get(key);
        num.value = fmt(this.cfg.get(key), dec);
      });
      input.addEventListener('dblclick', () => { set(g.v, true); this.refresh(); });
      wrap.appendChild(input); wrap.appendChild(num);
      readout = num;
    }
    row.appendChild(wrap);
    this.rows.set(key, { row, input, readout, def: g, flag: fl });
    return row;
  }
  readoutFor(g, readout) {
    if (!readout) return;
    const v = this.cfg.get(keyOf(g));
    readout.value = fmt(v, decimals(g.st || 0.01));
  }

  /* ------------------------------------------------------------ presets ---- */
  buildPresets() {
    const pane = this.el.querySelector('.tabpane[data-pane="presets"]');
    pane.innerHTML = '';
    const intro = this.h('p', 'pint');
    intro.textContent = 'Each preset is a claim from the conversation, set up so you can disagree with it. They only change what they name.';
    pane.appendChild(intro);
    const list = this.h('div', 'presetlist');
    for (const p of PRESETS) {
      const item = this.h('button', 'preset');
      item.innerHTML = `<span class="pn">${p.n}</span><span class="pm">${(MODES.find(m => m.id === p.mode) || {}).name || p.mode}</span><span class="pd">${p.note || ''}</span>`;
      item.addEventListener('click', () => this.hooks.onPreset && this.hooks.onPreset(p));
      list.appendChild(item);
    }
    pane.appendChild(list);
  }

  /* --------------------------------------------------------- field notes --- */
  buildNotes() {
    const pane = this.el.querySelector('.tabpane[data-pane="notes"]');
    pane.innerHTML = '';
    pane.appendChild(this.h('p', 'pint', 'The claims behind the overlay, with the hinge that makes each one interesting and the confidence it was given in the conversation. The colour bars are the drawing.'));
    const shape = this.h('div', 'notesbox');
    shape.innerHTML = `<h4>The shape these share</h4><ol>${HINGE.map(x => `<li>${x}</li>`).join('')}</ol>`;
    pane.appendChild(shape);
    const cats = ['material', 'bio', 'geo', 'cognitive', 'tech', 'eco'];
    const table = this.h('div', 'claims');
    for (const c of CLAIMS) {
      const d = this.h('div', 'claim');
      d.dataset.cat = c.cat;
      d.dataset.conf = c.lvl;
      d.innerHTML = `<div class="ctop"><span class="ccat" data-cat="${c.cat}">${c.cat}</span>
        <span class="cyear">${c.band[0]}–${c.band[1]}</span><span class="cconf">${c.lvl}</span></div>
        <div class="ctext">${c.t}</div><div class="chinge">${c.hinge}</div>`;
      table.appendChild(d);
    }
    pane.appendChild(table);
    pane.appendChild(this.h('p', 'pint', 'Colour in the overlay follows the domain; the fill follows the confidence; the bar is the year window. The faint ones are faint on purpose.'));
  }

  /* --------------------------------------------------------------- shader --- */
  buildShader() {
    const pane = this.el.querySelector('.tabpane[data-pane="shader"]');
    pane.innerHTML = '';
    const bar = this.h('div', 'shaderbar');
    for (const m of MODES) {
      const b = this.h('button', 'pchip', m.name);
      b.dataset.shaderMode = m.id;
      b.addEventListener('click', () => this.showShader(m.id, b));
      bar.appendChild(b);
    }
    pane.appendChild(bar);
    this.shaderPre = this.h('pre', 'shadercode', 'Pick a mode to read its fragment shader.');
    pane.appendChild(this.shaderPre);
    this.shaderInfo = this.h('div', 'pint');
    pane.appendChild(this.shaderInfo);
  }
  showShader(modeId, btn) {
    this.el.querySelectorAll('[data-shader-mode]').forEach(b => b.classList.toggle('active', b === btn));
    const src = this.hooks.sourceFor ? this.hooks.sourceFor(modeId) : null;
    if (!src) { this.shaderPre.textContent = 'shader source not available yet'; return; }
    const lines = src.split('\n');
    this.shaderPre.textContent = lines.map((l, i) => String(i + 1).padStart(3) + '  ' + l).join('\n');
    this.shaderInfo.textContent = `${lines.length} lines. The same file runs on the GPU as-is: edit it, reload, and the page is different.`;
  }

  /* ----------------------------------------------------------- load/save --- */
  buildShare() {
    const pane = this.el.querySelector('.tabpane[data-pane="share"]');
    pane.innerHTML = '';
    pane.appendChild(this.h('p', 'pint', 'The whole configuration is a JSON object; a share link is that object, base64url-encoded, in the hash. Nothing leaves the page.'));
    this.jsonBox = this.h('textarea', 'jsonbox');
    this.jsonBox.spellcheck = false;
    this.jsonBox.rows = 10;
    pane.appendChild(this.jsonBox);
    const row = this.h('div', 'btnrow');
    const mk = (label, fn, title) => {
      const b = this.h('button', 'pbtn', label);
      if (title) b.title = title;
      b.addEventListener('click', fn);
      row.appendChild(b);
      return b;
    };
    mk('Fill from current', () => this.syncJson());
    mk('Apply', () => this.applyJson());
    mk('Copy share link', () => this.hooks.onCopyLink && this.hooks.onCopyLink());
    mk('Download .json', () => this.hooks.onDownload && this.hooks.onDownload());
    mk('Open .json', () => this.hooks.onOpenFile && this.hooks.onOpenFile());
    mk('Reset everything', () => { if (confirm('Reset every parameter to its default?')) this.hooks.onReset && this.hooks.onReset(); });
    pane.appendChild(row);
    /* the share link also lives here in plain text, for browsers that will not
       hand over the clipboard */
    const fb = this.h('div', 'sharefallback hidden');
    fb.id = 'shareFallback';
    const link = this.h('input', 'sharelink');
    link.type = 'text';
    link.readOnly = true;
    link.spellcheck = false;
    link.value = '';
    link.title = 'the current share link — select it and copy it by hand';
    const copy = this.h('button', 'pbtn', 'select');
    copy.addEventListener('click', () => { link.select(); });
    fb.appendChild(link);
    fb.appendChild(copy);
    pane.appendChild(fb);
    this.jsonNote = this.h('div', 'pint');
    pane.appendChild(this.jsonNote);
    this.syncJson();
  }
  syncJson() {
    if (!this.jsonBox) return;
    this.jsonBox.value = JSON.stringify(this.cfg.toObject(), null, 1);
    this.jsonNote.textContent = `${Object.keys(this.cfg.toObject()).length - 2} parameters differ from the defaults.`;
  }
  applyJson() {
    try {
      const o = JSON.parse(this.jsonBox.value);
      const errs = this.cfg.fromObject(o);
      this.refresh();
      if (this.hooks.onLoad) this.hooks.onLoad();
      this.jsonNote.textContent = errs.length ? `applied, ignoring unknown keys: ${errs.slice(0, 8).join(', ')}` : 'applied.';
    } catch (e) {
      this.jsonNote.textContent = 'that is not JSON: ' + e.message;
    }
  }

  buildAbout() {
    const pane = this.el.querySelector('.tabpane[data-pane="about"]');
    pane.innerHTML = '';
    const t = this.h('h3', 'abouttitle', ABOUT.title);
    pane.appendChild(t);
    pane.appendChild(this.h('p', 'pint', ABOUT.lede));
    for (const [head, body] of ABOUT.paras) {
      pane.appendChild(this.h('h4', 'abouthead', head));
      pane.appendChild(this.h('p', 'pint', body));
    }
    const keys = this.h('div', 'notesbox');
    keys.innerHTML = `<h4>Keys</h4><ul>
      <li><kbd>1</kbd>–<kbd>7</kbd> modes · <kbd>Tab</kbd> panel · <kbd>P</kbd> presets · <kbd>R</kbd> randomise</li>
      <li><kbd>Space</kbd> pause · <kbd>F</kbd> fullscreen · <kbd>S</kbd> save PNG · <kbd>?</kbd> help</li>
      <li><kbd>Ctrl</kbd>+<kbd>K</kbd> search every parameter · drag on the picture to orbit · wheel to zoom</li>
      <li>click on the picture to press a trace into the ground, when the memory field is on</li></ul>`;
    pane.appendChild(keys);
  }

  /* ------------------------------------------------------------- behaviour -- */
  bindPanelChrome() {
    const el = this.el;
    el.querySelector('#search').addEventListener('input', (e) => {
      this.search = e.target.value.toLowerCase().trim();
      this.applyFilters();
    });
    el.querySelectorAll('.ptab').forEach(b => {
      b.addEventListener('click', () => this.setPane(b.dataset.tab));
    });
    const btnPanelClose = el.querySelector('#btnPanelClose');
    if (btnPanelClose) btnPanelClose.addEventListener('click', () => this.hooks.onTogglePanel && this.hooks.onTogglePanel());
  }
  setPane(name) {
    this.el.querySelectorAll('.ptab').forEach(b => b.classList.toggle('active', b.dataset.tab === name));
    this.el.querySelectorAll('.tabpane').forEach(p => p.classList.toggle('hidden', p.dataset.pane !== name));
    if (name === 'share') this.syncJson();
    if (name === 'shader') this.showShader(this.cfg.mode, this.el.querySelector('[data-shader-mode="' + this.cfg.mode + '"]'));
  }
  filterByTab(tab) {
    this.tabFilter = tab;
    this.applyFilters();
  }
  applyFilters() {
    const lvlMax = parseInt(this.cfg.get('lvl'), 10) || 0;
    const q = this.search;
    let shown = 0;
    for (const [key, r] of this.rows) {
      const g = r.def;
      let ok = true;
      if ((g.lvl || 0) > lvlMax) ok = false;
      if (ok && this.tabFilter && this.tabFilter !== 'all' && g.tab !== this.tabFilter) ok = false;
      if (ok && q) {
        const hay = (key + ' ' + (g.l || '') + ' ' + (g.a || '') + ' ' + g.tab + ' ' + g.grp).toLowerCase();
        if (hay.indexOf(q) < 0) ok = false;
      }
      r.row.classList.toggle('hidden', !ok);
      if (ok) shown++;
    }
    this.el.querySelectorAll('.psection').forEach(sec => {
      const any = [...sec.querySelectorAll('.prow')].some(r => !r.classList.contains('hidden'));
      sec.classList.toggle('hidden', !any);
    });
    if (this.filterTabs) {
      const all = this.filterTabs.querySelectorAll('.pchip');
      if (this.search) all.forEach(b => b.classList.toggle('active', b.dataset.tab === 'all'));
    }
  }
  refresh() {
    for (const [key, r] of this.rows) {
      const g = r.def;
      const v = this.cfg.get(key);
      if (g.t === 'B') r.input.checked = !!v;
      else if (g.t === 'S') r.input.value = v;
      else if (g.t === 'C') {
        if (r.input.type === 'color') r.input.value = v;
        const hex = r.row.querySelector('.phex');
        if (hex) hex.value = v;
      } else {
        r.input.value = v;
        if (r.readout) r.readout.value = fmt(v, decimals(g.st || 0.01));
      }
    }
    this.applyFilters();
  }

  /* ------------------------------------------------------------- palette --- */
  openPalette() {
    const p = this.el.querySelector('#palette');
    p.classList.remove('hidden');
    const inp = this.el.querySelector('#paletteInput');
    inp.value = this.search || '';
    inp.focus();
    inp.select();
    this.palIndex = 0;
    this.paletteList();
    inp.oninput = () => { this.palIndex = 0; this.paletteList(); };
    inp.onkeydown = (e) => this.paletteKey(e);
  }
  closePalette() { this.el.querySelector('#palette').classList.add('hidden'); }
  paletteItems(q) {
    const items = [];
    for (const m of MODES) items.push({ label: m.name, sub: 'mode · ' + m.tag, run: () => this.hooks.onMode(m.id) });
    for (const p of PRESETS) items.push({ label: p.n, sub: 'preset · ' + (p.note || '').slice(0, 70), run: () => this.hooks.onPreset(p) });
    for (const g of GROUPS) {
      const key = keyOf(g);
      items.push({
        label: (g.l || key) + '  ' + (g.t === 'B' ? (this.cfg.get(key) ? '[on]' : '[off]') : `= ${this.cfg.get(key)}`),
        sub: `${g.tab} · ${g.grp}` + (g.a ? ' · ' + g.a : ''),
        param: g,
        run: () => {
          const g2 = g;
          if (g2.t === 'B') this.cfg.set(key, !this.cfg.get(key));
          else if (g2.t === 'S') this.cfg.set(key, g2.o[(g2.o.indexOf(this.cfg.get(key)) + 1) % g2.o.length]);
          else this.cfg.set(key, g2.v);
          this.refresh();
          if (this.hooks.onChange) this.hooks.onChange(key, false);
          this.hooks.onReveal && this.hooks.onReveal(key);
        },
      });
    }
    for (const c of CLAIMS) items.push({ label: c.t, sub: `claim ${c.id} · ${c.cat} · ${c.band[0]}–${c.band[1]}`, run: () => this.hooks.onClaim && this.hooks.onClaim(c) });
    items.push({ label: 'Randomise everything', sub: 'command', run: () => this.hooks.onRandom && this.hooks.onRandom() });
    items.push({ label: 'Copy share link', sub: 'command', run: () => this.hooks.onCopyLink && this.hooks.onCopyLink() });
    items.push({ label: 'Save PNG', sub: 'command', run: () => this.hooks.onShot && this.hooks.onShot() });
    const qq = (q || '').toLowerCase().trim();
    if (!qq) return items.slice(0, 60);
    return items
      .map(it => ({ it, score: scoreMatch((it.label + ' ' + it.sub).toLowerCase(), qq) }))
      .filter(x => x.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 60)
      .map(x => x.it);
  }
  paletteList() {
    const q = this.el.querySelector('#paletteInput').value;
    this.palItems = this.paletteItems(q);
    const list = this.el.querySelector('#paletteList');
    list.innerHTML = '';
    this.palItems.forEach((it, i) => {
      const d = this.h('div', 'palitem' + (i === this.palIndex ? ' sel' : ''));
      d.innerHTML = `<span class="pl">${escapeHtml(it.label)}</span><span class="ps">${escapeHtml(it.sub)}</span>`;
      d.addEventListener('click', () => { it.run(); this.closePalette(); });
      d.addEventListener('mouseenter', () => { this.palIndex = i; this.paletteList(); });
      list.appendChild(d);
    });
  }
  paletteKey(e) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const n = this.palItems.length || 1;
      this.palIndex = (this.palIndex + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
      this.paletteList();
      const sel = this.el.querySelector('.palitem.sel');
      if (sel) sel.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const it = this.palItems[this.palIndex];
      if (it) { it.run(); this.closePalette(); }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      this.closePalette();
    }
  }

  /* --------------------------------------------------------- preset menu --- */
  togglePresetMenu() {
    const menu = this.el.querySelector('#presetMenu');
    if (!menu.classList.contains('hidden')) { menu.classList.add('hidden'); return; }
    menu.innerHTML = '';
    for (const p of PRESETS) {
      const b = this.h('button', 'mitem');
      b.innerHTML = `<span class="pn">${p.n}</span><span class="pm">${(MODES.find(m => m.id === p.mode) || {}).name || p.mode}</span><span class="pd">${p.note || ''}</span>`;
      b.addEventListener('click', () => { menu.classList.add('hidden'); this.hooks.onPreset && this.hooks.onPreset(p); });
      menu.appendChild(b);
    }
    menu.classList.remove('hidden');
  }
  toast(msg) {
    const t = this.el.querySelector('#toast');
    if (!t) return;
    t.textContent = msg;
    t.classList.add('show');
    clearTimeout(this.toastT);
    this.toastT = setTimeout(() => t.classList.remove('show'), 2600);
  }
}

/* ------------------------------------------------------------------ helpers -- */
function decimals(step) {
  const s = String(step);
  const i = s.indexOf('.');
  return i < 0 ? 0 : Math.min(4, s.length - i - 1);
}
function fmt(v, d) {
  if (typeof v === 'number') {
    if (Math.abs(v) >= 10000) return v.toFixed(0);
    return v.toFixed(d);
  }
  return String(v);
}
/* Honesty markers: a knob that is an artistic multiplier, and a knob that is an
   approximation of something a real solver would do exactly, both say so. */
const ART = new Set(['u_dnScale', 'u_opd', 'u_phaseScale', 'u_xtlWalk', 'u_fieldScale', 'u_speckle',
  'u_vWarp', 'u_vSwirl', 'u_vGrad', 'u_ci', 'u_xtlCloudy', 'u_shake', 'u_grain', 'u_vignette', 'u_crt',
  'u_scan', 'u_glyphMix', 'u_palMix', 'u_flowGain', 'u_report', 'u_lcStress']);
const APPROX = new Set(['u_lcSlices', 'u_volt', 'u_prt', 'u_lcTiltProf', 'u_lcTwistProf', 'u_xtlFaces',
  'u_memoryDiff', 'u_vOct', 'u_lcOrder', 'u_xtlElong', 'u_fSlope', 'u_trateApprox']);
function paramFlag(g) {
  const key = g.k || g.u;
  if (ART.has(key)) return { text: '✱', why: 'an artistic multiplier rather than a physical one' };
  if (APPROX.has(key)) return { text: '≈', why: 'a model or approximation, described in the tooltip' };
  if (g.a && /approx|model|fictional|not to scale/i.test(g.a)) return { text: '≈', why: g.a };
  return null;
}
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
/* a very small fuzzy matcher: subsequence with a bonus for word starts */
function scoreMatch(hay, q) {
  if (!q) return 1;
  let score = 0, hi = 0;
  for (let i = 0; i < q.length; i++) {
    const c = q[i];
    if (c === ' ') continue;
    const at = hay.indexOf(c, hi);
    if (at < 0) return 0;
    score += 1 + (at === 0 || hay[at - 1] === ' ' ? 2 : 0);
    hi = at + 1;
  }
  return score;
}