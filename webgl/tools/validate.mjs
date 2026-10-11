#!/usr/bin/env node
/* =============================================================================
   validate.mjs — everything that can be checked without a GPU.

   1. assembles every shader program exactly the way js/renderer.js does
   2. expands the #define alias table so a real GLSL parser can read it
   3. parses each program and reports syntax errors with line numbers
   4. reports undefined variables, undeclared functions and use-before-definition
   5. cross-checks the uniform aliases against js/params.js
   6. checks the uniform-vector budget against the WebGL2 minimum of 224
   ============================================================================= */
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { GROUPS, RAMPS } from '../js/params.js';
import { layoutUniforms, vectorCost, DERIVED, SAMPLERS } from '../js/uniforms.js';
import { RAMP_STOPS } from '../js/ramps.js';
import { MODE_GLSL } from '../js/renderer.js';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const read = (f) => readFileSync(join(root, 'js', 'shaders', f), 'utf8');

const PRELUDE = `#version 300 es
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

function assemble(kind, modeId) {
  const g = read('globals.glsl');
  if (kind === 'mode') {
    const m = MODE_GLSL[modeId];
    return PRELUDE + g + read('lib.glsl') + read('optics.glsl')
      + (m.march === 'sdf' ? read('march-sdf.glsl') : '')
      + (m.march === 'height' ? read('march-height.glsl') : '')
      + read(m.src) + AA_TAIL;
  }
  if (kind === 'composite') return PRELUDE + g + read('lib.glsl') + read('glyphs.glsl') + read('claims.glsl')
    + read('composite.glsl') + '\nvoid main() { outColor = vec4(compositeMain(), 1.0); }\n';
  if (kind === 'bright' || kind === 'blur') {
    const body = kind === 'bright' ? 'brightPassMain()' : 'blurMain()';
    return PRELUDE + g + read('lib.glsl') + read('bloom.glsl') + `\nvoid main() { outColor = vec4(${body}, 1.0); }\n`;
  }
  if (kind === 'mem-update') return PRELUDE + g + read('lib.glsl') + read('mem-update.glsl') + '\nvoid main() { memMain(); }\n';
  if (kind === 'mem-stamp') return PRELUDE + g + read('lib.glsl') + read('mem-stamp.glsl') + '\nvoid main() { stampMain(); }\n';
  if (kind === 'vs') return '#version 300 es\n' + read('common.vert.glsl');
  if (kind === 'stamp-vs') return '#version 300 es\n' + read('stamp.vert.glsl');
  throw new Error('unknown kind ' + kind);
}

/* ---------------------------------------------------------------- preprocess -- */
/* object-like #define expansion, which is all the alias table needs */
function preprocess(src) {
  const defines = [];
  const lines = src.split('\n').map((l) => {
    const m = l.match(/^\s*#define\s+([A-Za-z_][A-Za-z0-9_]*)\s+(.*)$/);
    if (m && !/^\(/.test(m[2])) { defines.push([m[1], m[2]]); return '/* define ' + m[1] + ' */'; }
    return l;
  });
  let out = lines.join('\n');
  for (let pass = 0; pass < 3; pass++) {
    for (const [name, body] of defines) {
      out = out.replace(new RegExp(`\\b${name}\\b`, 'g'), body);
    }
  }
  return out;
}
/* strip comments so the scans below cannot trip over prose */
function strip(src) {
  return src.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' ')).replace(/\/\/[^\n]*/g, '');
}

/* ------------------------------------------------------------------- parse --- */
let parser = null;
try {
  parser = (await import('@shaderfrog/glsl-parser')).parser;
} catch (e) {
  console.log('· @shaderfrog/glsl-parser not installed: skipping parse checks (npm i to enable)');
}

const BUILTIN_OK = /^(gl_|texture|textureLod|texelFetch|radians|degrees|sin|cos|tan|asin|acos|atan|pow|exp|log|exp2|log2|sqrt|inversesqrt|abs|sign|floor|ceil|fract|mod|min|max|clamp|mix|step|smoothstep|length|distance|dot|cross|normalize|faceforward|reflect|refract|transpose|inverse|determinant|matrixCompMult|outerProduct|lessThan|lessThanEqual|greaterThan|greaterThanEqual|equal|notEqual|any|all|not|isnan|isinf|floatBitsToInt|intBitsToFloat|uintBitsToFloat|floatBitsToUint|packSnorm2x16|unpackSnorm2x16|discard|dFdx|dFdy|fwidth|sign)$/;

const problems = [];
const warnings = [];

function checkSource(label, rawSrc) {
  const src = preprocess(rawSrc);
  const clean = strip(src);
  const lines = src.split('\n');

  /* --- parse --- */
  if (parser) {
    const captured = [];
    const origError = console.error;
    console.error = (...a) => captured.push(a.join(' '));
    let ast = null;
    try {
      ast = parser.parse(src);
    } catch (e) {
      problems.push(`${label}: parse error — ${String(e.message).split('\n')[0]}`);
    }
    console.error = origError;
    for (const msg of captured) {
      if (/undefined variable|undeclared function|undefined (type|function)/i.test(msg)) {
        const name = (msg.match(/"([^"]+)"/) || [])[1] || msg;
        if (BUILTIN_OK.test(String(name).split(' ')[0])) continue;
        warnings.push(`${label}: ${msg.trim()}`);
      }
    }
    void ast;
  }

  /* --- use before definition (GLSL requires declaration first) --- */
  const defRe = /^[A-Za-z_][A-Za-z0-9_\[\]]*\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(/gm;
  const definedAt = new Map();
  let m;
  while ((m = defRe.exec(clean))) {
    if (/^(if|for|while|switch|return|else)$/.test(m[1])) continue;
    if (!definedAt.has(m[1])) definedAt.set(m[1], clean.slice(0, m.index).split('\n').length);
  }
  const callRe = /\b([A-Za-z_][A-Za-z0-9_]*)\s*\(/g;
  while ((m = callRe.exec(clean))) {
    const name = m[1];
    if (BUILTIN_OK.test(name)) continue;
    if (/^(if|for|while|switch|return)$/.test(name)) continue;
    const lineNo = clean.slice(0, m.index).split('\n').length;
    const defLine = definedAt.get(name);
    if (defLine === undefined) continue;         /* handled by the parser pass */
    if (defLine > lineNo) problems.push(`${label}:${lineNo}: "${name}" is used at line ${lineNo} but defined at line ${defLine}`);
  }

  /* --- const array constructor size sanity --- */
  const arrRe = /const\s+\w+\s+(\w+)\[(\d+)\]\s*=\s*\w+\[(\d+)\]\s*\(([^;]*)\)/g;
  while ((m = arrRe.exec(clean))) {
    const declared = parseInt(m[2], 10), used = parseInt(m[3], 10);
    const items = m[4].split(',').length;
    if (declared !== used) warnings.push(`${label}: array ${m[1]} declared [${declared}] but constructed [${used}]`);
    if (items !== used) warnings.push(`${label}: array ${m[1]} constructor has ${items} items for ${used} slots`);
  }
  return src;
}

/* --------------------------------------------------------- uniform coverage --- */
const layout = layoutUniforms(GROUPS, RAMPS);
const aliasNames = new Set(layout.uniforms.map(u => u.name));
const globals = read('globals.glsl');
const lib = read('lib.glsl');
const declaredAliases = new Set([...globals.matchAll(/^#define\s+(u_[A-Za-z0-9_]+)/gm)].map(m => m[1]));
for (const u of DERIVED) declaredAliases.add(u.n);
for (const s of SAMPLERS) declaredAliases.add(s);

const files = ['lib.glsl', 'optics.glsl', 'glyphs.glsl', 'claims.glsl', 'march-sdf.glsl', 'march-height.glsl',
  'mem-update.glsl', 'mem-stamp.glsl', 'bloom.glsl', 'composite.glsl', 'common.vert.glsl', 'stamp.vert.glsl',
  ...Object.values(MODE_GLSL).map(m => m.src)];

/* Two scans, because the alias table holds both spellings: snake_case knobs and
   camelCase derived values (uRes, uTime, uCamO...). The first scan knows the
   declared names, so it can see every one of them; the second catches a knob
   that was written into a shader but never made it into the table. */
const nameRe = new RegExp('\\b(' + [...declaredAliases].join('|') + ')\\b', 'g');
const used = new Map();
for (const f of files) {
  const src = strip(read(f));
  for (const m of src.matchAll(nameRe)) {
    if (!used.has(m[1])) used.set(m[1], new Set());
    used.get(m[1]).add(f);
  }
  for (const m of src.matchAll(/\bu_[a-z][A-Za-z0-9_]*/g)) {
    if (!declaredAliases.has(m[0])) problems.push(`${f}: ${m[0]} looks like a knob but is not in globals.glsl`);
  }
}
/* A knob can legitimately leave the shader alone: the clock, the camera rig and
   the HUD maths all live on the CPU. Those count as used if the JS mentions them. */
const cpuUsed = new Set();
for (const f of readdirSync(join(root, 'js'))) {
  if (!f.endsWith('.js')) continue;
  const src = readFileSync(join(root, 'js', f), 'utf8');
  for (const m of src.matchAll(nameRe)) cpuUsed.add(m[1]);
}
for (const name of declaredAliases) {
  if (used.has(name)) continue;
  if (cpuUsed.has(name)) continue;
  warnings.push(`alias ${name} is declared, and nothing — shader or JS — reads it`);
}

/* Snake-case aliases are not the only uniforms on the page: the derived values
   are camelCase (uRes, uTime, uLC...). The parser's undefined-variable report is
   unreliable around function parameters, so scan for that shape explicitly. It
   is exactly the shape a typo takes: uLC instead of u_lc. */
const declRe = /\b(?:uniform\s+)?(?:const\s+)?(?:highp\s+|mediump\s+|lowp\s+)?(?:vec[234]|ivec[234]|uvec[234]|bvec[234]|float|int|uint|bool|mat[234]|sampler2D|samplerCube)\s+([A-Za-z_][A-Za-z0-9_]*)/g;
const declared = new Set(declaredAliases);
for (const f of files) {
  const src = strip(read(f));
  for (const m of src.matchAll(declRe)) declared.add(m[1]);
  for (const m of src.matchAll(/\b(?:float|void|vec[234]|int|bool)\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)) declared.add(m[1]);
  for (const m of src.matchAll(/\bstruct\s+([A-Za-z_][A-Za-z0-9_]*)/g)) declared.add(m[1]);
}
const BUILTIN = new Set(['gl_FragCoord', 'gl_VertexID', 'gl_Position', 'gl_PointSize', 'gl_FragDepth']);
for (const f of files) {
  const src = strip(read(f));
  for (const m of src.matchAll(/\b([ug][A-Za-z0-9_]*)\b/g)) {
    const id = m[1];
    if (id.length < 3) continue;
    if (!/^[ug][A-Z_]/.test(id)) continue;          /* uFoo, g_cone, u_x — not plain locals */
    if (declared.has(id) || BUILTIN.has(id)) continue;
    const line = src.slice(0, m.index).split('\n').length;
    problems.push(`${f}:${line}: "${id}" looks like a uniform but is declared nowhere`);
  }
}
for (const g of GROUPS) {
  if (g.u && !aliasNames.has(g.u)) problems.push(`params.js declares uniform ${g.u} which the layout does not contain`);
}

/* ---- the JS ramp mirror must keep up with the GLSL literals ---- */
{
  const body = lib.slice(lib.indexOf('vec3 ramp(int id, float t) {'));
  let compared = 0;
  for (const name of RAMPS) {
    if (name === 'spectral') continue;
    const m = body.match(new RegExp(`RAMP_${name.toUpperCase()}\\b[^;]*?ramp(?:10|6)\\(t,([^;]*)\\);`, 's'));
    if (!m) { warnings.push(`lib.glsl has no literal stop list for ramp ${name}`); continue; }
    const glsl = (m[1].match(/vec3\(([^)]*)\)/g) || []).map(v => v.slice(5, -1).split(',').map(Number));
    const js = RAMP_STOPS[name];
    if (!js) { problems.push(`js/ramps.js has no stops for ramp ${name}`); continue; }
    if (glsl.length !== js.length) { problems.push(`ramp ${name}: ${glsl.length} stops in GLSL, ${js.length} in js/ramps.js`); continue; }
    for (let i = 0; i < glsl.length; i++) {
      for (let k = 0; k < 3; k++) {
        if (Math.abs(glsl[i][k] - js[i][k]) > 1e-6) problems.push(`ramp ${name} stop ${i}[${k}]: GLSL ${glsl[i][k]} vs JS ${js[i][k]}`);
      }
    }
    compared++;
  }
  if (compared !== RAMPS.length - 1) warnings.push(`only ${compared} of ${RAMPS.length - 1} stopped ramps were comparable`);
}

/* ------------------------------------------------------------------ programs -- */
const programs = ['vs', 'stamp-vs', 'composite', 'bright', 'blur', 'mem-update', 'mem-stamp',
  ...Object.keys(MODE_GLSL).map(id => 'mode:' + id)];
const sources = new Map();
for (const p of programs) {
  const kind = p.startsWith('mode:') ? 'mode' : p;
  const modeId = p.startsWith('mode:') ? p.slice(5) : null;
  let src;
  try {
    src = assemble(kind, modeId);
  } catch (e) {
    problems.push(`${p}: assembly failed — ${e.message}`);
    continue;
  }
  sources.set(p, src);
  checkSource(p, src);
  if (src.split('\n').length > 2000) warnings.push(`${p}: ${src.split('\n').length} lines of generated shader — compile time may be noticeable`);
}
function assembleMode(id) { return assemble('mode', id); }
void assembleMode;

/* every mode must define render(vec2) returning vec4 */
for (const id of Object.keys(MODE_GLSL)) {
  const src = sources.get('mode:' + id) || '';
  if (!/vec4\s+render\s*\(\s*vec2/.test(src)) problems.push(`mode:${id}: does not define "vec4 render(vec2 fragCoord)"`);
  if (/\bvec3\s+render\s*\(\s*vec2/.test(src)) problems.push(`mode:${id}: render still returns vec3 — the auxiliary channel needs vec4`);
}

/* --------------------------------------------------------------- the report -- */
const budget = vectorCost(layout);
console.log(`uniform vectors: ${budget} of the 224 that WebGL2 guarantees (${layout.uniforms.length} knobs in ${layout.floats.length} vec4 + ${layout.ints.length} ivec4 + ${layout.cols.length} vec3)`);
console.log(`programs assembled: ${programs.length} · total generated lines: ${[...sources.values()].reduce((a, s) => a + s.split('\n').length, 0)}`);
if (warnings.length) {
  console.log(`\n${warnings.length} warning(s):`);
  for (const w of warnings.slice(0, 40)) console.log('  · ' + w);
  if (warnings.length > 40) console.log(`  … and ${warnings.length - 40} more`);
}
if (problems.length) {
  console.log(`\n${problems.length} problem(s):`);
  for (const p of problems) console.log('  ✗ ' + p);
  process.exitCode = 1;
} else {
  console.log('\nno problems found.');
}
