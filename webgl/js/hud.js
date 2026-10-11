/* =============================================================================
   hud.js — the readouts that travel with the picture.

   The HUD is not decoration: it prints the numbers that make the image
   falsifiable. Retardance in nanometres, the order at which the fringes land,
   the voltage as a fraction of threshold, how many traces the ground is holding
   and how fast it forgets them. Screenshot the page and you have the settings.
   ============================================================================= */
'use strict';

import { MODES, CLAIMS, RAMPS } from './params.js';
import { rampCss } from './ramps.js';
import { keyOf } from './state.js';
import { MODE_GLSL } from './renderer.js';

const CAT_NAME = { material: 'material', bio: 'biological', geo: 'geophysical', cognitive: 'cognitive', tech: 'technical', eco: 'ecological' };
const CAT_HUE = { material: 28, bio: 132, geo: 202, cognitive: 288, tech: 320, eco: 168 };

export class Hud {
  constructor(cfg, el) {
    this.cfg = cfg;
    this.el = el;
    this.hudEl = el.querySelector('#hud');
    this.legendEl = el.querySelector('#legend');
    this.statsEl = el.querySelector('#stats');
    this.errEl = el.querySelector('#err');
    this.lastClaim = 0;
  }
  /* ------------------------------------------------------------- numbers --- */
  numbers(stats) {
    const c = this.cfg;
    const ne = c.get('u_ne'), no = c.get('u_no');
    const dmm = c.get('u_thick');
    const dn = (ne - no) * c.get('u_dnScale');
    const ret = dn * dmm * 1000;                      /* nm */
    const lam = c.get('u_lambda');
    const order = ret / lam;
    const volt = c.get('u_volt');
    const decay = c.get('u_decay');
    const trace = { mixed: 'mixed', root: 'roots', shell: 'shells', foot: 'footprints', crack: 'fractures', rootless: 'feeding traces', needle: 'needle marks' }[c.get('u_traceKind')] || '';
    const mem = c.get('u_memoryOn') ? `${c.get('u_memoryRes')}² field` : 'off';
    const retention = decay < 0.001 ? 'permanent' : decay > 0.6 ? 'a tide' : decay > 0.2 ? 'a season' : decay > 0.06 ? 'a decade' : 'a few centuries';
    return { ne, no, dn, dmm, ret, lam, order, volt, decay, trace, mem, retention };
  }

  /* ---------------------------------------------------------------- update -- */
  update(stats, clock) {
    const c = this.cfg;
    const n = this.numbers(stats);
    const mode = MODES.find(m => m.id === c.mode) || MODES[0];
    if (c.get('hud')) {
      const items = [];
      items.push(['mode', mode.name, mode.tag]);
      items.push(['epoch', `${c.get('u_epoch').toFixed(1)} ± ${(c.get('u_span') / 2).toFixed(1)} yr`, 'the window the annotations and evidence bands are drawn from']);
      items.push(['Δn', n.dn.toFixed(4), `n_e ${n.ne.toFixed(3)} − n_o ${n.no.toFixed(3)} at 550 nm, ×${c.get('u_dnScale').toFixed(2)}`]);
      items.push(['Γ', `${n.ret.toFixed(0)} nm`, `${n.dn.toFixed(3)} × ${n.dmm.toFixed(1)} µm — the retardance of the layer`]);
      items.push(['orders', `${n.order.toFixed(2)} λ`, `at ${n.lam} nm · ${c.get('u_bands')} spectral bands`]);
      items.push(['NA', `${c.get('u_na').toFixed(2)}`, `${c.get('u_rays')} oblique rays per pixel, cone ${c.get('u_raySpread').toFixed(2)}`]);
      items.push(['drive', `${n.volt.toFixed(2)} V/Vth`, `${c.get('u_voltFreq').toFixed(0)} Hz · θ pre ${c.get('u_prt').toFixed(1)}°`]);
      items.push(['polars', `${c.get('u_polP').toFixed(0)}° / ${c.get('u_polA').toFixed(0)}° ${c.get('u_polMode')}`, 'polariser and analyser, degrees from the lab x axis']);
      items.push(['traces', `${c.get('u_traceCount')} ${n.trace}`, 'drawn into the memory field, then left to decay']);
      items.push(['memory', n.mem, `decay ${n.decay.toFixed(3)}/s → a record lasting about ${n.retention}`]);
      items.push(['aux', MODE_GLSL[c.mode] ? MODE_GLSL[c.mode].aux : '', 'what the mode writes into its auxiliary channel, which the palette can colour by']);
      items.push(['palette', c.get('u_palette') === 'none' ? 'none' : `${c.get('u_palette')} ← ${c.get('u_palSource')}`, 'the colour added on top']);
      this.hudEl.innerHTML = items
        .filter(([k, v]) => v !== undefined && v !== '')
        .map(([k, v, why]) => `<div class="hrow" title="${why}"><span class="hk">${k}</span><span class="hv">${v}</span></div>`)
        .join('');
      this.hudEl.classList.remove('hidden');
      const corner = c.get('hudCorner');
      this.hudEl.className = 'overlay hud-' + corner + (c.get('hudBg') ? ' bg' : '');
      this.hudEl.style.setProperty('--hud-scale', c.get('hudScale'));
      this.hudEl.style.opacity = c.get('hudOpacity');
      this.hudEl.style.display = c.get('hudItems') ? '' : 'none';
    } else {
      this.hudEl.classList.add('hidden');
    }

    /* the claim of the moment: whichever claim's window contains the epoch */
    const ep = c.get('u_epoch');
    const inWindow = CLAIMS.filter(cl => ep >= cl.band[0] && ep <= cl.band[1]);
    const showClaims = c.get('hudClaims');
    if (showClaims && inWindow.length) {
      const cl = inWindow[this.lastClaim % inWindow.length];
      this.claimLine = `<b>${cl.band[0]}–${cl.band[1]}</b> ${cl.t} <i>(${cl.lvl})</i>`;
    } else this.claimLine = '';

    if (c.get('annotations') && this.claimLine) {
      this.hudEl.insertAdjacentHTML('beforeend', `<div class="hclaim">${this.claimLine}</div>`);
    }

    /* performance */
    if (c.get('stats') && stats) this.statsEl.innerHTML =
      `<span title="frames per second">${stats.fps.toFixed(0)} fps</span>` +
      `<span title="milliseconds of GPU work per frame">${stats.ms.toFixed(1)} ms</span>` +
      `<span title="render passes this frame">${stats.passes} passes</span>` +
      `<span title="the target the mode renders into">${stats.w}×${stats.h}${stats.hdr ? ' HDR' : ''}</span>` +
      `<span title="traces stamped into the memory field on the last frame">${stats.stamps} stamps</span>` +
      `<span title="adaptive render scale">×${(stats.scale || 1).toFixed(2)}</span>`;
    else this.statsEl.innerHTML = '';
    this.legend(stats, n, mode);
  }

  /* --------------------------------------------------------------- legend --- */
  legend(stats, n, mode) {
    if (!this.cfg.get('legend')) { this.legendEl.classList.add('hidden'); return; }
    this.legendEl.classList.remove('hidden');
    const c = this.cfg;
    const rows = [];
    rows.push(`<div class="lhead">${mode.name} <span>${mode.tag}</span></div>`);
    rows.push(`<div class="lsub">${mode.blurb}</div>`);

    /* what the colours mean, derived from the live settings */
    const pal = c.get('u_palette');
    if (pal !== 'none') {
      rows.push(`<div class="lsec">palette</div>`);
      rows.push(`<div class="lramp">
        <span class="lsw">${rampSwatch(pal)}</span>
        <span>${pal} · ${c.get('u_palSource')} · ${c.get('u_palCycles')} cycles</span></div>`);
      rows.push(`<div class="lsub">${MODE_GLSL[c.mode] ? 'aux = ' + MODE_GLSL[c.mode].aux : ''}</div>`);
    }
    const items = [];
    if (c.mode === 'substrata' || c.mode === 'weathering' || c.mode === 'hologram') {
      items.push([c.get('u_vTint'), 'body colour']);
      items.push([c.get('u_vTint2'), 'second colour']);
      items.push([c.get('u_sky'), 'sky / ambient']);
      items.push([c.get('u_ground'), 'ground bounce']);
    }
    if (c.mode === 'director' || c.mode === 'conoscope') {
      items.push(['hue', `director azimuth, ${c.get('u_lcTwist').toFixed(0)}° of twist`]);
      items.push(['brightness', `tilt from the surface (V/Vth ${n.volt.toFixed(2)})`]);
      items.push(['#' + c.get('u_lightCol').slice(1), 'lamp']);
    }
    if (c.mode === 'crystal') {
      items.push([c.get('u_xtlPleochro'), 'pleochroic tint']);
      items.push(['hue', 'walk-off split: cyan ordinary, orange extraordinary']);
    }
    if (items.length) {
      rows.push(`<div class="lsec">colour</div>`);
      for (const [col, lab] of items) {
        const sw = String(col).startsWith('#') ? `<span class="lsw2" style="background:${col}"></span>` : '';
        rows.push(`<div class="litem">${sw}<span>${lab}</span></div>`);
      }
    }
    /* the claims, with their domains — the mosaic's key */
    rows.push(`<div class="lsec">evidence overlay${c.get('u_mosaic') > 0.01 ? ` (${(c.get('u_mosaic') * 100).toFixed(0)}% on)` : ''}</div>`);
    const withMosaic = c.get('u_mosaic') > 0.01;
    for (const cat of Object.keys(CAT_NAME)) {
      const sw = withMosaic ? `<span class="lsw2" style="background:hsl(${CAT_HUE[cat]} 45% 52%)"></span>` : '<span class="lsw2 dim"></span>';
      rows.push(`<div class="litem">${sw}<span>${CAT_NAME[cat]}</span></div>`);
    }
    rows.push(`<div class="lsub">fill = confidence: solid measured · hatched plausible · open single-study. The bar in each cell is the year window.</div>`);
    /* the instrument, in numbers */
    rows.push(`<div class="lsec">instrument</div>`);
    rows.push(`<div class="litem"><span>Γ = Δn·d = <b>${n.ret.toFixed(0)} nm</b> (${n.order.toFixed(2)} λ at ${n.lam} nm)</span></div>`);
    rows.push(`<div class="litem"><span>memory retains for about <b>${n.retention}</b></span></div>`);
    if (c.get('u_slotB')) rows.push(`<div class="litem"><span>slot B: <b>${c.get('u_slotBMode')}</b> · ${c.get('u_splitMode')} at ${(c.get('u_split') * 100).toFixed(0)}%</span></div>`);
    this.legendEl.innerHTML = rows.join('');
  }

  error(msg) {
    this.errEl.classList.remove('hidden');
    this.errEl.classList.add('bg');
    this.errEl.innerHTML = msg;
  }
  clearError() { this.errEl.classList.add('hidden'); }
}

function rampSwatch(name) {
  const i = RAMPS.indexOf(name);
  if (i < 0) return '';
  const stops = [];
  for (let k = 0; k <= 10; k++) stops.push(rampCss(RAMPS[i], k / 10));
  return `<div class="lbar">${stops.map(col => `<i style="background:${col}"></i>`).join('')}</div>`;
}
