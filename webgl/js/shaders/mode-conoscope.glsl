/* =============================================================================
   mode-conoscope.glsl — the polarising microscope, pupil by pupil.

   Every pixel is one ray leaving the condenser, crossing the cell at its own
   angle, and arriving at the eyepiece. The cell is divided into slices and each
   slice is a tilted uniaxial slab, so the state of polarisation is integrated
   through the depth exactly as a Jones-matrix calculation would.

   Nothing here is drawn for effect. The Maltese cross, the concentric orders,
   the coloured rings at high NA and the black field at extinction all fall out
   of the algebra; if you turn the polarisers parallel, or add quarter-wave
   plates, the figure changes the way the textbooks say it will.
   ============================================================================= */

/* Stokes parameters of a Jones vector: a full description of the polarisation
   state, which is what the microscope can actually report. */
vec4 stokes(vec2 Ex, vec2 Ey) {
  float ax = dot(Ex, Ex), ay = dot(Ey, Ey);
  vec2 cc = vec2(Ex.x * Ey.x + Ex.y * Ey.y, Ex.y * Ey.x - Ex.x * Ey.y);
  return vec4(ax + ay, ax - ay, 2.0 * cc.x, 2.0 * cc.y);
}

/* the director through the thickness, exactly as in the director mode but in
   the optics frame (z = layer normal) */
vec3 cscopeDirector(float s, vec2 xy, float volt) {
  float vTilt = freedericksz(volt) * u_voltMod;
  float pre = radians(u_prt) + radians(u_lcTilt);
  float tilt = lcTiltProfile(s, pre + vTilt * 1.35);
  float tw = lcTwistProfile(s, radians(u_lcTwist));
  if (u_lcTwistProf == 3) tw *= 1.0;
  if (u_lcPitch > 0.001) tw += TAU * (u_thick / max(u_lcPitch, 0.001)) * s;
  tw *= mix(1.0, 0.25, clamp(freedericksz(volt) * u_voltMod, 0.0, 1.0));
  /* lateral patterning and the field pattern near a defect */
  if (u_lcVoltProf == 2) tw += step(0.0, sin(xy.x * 9.0) * sin(xy.y * 9.0)) * 1.1;
  if (u_lcVoltProf == 3) tw += atan(xy.y, xy.x) * 1.4;
  if (u_lcVoltProf == 5) tw += length(xy) * 2.0;
  /* thermal fluctuation: S < 1 means the rods never quite agree */
  float fluct = (1.0 - clamp(u_lcOrder, 0.0, 1.0)) * 0.25;
  tw += (gnoise(xy * 3.0 + s * 5.0) + gnoise(xy * 3.0 + 17.0)) * fluct;
  return lcDirector(tilt, tw);
}

/* the whole cell as a Jones matrix for one pupil ray, plus the diagnostics the
   other display modes need.  `lamNm` is the band, `dz` the slice thickness. */
J cscopeCell(vec3 d, float lamNm, int slices, float volt, vec2 xy, out float retardNm, out float depthAcc) {
  J acc = jIdentity();
  float dz = u_thick / float(max(slices, 1));
  retardNm = 0.0;
  depthAcc = 0.0;
  for (int i = 0; i < 96; i++) {
    if (i >= slices) break;
    float s = (float(i) + 0.5) / float(slices);
    vec3 u = cscopeDirector(s, xy, volt);
    vec4 sr = slabRetardance(u, d, dz, lamNm * 1e-3);
    float delta = TAU * sr.x / lamNm;
    acc = jmul(jSlab(delta, sr.yz), acc);
    /* the retardance the eye sees is the algebraic sum along the path */
    retardNm += sr.x;
    depthAcc = s;
  }
  return acc;
}

/* accumulated state up to depth s, for the Stokes map */
J cscopePartial(vec3 d, float lamNm, int slices, float sMax, float volt, vec2 xy, out float retardNm) {
  J acc = jIdentity();
  float dz = u_thick / float(max(slices, 1));
  retardNm = 0.0;
  for (int i = 0; i < 96; i++) {
    if (i >= slices) break;
    float s = (float(i) + 0.5) / float(slices);
    if (s > sMax) break;
    vec3 u = cscopeDirector(s, xy, volt);
    vec4 sr = slabRetardance(u, d, dz, lamNm * 1e-3);
    acc = jmul(jSlab(TAU * sr.x / lamNm, sr.yz), acc);
    retardNm += sr.x;
  }
  return acc;
}

/* pupil optics: aperture stop, central obscuration, apodisation */
float pupilWeight(vec2 pq) {
  float r = length(pq);
  float w = smoothstep(1.0, 0.985, r);
  float r2 = max(u_obscure, 0.0);
  w *= smoothstep(r2 - 0.01, r2 + 0.02, r);
  w *= mix(1.0, exp(-r * r * 2.2), clamp(u_apod, 0.0, 1.0));
  return w;
}

vec4 render(vec2 fragCoord) {
  vec2 uv = (fragCoord - 0.5 * uRes.xy) / uRes.y;
  int slices = clamp(u_lcSlices, 1, 96);
  int bands = bandCount();
  int scope = u_cScope;

  /* ---- 3. transmission against voltage: the electro-optic curve ---------- */
  if (scope == 3) {
    /* x axis: 0 … 2.5·Vth, y axis: transmission through the crossed polars */
    float v = clamp((uv.x * 1.15 + 0.5) * 2.6, 0.0, 2.6);
    int slicesC = min(slices, 32);
    float I = 0.0, wsum = 0.0;
    for (int b = 0; b < 16; b++) {
      if (b >= bands) break;
      float lam = bandLambda(b, bands);
      float w = bandWeight(b, bands) * coherenceEnv(u_thick * 1e3);
      float rn, da;
      J jj = cscopeCell(normalize(vec3(0.0, 0.0, -1.0)), lam, slicesC, v, vec2(0.0), rn, da);
      I += w * polarScope(jj, lam) * luma(bandRGB(lam));
      wsum += w;
    }
    I /= max(wsum, EPS);
    float curve = smoothstep(0.012, 0.0, abs(uv.y * 1.15 + 0.5 - I * 0.9));
    float line = smoothstep(0.0025, 0.0, abs(uv.x * 1.15 + 0.5 - u_volt / 2.6));
    vec3 col = vec3(0.02, 0.03, 0.045);
    col += vec3(0.15, 0.8, 1.0) * curve * 1.2;
    col += vec3(1.0, 0.75, 0.25) * line * 0.9;
    /* grid */
    float g = smoothstep(0.0015, 0.0, abs(fract(uv.x * 11.5 + 0.5) - 0.5) / 11.5) * 0.25;
    col += g * vec3(0.5);
    return vec4(col, clamp(uv.y * 1.15 + 0.5, 0.0, 1.0));
  }

  /* ---- pupil ray --------------------------------------------------------- */
  vec2 pq = uv / max(u_aperture, 0.02);
  float rho = length(pq);
  float sinT = clamp(rho * max(u_na, 0.02), 0.0, 0.999);
  float cosT = sqrt(1.0 - sinT * sinT);
  vec2 dir2 = (rho > 1e-5) ? pq / rho : vec2(0.0);
  vec3 d = vec3(dir2.x * sinT, dir2.y * sinT, -cosT);
  float pw = pupilWeight(pq);

  /* ---- 4. Stokes: the polarisation state as it builds through the depth --- */
  if (scope == 4) {
    float sMax = clamp(uv.x * 1.15 + 0.5, 0.0, 1.0);
    float v = clamp((uv.y * 1.15 + 0.5) * 2.4, 0.0, 2.4);
    float rn;
    J jj = cscopePartial(vec3(0.0, 0.0, -1.0), u_lambda, slices, sMax, v, vec2(0.0), rn);
    float P = radians(u_polP);
    vec2 eP = vec2(cos(P), sin(P));
    vec2 Ex = cadd(cmul(jj.a, vec2(eP.x, 0.0)), cmul(jj.b, vec2(eP.y, 0.0)));
    vec2 Ey = cadd(cmul(jj.c, vec2(eP.x, 0.0)), cmul(jj.d, vec2(eP.y, 0.0)));
    vec4 S = stokes(Ex, Ey);
    float dop = (S.x > 1e-5) ? length(S.yzw) / S.x : 0.0;
    float az = 0.5 * atan(S.w, S.z);
    float ell = 0.5 * asin(clamp(S.w / max(S.x, 1e-5), -1.0, 1.0));
    vec3 col = hsv2rgb(vec3(fract(az / PI), clamp(dop, 0.0, 1.0), 0.35 + 0.65 * abs(sin(2.0 * ell))));
    col *= 0.25 + 0.75 * clamp(S.x / max(u_thick * 0.22, 0.05), 0.0, 1.0);
    return vec4(col * pw, clamp(rn / max(u_thick * 320.0, 1.0), 0.0, 1.0));
  }

  /* ---- 0,1,2. the pupil figure ------------------------------------------ */
  vec3 acc = vec3(0.0);
  float wsum = 0.0;
  float retard0 = 0.0, retTotal = 0.0;
  float azim = 0.0;
  for (int b = 0; b < 16; b++) {
    if (b >= bands) break;
    float lam = bandLambda(b, bands);
    float w = bandWeight(b, bands);
    float rn, da;
    J jj = cscopeCell(d, lam, slices, u_volt, uv, rn, da);
    float I = polarScope(jj, lam);
    I *= coherenceEnv(rn);
    if (b == bands / 2) { retard0 = rn; retTotal = rn; }
    /* the effective slow axis from a single-slice probe: cheap and honest */
    vec3 ud = cscopeDirector(0.5, uv, u_volt);
    vec4 probe = slabRetardance(ud, d, 1.0, lam * 1e-3);
    azim = atan(probe.w, probe.z);
    acc += w * I * bandRGB(lam);
    wsum += w;
  }
  acc /= max(wsum, EPS);

  vec3 col;
  if (scope == 1) {
    /* retardance as a map, in nanometres */
    float t = clamp(retTotal / max(u_thick * 320.0, 1.0), 0.0, 1.0);
    col = ramp(RAMP_NICHE, fract(t * 2.0)) * (0.25 + 0.9 * pw);
  } else if (scope == 2) {
    /* the slow axis direction: this is what makes the cross a cross */
    float mag = clamp(abs(retTotal) / max(u_thick * 160.0, 1.0), 0.0, 1.0);
    col = hsv2rgb(vec3(fract(azim / PI + 0.5), 0.75, 0.25 + 0.75 * mag)) * (0.2 + 0.95 * pw);
  } else {
    col = acc * (0.15 + 0.95 * pw) * u_fringe * 2.4;
    /* the aperture rim, so the instrument is visible in its own picture */
    col += vec3(0.35, 0.45, 0.6) * smoothstep(0.004, 0.0, abs(length(pq) - 1.0)) * 0.35;
  }
  if (u_cscopeRose > 0) col += polishRose(fragCoord, vec2(uRes.x * 0.5 - 62.0, uRes.y * 0.5 - 62.0), 34.0) * 0.85;
  /* aux: retardance through the cell at this pupil point, in units of 320 nm/µm */
  return vec4(col, clamp(retTotal / max(u_thick * 320.0, 1.0), 0.0, 1.0));
}
