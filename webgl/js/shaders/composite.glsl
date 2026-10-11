/* =============================================================================
   composite.glsl — the last pass: two renderings, one screen.

   Everything about output lives here, which is why the page can offer an
   unreasonable number of output knobs without any of them costing the modes
   anything: split comparison and anaglyph, palettes driven by a physical
   quantity, the 2020–2026 evidence mosaic, a character pass that reduces the
   image to the terminal this project came from, grade, tone curve, tube,
   dither, grain and the tracing overlays.
   ============================================================================= */

uniform sampler2D uSceneA;
uniform sampler2D uSceneB;
uniform sampler2D uBloom;

vec3 fetchA(vec2 uv) { return texture(uSceneA, uv).rgb; }
vec3 fetchB(vec2 uv) { return texture(uSceneB, uv).rgb; }

/* ------------------------------------------------------------- the mosaic -- */
/* Fifteen claims, drawn as fifteen cells. Colour is the domain, the bar is the
   years it covers, and the fill says how well established it is. A claim that
   is one study old looks one study old. */
vec3 evidenceMosaic(vec2 uv, vec3 base) {
  float cols = max(2.0, round(u_mosaicCells));
  float rows = max(1.0, round(cols * 9.0 / 16.0));
  vec2 g = uv * vec2(cols, rows);
  vec2 ci = floor(g);
  vec2 cf = fract(g);
  float col = ci.x, row = ci.y;
  int idx = int(mod(col + row * cols, float(CLAIM_N)));
  float gap = clamp(u_mosaicGap, 0.0, 0.45);
  float inside = step(gap, cf.x) * step(cf.x, 1.0 - gap) * step(gap, cf.y) * step(cf.y, 1.0 - gap);

  float cat = float(CLAIM_CAT[idx]) / 5.0;
  vec3 c = ramp(clamp(int(u_mosaicRamp), 0, 15), cat);
  int conf = CLAIM_CONF[idx];
  /* confidence decides the fill: solid, hatched, or nearly open */
  float hatch = 0.5 + 0.5 * sin((cf.x + cf.y) * 26.0);
  float fill = (conf == 2) ? 0.85 : ((conf == 1) ? 0.45 + 0.25 * hatch : 0.10 + 0.42 * hatch);
  /* the year band slides down the cell as the claim’s window moves later */
  float mid = (float(CLAIM_Y0[idx]) + float(CLAIM_Y1[idx])) * 0.5;
  float f = clamp((mid - (u_epoch - u_span * 0.5)) / max(u_span, 0.001), 0.0, 1.0);
  float band = smoothstep(0.035, 0.0, abs(cf.y - (0.08 + 0.84 * f)));
  float spread = clamp(float(CLAIM_Y1[idx] - CLAIM_Y0[idx]) / max(u_span, 0.001), 0.0, 1.0);
  vec3 cell = c * fill * inside + c * band * inside * (0.6 + 1.4 * u_mosaicBand);
  cell += c * inside * spread * 0.07;
  /* hairlines */
  float line = (step(cf.x, gap * 0.35) + step(1.0 - gap * 0.35, cf.x) +
                step(cf.y, gap * 0.35) + step(1.0 - gap * 0.35, cf.y));
  cell = max(cell, vec3(0.35) * clamp(line, 0.0, 1.0) * 0.5);
  return mix(base, base * 0.25 + cell * 1.15, clamp(u_mosaic, 0.0, 1.0));
}

/* --------------------------------------------------------------- the glyph -- */
vec3 glyphPass(vec2 fragCoord, vec3 base, sampler2D src) {
  float cell = max(u_glyphScale, 3.0);
  vec2 gc = fragCoord / cell;
  vec2 ci = floor(gc);
  vec2 cf = fract(gc);
  vec3 c = texture(src, (ci + 0.5) * cell / uRes.xy).rgb;
  float l = clamp(luma(c), 0.0, 1.0);
  float ink = 0.0;
  if (u_glyph == 1) {                       /* blocks */
    ink = step(0.12, cf.x) * step(cf.x, 0.88) * step(0.12, cf.y) * step(cf.y, 0.88);
    l = smoothstep(0.0, 1.0, l);
  } else if (u_glyph == 2) {                /* a brightness ramp of characters */
    int g = int(clamp(floor(l * 15.99), 0.0, 15.0));
    vec2 q = (cf - 0.5) * 0.86 + 0.5;
    ink = glyphInk(g, vec2(q.x, 1.0 - q.y));
  } else if (u_glyph == 3 || u_glyph == 4) { /* halftone dots, straight or rotated */
    vec2 q = cf;
    if (u_glyph == 4) {
      float a = 0.785398;
      q = mat2(cos(a), -sin(a), sin(a), cos(a)) * (cf - 0.5) + 0.5;
    }
    float r = length(q - 0.5);
    float rad = 0.16 + 0.34 * sqrt(l);
    ink = 1.0 - smoothstep(rad - 0.05, rad + 0.05, r);
  } else {                                  /* hexadecimal readout of the value */
    int g = int(clamp(floor(l * 15.99), 0.0, 15.0));
    vec2 q = (cf - 0.5) * 0.82 + 0.5;
    ink = glyphInkHex(g, vec2(q.x, 1.0 - q.y));
  }
  vec3 inkCol = mix(c, vec3(luma(c)) * 1.2, 0.2);
  vec3 outCol = mix(base * 0.10, inkCol, clamp(ink, 0.0, 1.0));
  return mix(base, outCol, clamp(u_glyphMix, 0.0, 1.0));
}

/* ------------------------------------------------------------- the overlays -- */
vec3 overlays(vec2 fragCoord, vec3 col) {
  vec2 uv = fragCoord / uRes.xy;
  float px = 1.0;
  /* tracing guide: an inset frame, thirds, and scale ticks */
  if (u_stroke > 0) {
    vec2 m = min(uv, 1.0 - uv) * uRes.xy;
    float frame = smoothstep(24.0, 22.0, min(m.x, m.y));
    col += vec3(0.30, 0.36, 0.44) * frame * 0.35;
    float thirds = smoothstep(0.0016, 0.0, abs(uv.x - 0.3333)) + smoothstep(0.0016, 0.0, abs(uv.x - 0.6667))
                 + smoothstep(0.0016, 0.0, abs(uv.y - 0.3333)) + smoothstep(0.0016, 0.0, abs(uv.y - 0.6667));
    col += vec3(0.16, 0.20, 0.26) * clamp(thirds, 0.0, 1.0) * 0.5;
    /* ticks along the bottom edge, one per unit of whatever the mode measures */
    float tick = smoothstep(0.5, 0.0, abs(fract(uv.x * 32.0) - 0.5));
    col += vec3(0.5) * tick * smoothstep(0.02, 0.012, uv.y) * 0.5;
  }
  /* annotations: a reticle with radius marks, the way a conoscope has */
  if (u_annot > 0) {
    vec2 q = (fragCoord - 0.5 * uRes.xy) / uRes.y;
    float r = length(q);
    float cross_ = smoothstep(0.0016, 0.0, abs(q.x)) + smoothstep(0.0016, 0.0, abs(q.y));
    float ring = smoothstep(0.0016, 0.0, abs(r - 0.42)) + smoothstep(0.0012, 0.0, abs(r - 0.21)) * 0.6;
    col += vec3(0.42, 0.55, 0.7) * clamp(cross_ * 0.35 + ring * 0.5, 0.0, 1.0);
  }
  /* mirror assist */
  if (u_symmetry > 0) {
    float d = abs(uv.x - 0.5);
    col += vec3(0.8, 0.4, 0.9) * smoothstep(0.0015, 0.0, d) * 0.6;
    col = mix(col, col.brg * 0.5 + col * 0.5, smoothstep(0.0, 0.5, d) * 0.12);
  }
  return col;
}

vec3 compositeMain() {
  vec2 uv = gl_FragCoord.xy / uRes.xy;
  vec2 px = gl_FragCoord.xy;

  /* ---- which rendering, and how the two are mixed ---- */
  vec3 a = fetchA(uv);
  vec3 b = fetchB(uv);
  vec4 da = texture(uSceneA, uv);
  vec4 db = texture(uSceneB, uv);
  float aux = da.w;
  vec3 c = a;
  int sm = u_splitMode;
  if (sm > 0 && u_slotB > 0) {
    if (sm == 5) {
      /* anaglyph: one polarisation channel per eye */
      float la = luma(a), lb = luma(b);
      c = vec3(la * 1.15, lb, lb) * 1.0;
    } else {
      bool useB = false;
      if (sm == 1) useB = uv.x > u_split;
      else if (sm == 2) useB = uv.y > u_split;
      else if (sm == 3) useB = (uv.x + uv.y) * 0.5 > u_split;
      else if (sm == 4) { float k = floor(px.x / 16.0) + floor(px.y / 16.0); useB = fract(k * 0.5) > 0.25; }
      if (useB) { c = b; aux = db.w; }
      /* the seam, so you always know you are looking at two things */
      float seam = 0.0;
      if (sm == 1) seam = smoothstep(u_splitGap, 0.0, abs(uv.x - u_split));
      else if (sm == 2) seam = smoothstep(u_splitGap, 0.0, abs(uv.y - u_split));
      else if (sm == 3) seam = smoothstep(u_splitGap * 1.4, 0.0, abs((uv.x + uv.y) * 0.5 - u_split));
      c += vec3(0.6, 0.85, 1.0) * seam * 0.5;
    }
  }

  /* ---- palette driven by a physical quantity ---- */
  if (u_palette > 0) {
    float src = 0.0;
    if (u_palSource == 1) src = clamp(aux, 0.0, 1.0);
    else if (u_palSource == 2) {
      float gx = luma(fetchA(uv + vec2(1.0 / uRes.x, 0.0)) - a);
      float gy = luma(fetchA(uv + vec2(0.0, 1.0 / uRes.y)) - a);
      src = clamp(length(vec2(gx, gy)) * 6.0, 0.0, 1.0);
    } else if (u_palSource == 3) {
      float mx = max(max(c.r, c.g), c.b);
      float mn = min(min(c.r, c.g), c.b);
      src = clamp(mx - mn, 0.0, 1.0);
    } else if (u_palSource == 4) src = fract(aux * u_palCycles + u_palOffset);
    else if (u_palSource == 5) {
      float ex = abs(aux - texture(uSceneA, uv + vec2(2.0 / uRes.x, 0.0)).w);
      float ey = abs(aux - texture(uSceneA, uv + vec2(0.0, 2.0 / uRes.y)).w);
      src = clamp((ex + ey) * 24.0, 0.0, 1.0);
    } else src = clamp(luma(c), 0.0, 1.0);
    float t = (u_palSource == 4) ? src : fract(src * u_palCycles + u_palOffset);
    vec3 pc = ramp(clamp(int(u_palette) - 1, 0, 15), t) * (0.35 + 0.85 * luma(c));
    c = mix(c, pc, clamp(u_palMix, 0.0, 1.0));
  }

  c = evidenceMosaic(uv, c);

  /* ---- bloom, then the tone curve ---- */
  c += texture(uBloom, uv).rgb * u_bloom;

  /* ---- grade in linear space ---- */
  c *= max(u_exposure, 0.0);
  c = max(c, 0.0);
  c += vec3(u_lift);
  /* temperature and tint move the white point */
  c *= vec3(1.0 + u_temp * 0.22, 1.0 + u_tint * 0.08, 1.0 - u_temp * 0.22);
  vec3 tone = tonemap(u_tonemap, c);
  vec3 ldr = pow(clamp(tone, 0.0, 1.0), vec3(1.0 / max(u_gamma, 0.05)));
  ldr = satAdjust(ldr, u_sat);
  ldr = contrastAdjust(ldr, u_contrast);

  /* ---- the glyph pass works on the tone-mapped image ---- */
  if (u_glyph > 0) ldr = glyphPass(px, ldr, uSceneA);

  /* ---- tube, vignette, scanlines, grain, dither ---- */
  if (u_crt > 0.001) {
    vec2 q = uv * 2.0 - 1.0;
    float r2 = dot(q, q);
    q *= 1.0 + u_crt * 0.22 * r2;
    ldr *= smoothstep(0.02, 0.35, 1.0 - r2 * (0.35 + 0.5 * u_crt));
  } else {
    float r2v = dot(uv * 2.0 - 1.0, uv * 2.0 - 1.0) * 0.5;
    ldr *= mix(1.0, smoothstep(1.25, 0.15, r2v), clamp(u_vignette, 0.0, 1.5) * 0.9);
  }
  float scan = 1.0 - u_scan * 0.5 * (0.5 + 0.5 * sin(px.y * 3.14159));
  ldr *= scan;
  float g2 = hash12(px + vec2(uTime * 37.0, uFrame * 3.1)) - 0.5;
  ldr += g2 * u_grain * 0.5;

  ldr = overlays(px, ldr);
  return ldr;
}
