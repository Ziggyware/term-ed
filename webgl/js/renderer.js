/* =============================================================================
   renderer.js — the WebGL2 engine.

   Six passes, in this order:
     1  substrate memory   decay / diffuse / drift, then stamp new traces
     2  scene A            the selected mode, into a float target
     3  scene B            optional second rendering, for split and anaglyph
     4  bloom              bright pass and two blurs at half resolution
     5  composite          split, palette, evidence mosaic, glyphs, grade, tube
     6  (capture)          PNG straight out of the drawing buffer

   Every mode is the same program shape: globals + lib + optics + marcher + mode,
   and every knob lives in packed vec4 uniforms, so switching modes costs one
   program switch and nothing else.
   ============================================================================= */
'use strict';

import { GROUPS, RAMPS, MODES } from './params.js';
import { keyOf } from './state.js';
import { layoutUniforms, packUniforms, DERIVED, vectorCost } from './uniforms.js';

/* Which march file each mode needs, and what its auxiliary channel reports. */
export const MODE_GLSL = {
  substrata:  { march: 'height', src: 'mode-substrata.glsl',  aux: 'what the ground still remembers' },
  director:   { march: 'sdf',    src: 'mode-director.glsl',   aux: 'how far the director has tilted' },
  conoscope:  { march: 'none',   src: 'mode-conoscope.glsl',  aux: 'retardance through the cell' },
  hologram:   { march: 'none',   src: 'mode-hologram.glsl',   aux: 'phase of the recorded wavefront' },
  crystal:    { march: 'sdf',    src: 'mode-crystal.glsl',    aux: 'retardance along the chord' },
  weathering: { march: 'height', src: 'mode-weathering.glsl',  aux: 'depth of the weathering rind' },
  causal:     { march: 'none',   src: 'mode-causal.glsl',     aux: 'strength of the field at the top layer' },
};

const SHADER_FILES = [
  'globals.glsl', 'lib.glsl', 'optics.glsl', 'glyphs.glsl', 'claims.glsl',
  'march-sdf.glsl', 'march-height.glsl', 'mem-update.glsl', 'mem-stamp.glsl',
  'bloom.glsl', 'composite.glsl', 'common.vert.glsl', 'stamp.vert.glsl',
  'mode-substrata.glsl', 'mode-director.glsl', 'mode-conoscope.glsl',
  'mode-hologram.glsl', 'mode-crystal.glsl', 'mode-weathering.glsl', 'mode-causal.glsl',
];

const PRELUDE = `
precision highp float;
precision highp int;
out vec4 outColor;
`;

const AA_TAIL = `
void main() {
  vec4 acc = vec4(0.0);
  int aa = clamp(u_aa, 1, 4);
  int samples = clamp(max(aa * aa, clamp(u_rays, 1, 16)), 1, 16);
  for (int i = 0; i < 16; i++) {
    if (i >= samples) break;
    /* one aperture point per sample: it shifts the origin and re-aims the ray,
       so defocus and the acceptance cone are the same mechanism, not two */
    g_cone = (samples > 1) ? diskSample(i, samples) : vec2(0.0);
    acc += render(gl_FragCoord.xy + aaOffset(i, samples, gl_FragCoord.xy));
  }
  outColor = acc / float(samples);
}
`;

export class Renderer {
  constructor(canvas, cfg, opts) {
    this.canvas = canvas;
    this.cfg = cfg;
    this.log = (opts && opts.log) || (() => {});
    this.gl = canvas.getContext('webgl2', {
      alpha: false, antialias: false, depth: false, stencil: false,
      preserveDrawingBuffer: false, powerPreference: 'high-performance',
      premultipliedAlpha: false, desynchronized: false,
    });
    if (!this.gl) throw new Error('WebGL2 is required, and this browser did not give it us.');
    const gl = this.gl;
    this.floatTargets = !!gl.getExtension('EXT_color_buffer_float');
    this.floatTargetsHalf = !!gl.getExtension('EXT_color_buffer_half_float');
    this.linFilterFloat = !!gl.getExtension('OES_texture_float_linear');
    this.hdrFormat = this.floatTargetsHalf ? gl.RGBA16F : (this.floatTargets ? gl.RGBA32F : gl.RGBA8);
    this.hdrType = this.floatTargetsHalf ? gl.HALF_FLOAT : (this.floatTargets ? gl.FLOAT : gl.UNSIGNED_BYTE);
    this.usingHDR = this.floatTargetsHalf || this.floatTargets;
    if (!this.usingHDR) this.log('warn: floating-point render targets unavailable — the memory field and bloom fall back to 8 bits.');

    this.limits = {
      uniformVectors: gl.getParameter(gl.MAX_FRAGMENT_UNIFORM_VECTORS),
      texture: gl.getParameter(gl.MAX_TEXTURE_SIZE),
      renderbuffer: gl.getParameter(gl.MAX_RENDERBUFFER_SIZE),
      vertexTex: gl.getParameter(gl.MAX_VERTEX_TEXTURE_IMAGE_UNITS),
    };
    this.layout = layoutUniforms(GROUPS, RAMPS);
    if (vectorCost(this.layout) > this.limits.uniformVectors) {
      throw new Error(`This GPU allows ${this.limits.uniformVectors} uniform vectors in a fragment shader; the page needs ${vectorCost(this.layout)}.`);
    }

    this.sources = new Map();
    this.programs = new Map();
    this.targets = null;
    this.memory = null;
    this.stamps = [];
    this.stampAccum = 0;
    this.rng = null;
    this.seed = 1;
    this.bSlotMode = null;
    this.frameTime = 16;
    this.scaleBoost = 1;
    this.lcLag = undefined;
    this.emptyTex = null;
    this.quadVao = gl.createVertexArray();
    this.stampVao = gl.createVertexArray();
    this.wantShot = null;
    this.stats = { fps: 0, ms: 0, passes: 0, pixels: 0 };
  }

  /* ------------------------------------------------------------- shaders -- */
  async fetchSources(base) {
    await Promise.all(SHADER_FILES.map(async (f) => {
      const res = await fetch(`${base}shaders/${f}`);
      if (!res.ok) throw new Error(`could not load shaders/${f}`);
      this.sources.set(f, await res.text());
    }));
    return this;
  }
  get(f) {
    const s = this.sources.get(f);
    if (s === undefined) throw new Error(`shader file not loaded: ${f}`);
    return s;
  }
  compile(vsSrc, fsSrc, label) {
    const gl = this.gl;
    const PRELUDE = '#version 300 es\n\n';
    const PRELUDE_LINES = PRELUDE.split('\n').length - 1; // 2
    const CONTEXT = 3;

    // Numbered excerpt of only the lines the driver flagged (± context).
    const excerpt = (src, log) => {
      const flagged = new Set();
      // ANGLE/Chrome: "ERROR: 0:12: ..." — older drivers: "0(12) : ..."
      for (const m of log.matchAll(/^(?:ERROR|WARNING):\s*\d+:(\d+):/gim))
        flagged.add(parseInt(m[1], 10) - PRELUDE_LINES);
      for (const m of log.matchAll(/^\d+\((\d+)\)\s*:/gim))
        flagged.add(parseInt(m[1], 10) - PRELUDE_LINES);

      const srcLines = src.split('\n');
      if (!flagged.size) { // no parseable locations — dump everything, numbered
        return srcLines.map((l, i) => `${String(i + 1).padStart(4)} | ${l}`).join('\n');
      }
      const keep = new Set();
      for (const n of flagged)
        for (let i = n - CONTEXT; i <= n + CONTEXT; i++)
          if (i >= 1 && i <= srcLines.length) keep.add(i);

      const out = [];
      let last = 0;
      for (const n of [...keep].sort((a, b) => a - b)) {
        if (n - last > 1) out.push('      ...');
          out.push(`${flagged.has(n) ? '>>>' : '   '}${String(n).padStart(4)} | ${srcLines[n - 1]}`);
        last = n;
      }
      return out.join('\n');
    };

    const sh = (type, src, lbl) => {
      const s = gl.createShader(type);
      gl.shaderSource(s, PRELUDE + src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
        const log = gl.getShaderInfoLog(s) || 'unknown error';
        throw new Error(`[${label} ${lbl}] shader compile failed\n${log}\n${excerpt(src, log)}`);
      }
      return s;
    };
                                                                                                
              
    
    const v = sh(gl.VERTEX_SHADER, vsSrc, 'vs');
    const f = sh(gl.FRAGMENT_SHADER, fsSrc, 'fs');
    const p = gl.createProgram();
    gl.attachShader(p, v); gl.attachShader(p, f); gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
      throw new Error(`[${label} link] ${gl.getProgramInfoLog(p)}`);
    }
    gl.deleteShader(v); gl.deleteShader(f);
    const loc = new Map();
    return {
      program: p, label,
      u(name) {
        if (loc.has(name)) return loc.get(name);
        const l = gl.getUniformLocation(p, name);
        loc.set(name, l);
        return l;
      },
    };
  }
  programFor(kind, modeId) {
    const key = kind + (modeId ? ':' + modeId : '');
    if (this.programs.has(key)) return this.programs.get(key);
    const gl = this.gl;
    const g = this.get('globals.glsl');
    let prog;
    if (kind === 'mode') {
      const m = MODE_GLSL[modeId];
      const fs = PRELUDE + g + this.get('lib.glsl') + this.get('optics.glsl')
        + (m.march === 'sdf' ? this.get('march-sdf.glsl') : '')
        + (m.march === 'height' ? this.get('march-height.glsl') : '')
        + this.get(m.src) + AA_TAIL;
      prog = this.compile( this.get('common.vert.glsl'), fs, 'mode-' + modeId);
    } else if (kind === 'composite') {
      const fs = PRELUDE + g + this.get('lib.glsl') + this.get('glyphs.glsl') + this.get('claims.glsl')
        + this.get('composite.glsl') + '\nvoid main() { outColor = vec4(compositeMain(), 1.0); }\n';
      prog = this.compile(this.get('common.vert.glsl'), fs, 'composite');
    } else if (kind === 'bright' || kind === 'blur') {
      const body = kind === 'bright' ? 'brightPassMain()' : 'blurMain()';
      const fs = PRELUDE + g + this.get('lib.glsl') + this.get('bloom.glsl')
        + `\nvoid main() { outColor = vec4(${body}, 1.0); }\n`;
      prog = this.compile(this.get('common.vert.glsl'), fs, kind);
    } else if (kind === 'mem-update') {
      const fs = PRELUDE + g + this.get('lib.glsl') + this.get('mem-update.glsl')
        + '\nvoid main() { memMain(); }\n';
      prog = this.compile(this.get('common.vert.glsl'), fs, kind);
    } else if (kind === 'mem-stamp') {
      const fs = PRELUDE + g + this.get('lib.glsl') + this.get('mem-stamp.glsl')
        + '\nvoid main() { stampMain(); }\n';
      prog = this.compile(this.get('stamp.vert.glsl'), fs, kind);
    } else {
      throw new Error('unknown program kind ' + kind);
    }
    this.programs.set(key, prog);
    return prog;
  }

  /* --------------------------------------------------------------- memory -- */
  makeTexture(w, h, internal, type, filter) {
    const gl = this.gl;
    const t = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, t);
    gl.texImage2D(gl.TEXTURE_2D, 0, internal, w, h, 0, gl.RGBA, type, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    return t;
  }
  makeTarget(w, h, half) {
    const gl = this.gl;
    const internal = half ? this.hdrFormat : gl.RGBA8;
    const type = half ? this.hdrType : gl.UNSIGNED_BYTE;
    const tex = this.makeTexture(w, h, internal, type, gl.LINEAR);
    const fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    const ok = gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    if (!ok) throw new Error('render target incomplete (floating point targets may be unsupported)');
    return { tex, fbo, w, h };
  }
  dispose(o) {
    const gl = this.gl;
    if (!o) return;
    for (const k in o) {
      if (o[k] && o[k].tex) { gl.deleteTexture(o[k].tex); gl.deleteFramebuffer(o[k].fbo); }
      else if (o[k] && o[k].texture) gl.deleteTexture(o[k]);
    }
  }
  ensureMemory(cfg) {
    const gl = this.gl;
    const want = Math.max(128, Math.min(cfg.get('u_memoryRes') | 0, Math.min(2048, this.limits.texture)));
    if (this.memory && this.memory.res === want) return;
    if (this.memory) { gl.deleteTexture(this.memory.read.tex); gl.deleteFramebuffer(this.memory.read.fbo); gl.deleteTexture(this.memory.write.tex); gl.deleteFramebuffer(this.memory.write.fbo); }
    const a = this.makeTarget(want, want, true);
    const b = this.makeTarget(want, want, true);
    /* clear both to machine-zero, then let frame 0 initialise */
    for (const t of [a, b]) {
      gl.bindFramebuffer(gl.FRAMEBUFFER, t.fbo);
      gl.clearColor(0.5, 0, cfg.get('u_wet'), 0.5);
      gl.clear(gl.COLOR_BUFFER_BIT);
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.memory = { read: a, write: b, res: want };
    this.memoryFrame = -1;
  }
  ensureEmpty() {
    if (this.emptyTex) return;
    const gl = this.gl;
    const t = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, t);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([0, 0, 0, 0]));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    this.emptyTex = t;
  }

  /* -------------------------------------------------------------- uploads -- */
  upload(prog, packet, extra) {
    const gl = this.gl;
    for (let i = 0; i < this.layout.floats.length; i++) {
      const l = prog.u('uPk' + i);
      if (l) gl.uniform4fv(l, packet.f[i]);
    }
    for (let i = 0; i < this.layout.ints.length; i++) {
      const l = prog.u('uIk' + i);
      if (l) gl.uniform4iv(l, packet.i[i]);
    }
    for (let i = 0; i < this.layout.cols.length; i++) {
      const l = prog.u('uCk' + i);
      if (l) gl.uniform3fv(l, packet.c[i]);
    }
    const d = packet.d || {};
    for (const u of DERIVED) {
      const l = prog.u(u.n);
      if (!l) continue;
      const v = d[u.n];
      if (v === undefined) continue;
      if (u.t === 'V3') gl.uniform3f(l, v[0], v[1], v[2]);
      else if (u.t === 'I') gl.uniform1i(l, v | 0);
      else gl.uniform1f(l, v);
    }
    if (extra) for (const k in extra) {
      const l = prog.u(k);
      if (l) {
        const v = extra[k];
        if (Array.isArray(v)) gl.uniform1fv(l, v); else gl.uniform1f(l, v);
      }
    }
  }
  packetSet(packet, key, value) {
    const s = this.layout.slot.get(key);
    if (!s) return;
    if (s.type === 'C') { const t = packet.c[s.block]; t[0] = value[0]; t[1] = value[1]; t[2] = value[2]; }
    else if (s.type === 'F') packet.f[s.block][s.comp] = value;
    else packet.i[s.block][s.comp] = value | 0;
  }
  packConfig(cfg) {
    const values = GROUPS.map(g => ({ key: keyOf(g), def: g, raw: cfg.values[keyOf(g)] }));
    const p = packUniforms(this.layout, values, RAMPS);
    return { f: p.f, i: p.i, c: p.c, d: {} };
  }
  clonePacket(p) {
    return {
      f: p.f.map(a => a.slice()), i: p.i.map(a => a.slice()), c: p.c.map(a => a.slice()),
      d: Object.assign({}, p.d),
    };
  }

  /* --------------------------------------------------------------- drawing -- */
  drawQuad() {
    const gl = this.gl;
    gl.bindVertexArray(this.quadVao);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
  bindTarget(t) {
    const gl = this.gl;
    if (t) { gl.bindFramebuffer(gl.FRAMEBUFFER, t.fbo); gl.viewport(0, 0, t.w, t.h); }
    else { gl.bindFramebuffer(gl.FRAMEBUFFER, null); gl.viewport(0, 0, this.canvas.width, this.canvas.height); }
  }
  bindTex(unit, tex, samplerName, prog) {
    const gl = this.gl;
    gl.activeTexture(gl.TEXTURE0 + unit);
    gl.bindTexture(gl.TEXTURE_2D, tex || this.emptyTex);
    const l = prog.u(samplerName);
    if (l) gl.uniform1i(l, unit);
  }

  /* Deterministic trace injection: the rate you set, the family you chose, and
     whatever you clicked on. */
  injectTraces(cfg, dt, forced) {
    if (!cfg.get('u_memoryOn')) return 0;
    const rate = cfg.get('u_traceRate');
    this.stampAccum += rate * dt;
    let n = 0;
    while (this.stampAccum >= 1 && n < 4) {
      this.stampAccum -= 1;
      this.stamps.push(this.makeStamp(cfg));
      n++;
    }
    if (forced) { this.stamps.push(forced); n++; }
    return n;
  }
  makeStamp(cfg, pos) {
    const kindSel = cfg.get('u_traceKind');
    const kinds = ['mixed', 'root', 'shell', 'foot', 'crack', 'rootless', 'needle'];
    let kind = kinds.indexOf(kindSel) - 1;
    if (kind < 0) kind = Math.floor(this.rng() * 6);
    const size = cfg.get('u_imprintWidth') * 0.28;
    const wide = [1.9, 0.85, 1.0, 5.0, 1.6, 3.2][kind] || 1.0;
    return {
      x: pos ? pos[0] : 0.12 + 0.76 * this.rng(),
      y: pos ? pos[1] : 0.12 + 0.76 * this.rng(),
      rot: this.rng() * Math.PI * 2,
      sx: size * wide,
      sy: size,
      kind,
      depth: cfg.get('u_imprint') * (kind === 1 ? 0.5 : 1.0),
      seed: this.rng() * 100,
      born: performance.now(),
    };
  }
  drawStamps(cfg) {
    if (!this.stamps.length || !this.memory) return 0;
    const gl = this.gl;
    const prog = this.programFor('mem-stamp');
    gl.useProgram(prog.program);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    gl.bindVertexArray(this.stampVao);
    for (const s of this.stamps) {
      gl.uniform3f(prog.u('uStampPos'), s.x, s.y, s.rot);
      gl.uniform2f(prog.u('uStampSize'), Math.min(s.sx, 0.5), Math.min(s.sy, 0.5));
      gl.uniform1i(prog.u('uStampKind'), s.kind);
      gl.uniform1f(prog.u('uStampDepth'), s.depth);
      gl.uniform1f(prog.u('uStampSeed'), s.seed);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    }
    gl.disable(gl.BLEND);
    const n = this.stamps.length;
    this.stamps.length = 0;
    return n;
  }

  /* ------------------------------------------------------------------ frame -- */
  renderFrame(clock, cfg, punch) {
    const gl = this.gl;
    const t0 = performance.now();
    this.detTime = clock.time;
    this.ensureEmpty();
    if (!this.rng) { this.seed = 1; this.rng = this.rngFor(cfg); }
    const cssW = Math.max(320, this.canvas.clientWidth || 960);
    const cssH = Math.max(240, this.canvas.clientHeight || 540);
    const dpr = Math.min(window.devicePixelRatio || 1, cfg.get('dprCap'));
    const qual = { low: 0.55, medium: 0.75, high: 1, ultra: 1.35, insane: 1.8 }[cfg.get('quality')] || 1;
    let scale = cfg.get('renderScale') * qual * this.scaleBoost;
    let w = Math.max(160, Math.round(cssW * dpr * scale));
    let h = Math.max(120, Math.round(cssH * dpr * scale));
    const maxPix = 4200000;
    if (w * h > maxPix) { const k = Math.sqrt(maxPix / (w * h)); w = Math.round(w * k); h = Math.round(h * k); }
    if (!this.targets || this.targets.w !== w || this.targets.h !== h) {
      this.dispose(this.targets);
      const bw = Math.max(8, w >> 1), bh = Math.max(8, h >> 1);
      this.targets = {
        w, h,
        a: this.makeTarget(w, h, true),
        b: this.makeTarget(w, h, true),
        bright: this.makeTarget(bw, bh, true),
        blurA: this.makeTarget(bw, bh, true),
        blurB: this.makeTarget(bw, bh, true),
      };
      this.canvas.width = w; this.canvas.height = h;
    }
    const T = this.targets;
    const packet = this.packConfig(cfg);
    let passes = 0;

    /* ---- derived, shared by every pass ---- */
    const aspect = w / h;
    const tanHalf = Math.tan((cfg.get('u_fov') * Math.PI / 180) * 0.5);
    const cam = this.camera(cfg, clock, aspect);
    packet.d.uRes = [w, h, 1 / w];
    packet.d.uTime = this.detTime;
    packet.d.uPulse = clock.pulse;
    packet.d.uPulse2 = clock.pulsePrev;
    packet.d.uDT = clock.dt;
    packet.d.uFrame = this.memoryFrame === undefined ? 1 : Math.max(1, this.memoryFrame);
    packet.d.uMode = MODES.findIndex(m => m.id === cfg.mode);
    packet.d.uAspect = aspect;
    packet.d.uFov = cfg.get('u_fov');
    packet.d.uTanHalfFov = tanHalf;
    /* the cell lags the drive: a first-order lag, integrated here because it is
       the CPU that owns time. u_settle smooths the signal; u_tau is the cell. */
    const tau = Math.max(1e-4, cfg.get('u_tau'));
    const lagK = 1 - Math.exp(-clock.dt / tau);
    this.lcLag = (this.lcLag === undefined ? clock.pulse : this.lcLag) + (clock.pulse - (this.lcLag === undefined ? clock.pulse : this.lcLag)) * lagK;
    if (!(this.lcLag >= 0)) this.lcLag = clock.pulse;
    packet.d.uLC = Math.max(0, Math.min(1, this.lcLag));
    packet.d.uMouse = [this.mouse ? this.mouse[0] : 0, this.mouse ? this.mouse[1] : 0, this.mouse ? 1 : 0];
    packet.d.uCamO = cam.o; packet.d.uCamR = cam.r; packet.d.uCamU = cam.u; packet.d.uCamF = cam.f;
    packet.d.uLayout = [this.limits.uniformVectors, w, h];
    packet.d.uVecA = cam.o.slice();

    /* ---- 1. the substrate memory ---- */
    if (cfg.get('u_memoryOn')) {
      this.ensureMemory(cfg);
      const upd = this.programFor('mem-update');
      gl.useProgram(upd.program);
      const memPacket = this.clonePacket(packet);
      memPacket.d.uRes = [this.memory.res, this.memory.res, 1 / this.memory.res];
      this.upload(upd, memPacket);
      this.bindTarget(this.memory.write);
      this.bindTex(0, this.memory.read.tex, 'uPrev', upd);
      this.drawQuad(); passes++;
      this.injectTraces(cfg, clock.dt * ((cfg.get('u_anim') && !cfg.get('paused')) ? 1 : 0), punch);
      const nStamps = this.drawStamps(cfg);
      this.bindTarget(null);
      const tmp = this.memory.read; this.memory.read = this.memory.write; this.memory.write = tmp;
      this.memoryFrame = (this.memoryFrame || 0) + 1;
      this.lastStamps = nStamps;
    }

    /* ---- 2. scene A ---- */
    const modeA = cfg.mode;
    const progA = this.programFor('mode', modeA);
    gl.useProgram(progA.program);
    this.upload(progA, packet);
    this.bindTex(0, this.memory ? this.memory.read.tex : this.emptyTex, 'uMemory', progA);
    this.bindTarget(T.a);
    this.drawQuad(); passes++;

    /* ---- 3. scene B ---- */
    const wantB = cfg.get('u_slotB');
    let modeB = null;
    if (wantB) {
      const how = cfg.get('u_slotBMode');
      modeB = how === 'previous' ? (cfg.prevMode || modeA) : modeA;
      const progB = this.programFor('mode', modeB);
      gl.useProgram(progB.program);
      const pb = this.clonePacket(packet);
      if (how === 'analyser90') this.packetSet(pb, 'u_polA', ((cfg.get('u_polA') + 90 + 540) % 360) - 180);
      else if (how === 'polariser45') this.packetSet(pb, 'u_polP', ((cfg.get('u_polP') + 45 + 540) % 360) - 180);
      else if (how === 'deltaT') pb.d.uTime = this.detTime + (cfg.get('u_period') || 4) * 0.5;
      this.upload(progB, pb);
      this.bindTex(0, this.memory ? this.memory.read.tex : this.emptyTex, 'uMemory', progB);
      this.bindTarget(T.b);
      this.drawQuad(); passes++;
    }
    this.bSlotMode = modeB;

    /* ---- 4. bloom ---- */
    const bloomAmt = cfg.get('u_bloom');
    if (bloomAmt > 0.001) {
      const bp = this.programFor('bright');
      gl.useProgram(bp.program);
      this.upload(bp, packet);
      this.bindTex(0, T.a.tex, 'uSrc', bp);
      this.bindTarget(T.bright);
      this.drawQuad(); passes++;
      const bl = this.programFor('blur');
      const radius = 2.5 + 6.0 * Math.min(2, bloomAmt);
      const bh = T.bright.h, bw = T.bright.w;
      gl.useProgram(bl.program);
      this.upload(bl, packet);
      this.bindTex(0, T.bright.tex, 'uSrc', bl);
      gl.uniform2f(bl.u('uBlurDir'), radius / bw, 0);
      this.bindTarget(T.blurA);
      this.drawQuad(); passes++;
      this.bindTex(0, T.blurA.tex, 'uSrc', bl);
      gl.uniform2f(bl.u('uBlurDir'), 0, radius / bh);
      this.bindTarget(T.blurB);
      this.drawQuad(); passes++;
    }

    /* ---- 5. composite ---- */
    const comp = this.programFor('composite');
    gl.useProgram(comp.program);
    const cp = this.clonePacket(packet);
    cp.d.uRes = [w, h, 1 / w];
    this.upload(comp, cp);
    this.bindTex(0, T.a.tex, 'uSceneA', comp);
    this.bindTex(1, wantB ? T.b.tex : this.emptyTex, 'uSceneB', comp);
    this.bindTex(2, bloomAmt > 0.001 ? T.blurB.tex : this.emptyTex, 'uBloom', comp);
    this.bindTarget(null);
    this.drawQuad(); passes++;

    /* ---- 6. capture ---- */
    let shot = null;
    if (this.wantShot) {
      try { shot = this.canvas.toDataURL('image/png'); } catch (e) { shot = null; }
      const cb = this.wantShot;
      this.wantShot = null;
      if (cb) cb(shot);
    }

    /* ---- timing, and the adaptive scale ---- */
    const ms = performance.now() - t0;
    this.frameTime = this.frameTime * 0.9 + ms * 0.1;
    if (cfg.get('autoQuality')) {
      const target = 1000 / Math.max(cfg.get('adaptTarget'), 20);
      if (this.frameTime > target * 1.35 && this.scaleBoost > 0.45) this.scaleBoost = Math.max(0.45, this.scaleBoost - 0.02);
      else if (this.frameTime < target * 0.7 && this.scaleBoost < 1) this.scaleBoost = Math.min(1, this.scaleBoost + 0.01);
    }
    this.stats = {
      fps: this.frameTime > 0 ? 1000 / this.frameTime : 0,
      ms: this.frameTime,
      passes,
      pixels: w * h,
      w, h,
      hdr: this.usingHDR,
      stamps: this.lastStamps || 0,
    };
    return this.stats;
  }
  rngFor(cfg) {
    const seed = Math.floor(cfg.get('u_globalSeed') * 1000) ^ 0x9e3779b9;
    return (() => {
      let a = seed >>> 0;
      return () => {
        a = (a + 0x6D2B79F5) | 0;
        let t = Math.imul(a ^ (a >>> 15), 1 | a);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      };
    })();
  }

  /* ----------------------------------------------------------------- camera -- */
  /* One place for the camera, so every mode agrees about where it is looking. */
  camera(cfg, clock, aspect) {
    const az = cfg.get('u_azim') * Math.PI / 180;
    const el = cfg.get('u_elev') * Math.PI / 180;
    const roll = cfg.get('u_roll') * Math.PI / 180;
    const dist = cfg.get('u_dist');
    const orbit = cfg.get('u_orbit');
    const path = cfg.get('u_camPath');
    const amp = cfg.get('u_pathAmp');
    const t = this.detTime;
    let azm = az + orbit * t * 0.35;
    let elm = el, distm = dist, tx = cfg.get('u_targetX'), ty = cfg.get('u_targetY'), tz = cfg.get('u_targetZ');
    if (path === 'spiral') { azm += t * 0.12; distm *= 1 + 0.12 * amp * Math.sin(t * 0.31); elm += 0.10 * amp * Math.sin(t * 0.21); }
    else if (path === 'lissajous') { azm += amp * Math.sin(t * 0.23); elm += amp * 0.4 * Math.sin(t * 0.37); tx += amp * 0.3 * Math.sin(t * 0.19); }
    else if (path === 'pendulum') { azm += amp * 0.6 * Math.sin(t * 0.4); }
    else if (path === 'breath') { distm *= 1 + 0.06 * amp * Math.sin(t * 0.5); }
    else if (path === 'walk') { tx += amp * 0.6 * Math.sin(t * 0.15); tz += amp * 0.6 * Math.cos(t * 0.11); }
    const shake = cfg.get('u_shake');
    if (shake > 0) {
      const s = (k) => (this.noise1(t * 1.7 + k) - 0.5) * 2;
      azm += s(1.0) * shake * 0.05; elm += s(7.0) * shake * 0.04;
      tx += s(3.0) * shake * 0.05; ty += s(5.0) * shake * 0.05;
    }
    const px = cfg.get('u_parallax');
    if (px > 0 && this.mouse) { azm += this.mouse[0] * px * 0.25; elm += this.mouse[1] * px * 0.18; }
    const ce = Math.cos(elm), se = Math.sin(elm);
    const dir = [ce * Math.sin(azm), se, ce * Math.cos(azm)];
    const target = [tx, ty, tz];
    const o = [target[0] + dir[0] * distm, target[1] + dir[1] * distm, target[2] + dir[2] * distm];
    const f = [-dir[0], -dir[1], -dir[2]];
    let r = [f[2], 0, -f[0]];
    let rl = Math.hypot(r[0], r[1], r[2]) || 1;
    r = [r[0] / rl, r[1] / rl, r[2] / rl];
    let u = [
      r[1] * f[2] - r[2] * f[1],
      r[2] * f[0] - r[0] * f[2],
      r[0] * f[1] - r[1] * f[0],
    ];
    /* roll about the view axis */
    const cr = Math.cos(roll), sr = Math.sin(roll);
    const r2 = [r[0] * cr + u[0] * sr, r[1] * cr + u[1] * sr, r[2] * cr + u[2] * sr];
    const u2 = [u[0] * cr - r[0] * sr, u[1] * cr - r[1] * sr, u[2] * cr - r[2] * sr];
    return { o, f, r: r2, u: u2 };
  }
  noise1(x) {
    const i = Math.floor(x), f = x - i;
    const h = (n) => { const s = Math.sin(n * 127.1) * 43758.5453; return s - Math.floor(s); };
    const a = h(i), b = h(i + 1);
    const t = f * f * (3 - 2 * f);
    return a + (b - a) * t;
  }
}
