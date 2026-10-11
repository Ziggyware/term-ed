/* =============================================================================
   mode-hologram.glsl — phase, and what phase becomes.

   The object being recorded here is the substrate from the Substrata mode: the
   same fractal ground, the same stored traces, now treated as a phase object on
   a reflective plate. The recorded pattern is |E_ref + E_obj|² — that part is
   exact, and it is all a hologram is. The reconstruction is a Fresnel
   propagation sampled with a Gaussian-weighted chirp kernel (approximate, and
   labelled as such): twelve taps instead of a Fourier transform.
   ============================================================================= */

float objectHeight(vec2 p) {
  float h = fbm(p * u_vScale * 0.6 + vec2(0.3, 1.7), int(u_vOct), u_vGain, u_vLac, u_vRidge);
  h = h * 0.5 + 0.5;
  float lam = h * float(u_terraces) + p.x * u_laminaTilt;
  h = mix(h, (floor(lam) + smoothstep(0.3, 0.72, fract(lam))) / max(float(u_terraces), 1.0), 0.4);
  if (u_memoryOn > 0) {
    Mem m = memSample(p * 0.1667 * u_fieldScale + 0.5);
    h += m.h * u_imprint * 0.5 * (1.0 - u_decay * 0.5);
  }
  /* sparse dielectric specks: roughness is what makes the speckle */
  h += fbm(p * 90.0, 3, 0.5, 2.2, 1.0) * 0.01 * u_speckle;
  return (h - 0.5) * u_fieldScale;
}
vec2 objectField(vec2 p, float phScale) {
  float h = objectHeight(p);
  float amp = 0.32 + 0.68 * smoothstep(-0.6, 0.6, h);   /* how reflective the ground is */
  return amp * cexp2(h * TAU * phScale);
}
/* sinc(x) with the π folded in, for aperture envelopes */
float sincPi(float x) {
  float p = PI * x;
  return (abs(x) < 1e-4) ? 1.0 : sin(p) / p;
}

/* Fresnel propagation, sampled: twelve chirp taps under a Gaussian envelope
   whose width grows as √(λz). Not a Fourier transform, but it diffracts. */
vec2 propagate(vec2 p, float z, float lamMm, float taps) {
  float zz = max(z, 1e-3);
  float sig = sqrt(max(lamMm * zz, 1e-5)) * 3.2;
  vec2 acc = vec2(0.0);
  float wsum = 0.0;
  for (int i = 0; i < 16; i++) {
    if (float(i) >= taps) break;
    vec2 off = diskSample(i, int(taps)) * sig * (1.0 + 0.6 * float(i) / taps);
    float ch = PI * dot(off, off) / max(lamMm * zz, 1e-5);
    vec2 e = objectField(p + off, u_phaseScale);
    float w = exp(-dot(off, off) / max(2.0 * sig * sig, 1e-6));
    acc += cmul(e, cexp2(ch)) * w;
    wsum += w;
  }
  return acc / max(wsum, EPS);
}

/* a Fresnel zone plate: opaque where the path difference is an odd number of
   half-wavelengths. Spacing goes as √(nλz) — the reason it focuses at all. */
float zonePlate(vec2 p, float lamMm, float f) {
  float r2 = dot(p, p);
  float n = r2 / max(lamMm * f, 1e-6);
  return 0.5 + 0.5 * cos(PI * n);
}

/* Bessel J0, for the time-averaged interferogram: vibration fringes fade where
   the amplitude of the motion crosses the zeros of J0. */
float besselJ0(float x) {
  float ax = abs(x);
  if (ax < 8.0) {
    float y = x * x;
    float p1 = 57568490574.0 + y * (-13362590354.0 + y * (651619640.7 + y * (-11214424.18 + y * (77392.33017 + y * (-184.9052456)))));
    float p2 = 57568490411.0 + y * (1029532985.0 + y * (9494680.718 + y * (59272.64853 + y * (267.8532712 + y))));
    return p1 / p2;
  }
  float z = 8.0 / ax;
  float y = z * z;
  float xx = ax - 0.785398164;
  float p1 = 1.0 + y * (-0.1098628627e-2 + y * (0.2734510407e-4 + y * (-0.2073370639e-5 + y * 0.2093887211e-6)));
  float p2 = -0.1562499995e-1 + y * (0.1430488765e-3 + y * (-0.6911147651e-5 + y * (0.7621095161e-6 + y * -0.934935152e-7)));
  return sqrt(0.636619772 / ax) * (cos(xx) * p1 - z * sin(xx) * p2);
}

/* ------------------------------------------------------- the panel surface --- */
/* Returns  (phase, amplitude) of the wave leaving the plate at that point */
vec2 plateField(vec2 mm, float lamMm) {
  vec2 f = vec2(0.0);
  int v = u_field;
  if (v == 0) {
    /* inline hologram of a point at depth u_depth: the fringes are the
       interference of a reference wave with a spherical one */
    float z = max(u_depth * 0.35, 0.02);
    float r2 = dot(mm, mm);
    float sph = PI * r2 / max(lamMm * z, 1e-6);
    vec2 eObj = cexp2(sph) * (1.0 / (1.0 + r2 * 8.0));
    vec2 eRef = cexp2(dot(mm, vec2(cos(u_fAngle), sin(u_fAngle))) * TAU * u_carrier / max(lamMm, 1e-6));
    vec2 sum = eRef + eObj * u_recon;
    float I = dot(sum, sum) * 0.25;
    f = vec2(I, I);
  } else if (v == 1) {
    float zp = zonePlate(mm, lamMm, max(u_depth * 0.4, 0.05));
    f = vec2(mix(zp, zp * zp, 0.35), zp);
  } else if (v == 2) {
    /* off-axis: several object points, each with its own carrier, so the
       reconstructions separate into orders. The twin image is the price. */
    vec2 acc = vec2(0.0);
    for (int i = 0; i < 12; i++) {
      if (i >= u_branches) break;
      float fi = float(i) + 0.5;
      float a = fi * 2.39996323 + u_fAngle * 0.017;
      vec2 c = vec2(cos(a), sin(a)) * u_fieldScale * 0.16 * sqrt(fi / max(float(u_branches), 1.0));
      float z = max(u_depth * (0.25 + 0.5 * hash11(fi)), 0.02);
      float sph = PI * dot(mm - c, mm - c) / max(lamMm * z, 1e-6);
      float amp = 1.0 / (1.0 + 40.0 * dot(mm - c, mm - c));
      float car = TAU * dot(mm, vec2(cos(radians(u_branchAng)), sin(radians(u_branchAng)))) * u_carrier / max(lamMm, 1e-6);
      vec2 eObj = cexp2(sph);
      vec2 eRef = cexp2(car);
      acc += (eRef + eObj * amp) * 0.5;
    }
    f = vec2(dot(acc, acc) * 0.3, length(acc));
  } else if (v == 3) {
    /* the far field of the panel's own pixel grid: orders at sinθ = mλ/a, each
       carrying the RGB stripe envelope. This is the phone-screen rainbow. */
    float a = max(u_pixelPitch * 1e-3, 1e-4);           /* mm */
    vec2 acc = vec2(0.0);
    for (int my = -6; my <= 6; my++) {
      for (int mx = -6; mx <= 6; mx++) {
        float m = float(mx), n = float(my);
        if (abs(m) + abs(n) > 7.0) continue;
        float env = exp(-(m * m + n * n) * 0.16) * sincPi(m * 0.42) * sincPi(n * 0.42)
                  * mix(1.0, 0.55 + 0.45 * cos(float(mx) * 2.1), u_subpixel);
        float kx = TAU * m / a, ky = TAU * n / a;
        acc += cexp2(kx * mm.x + ky * mm.y) * env;
      }
    }
    float I = dot(acc, acc) * 0.02;
    f = vec2(I, I);
  } else if (v == 4) {
    /* Chladni: what sand does on a plate driven at one frequency. The grains
       walk until they find the nodal lines and then they stop, which is
       memory in a granular medium — the one place sand keeps a record. */
    float nm = u_fCym;
    float kx = PI * (floor(nm) + 1.0) / max(u_plateW, 0.1);
    float ky = PI * (fract(nm) * 4.0 + 1.0) / max(u_plateW, 0.1);
    float w = sin(kx * mm.x) * sin(ky * mm.y) + 0.62 * sin(ky * mm.x) * sin(kx * mm.y);
    f = vec2(abs(w), abs(w));
  } else if (v == 5) {
    /* wrapped phase: an atan2 of the wavefront, with its discontinuity lines */
    vec2 e = objectField(mm, u_phaseScale * 0.08);
    float ph = atan(e.y, e.x);
    float wrapped = abs(fract(ph / TAU) - 0.5);
    f = vec2(ph * 0.2, smoothstep(0.5, 0.42, wrapped));
  } else if (v == 6) {
    /* shearing interferogram: interfere the wavefront with a copy of itself
       displaced sideways; every fringe is a contour of the gradient */
    vec2 s = vec2(cos(u_fAngle), sin(u_fAngle)) * u_phaseScale * 0.004;
    vec2 e1 = objectField(mm, u_phaseScale * 0.35);
    vec2 e2 = objectField(mm + s, u_phaseScale * 0.35);
    vec2 sum = e1 + e2;
    f = vec2(dot(sum, sum) * 0.25, length(sum) * 0.5);
  } else if (v == 7) {
    /* moiré: two gratings, the beat between them, and the aliasing that a
       sensor's own pitch adds on top */
    float g1 = 0.5 + 0.5 * cos(TAU * mm.x / max(u_fieldScale * 0.04, 1e-4));
    float ang = u_fAngle * 0.017;
    vec2 r = vec2(mm.x * cos(ang) - mm.y * sin(ang), mm.x * sin(ang) + mm.y * cos(ang));
    float g2 = 0.5 + 0.5 * cos(TAU * r.x / max(u_fieldScale * 0.04, 1e-4) * (1.0 + u_carrier * 0.01));
    f = vec2(g1 * g2 * 2.0, abs(g1 - g2));
  } else {
    /* time-averaged: the object is vibrating, so the fringes wash out wherever
       the amplitude crosses a zero of J0 — a standing wave made visible */
    float amp = max(u_depth * 0.2, 0.0);
    float k = TAU / max(lamMm, 1e-6);
    float vib = sin(PI * mm.x / max(u_plateW, 0.1)) * sin(PI * mm.y / max(u_plateW, 0.1));
    float J = besselJ0(k * amp * vib * 0.02);
    f = vec2(J * J, abs(J));
  }
  return f;
}

/* ------------------------------------------------------------------ render -- */
vec4 render(vec2 fragCoord) {
  vec2 uv = (fragCoord - 0.5 * uRes.xy) / uRes.y;
  float lamMm = u_lambda * 1e-6;
  /* a little keystone, so the plate reads as an object in a room rather than
     as a fullscreen texture */
  float k = sin(radians(u_elev) * 0.35) * 0.25;
  vec2 pv = uv * (1.0 + k * uv.y * 2.2);
  vec2 mm = pv * u_plateW;

  vec2 pf = plateField(mm, lamMm);
  float recorded = pf.x;

  float z = max(u_depth * 0.35, 1e-3);
  int taps = clamp(int(u_tdm) * 3 + 6, 4, 16);
  vec2 rc = propagate(mm, z, lamMm, float(taps));
  float reconI = dot(rc, rc) / max(0.05, dot(propagate(mm, 1e-4, lamMm, 3.0), propagate(mm, 1e-4, lamMm, 3.0)) + 0.05);

  float recMix = clamp(u_recon, 0.0, 1.0);
  float I = mix(recorded, reconI, recMix * 0.85);
  /* speckle: coherent light off a rough surface, unavoidable and beautiful */
  float sp = 1.0 + u_speckle * 0.55 * (gnoise(mm * 90.0 + uTime * 0.7) * 2.0 - 0.5)
                                    * (gnoise(mm * 37.0 - uTime * 0.4) * 2.0 - 0.5) * 2.0;
  I *= max(sp, 0.0);

  vec3 tint = normalize(bandRGB(u_lambda) + vec3(0.02));
  vec3 col = tint * I * (1.25 + 0.5 * u_fringe);

  /* the panel itself: bezel, glass, and the pixel grid when it is resolved */
  float w = u_plateW;
  vec2 q = abs(mm);
  float frame = max(q.x / w, q.y / (w * 0.62));
  float bezel = smoothstep(1.10, 1.075, frame) * (1.0 - smoothstep(1.03, 1.0, frame));
  col = mix(col, vec3(0.06, 0.07, 0.09), smoothstep(1.075, 1.06, frame) * smoothstep(1.0, 1.03, frame));
  col += vec3(0.22, 0.25, 0.3) * bezel * 0.5;

  float pxMm = max(u_pixelPitch, 0.05) * 1e-3;
  float mmPerPx = max(length(vec2(dFdx(mm.x), dFdy(mm.x))), 1e-9);
  float pxPerPixel = pxMm / mmPerPx;          /* screen pixels per panel pixel */
  if (pxPerPixel > 1.6) {
    vec2 g = fract(mm / pxMm);
    vec3 sub = vec3(
      smoothstep(0.42, 0.58, g.x) * smoothstep(0.94, 1.0, g.x + 0.06),
      smoothstep(0.1, 0.4, fract(g.x * 3.0)) * 0.8,
      smoothstep(0.1, 0.4, fract(g.x * 3.0 + 0.66)) * 0.8);
    vec3 mask = mix(vec3(1.0), vec3(0.9, 0.95, 1.0) + sub * 0.5, clamp(u_subpixel, 0.0, 1.0));
    float grid = smoothstep(0.06, 0.0, min(fract(g.x), 1.0 - fract(g.x))) +
                 smoothstep(0.06, 0.0, min(fract(g.y), 1.0 - fract(g.y)));
    col *= mix(vec3(1.0), mask, clamp(u_subpixel, 0.0, 1.0) * 0.6);
    col *= 1.0 - 0.25 * clamp(grid, 0.0, 1.0);
  }
  /* the frame rate is a real constraint: a panel updates, it does not exist */
  float sweep = fract(uTime * u_frameRate / 60.0);
  col *= 1.0 - 0.05 * smoothstep(0.999, 1.0, sweep);
  /* aux: where the object's phase sits in its own 2π */
  vec2 oe = objectField(mm, u_phaseScale);
  return vec4(col, fract(atan(oe.y, oe.x) * 0.15915494 + 0.5));
}
