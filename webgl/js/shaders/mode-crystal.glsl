/* =============================================================================
   mode-crystal.glsl — one mineral that treats two polarisations differently.

   The habit is an exact convex distance field: an n-gonal prism intersected
   with a bicone, twinned by planar reflection, stacked like a real rock. The
   colour comes from a Jones calculation along the chord the ray actually takes
   through the stone, summed over a spectral band. Both rays are drawn, because
   the walk-off between them is the whole argument for anisotropy.
   ============================================================================= */

/* --------------------------------------------------------------- habit SDF -- */
/* iq's exact cone: apex at the origin, opening toward −y, base at y = −hb */
float sdConeApex(vec3 p, float rb, float hb) {
  vec2 q = vec2(rb, -hb);
  vec2 w = vec2(length(p.xz), p.y);
  vec2 a = w - q * clamp(dot(w, q) / max(dot(q, q), 1e-6), 0.0, 1.0);
  vec2 b = w - q * vec2(clamp(w.x / max(q.x, 1e-6), 0.0, 1.0), 1.0);
  float k = sign(q.y);
  float d = min(dot(a, a), dot(b, b));
  float s = max(k * (w.x * q.y - w.y * q.x), k * (w.y - q.y));
  return sqrt(d) * sign(s);
}
float sdBiCone(vec3 p, float rb, float hb) {
  float lower = sdConeApex(vec3(p.x, -(p.y + hb), p.z), rb, hb);
  float upper = sdConeApex(vec3(p.x, -(hb - p.y), p.z), rb, hb);
  return max(lower, upper);
}
float sdPrismN(vec3 p, float r, float n) {
  float d = -1e9;
  for (int i = 0; i < 12; i++) {
    if (float(i) >= n) break;
    float ang = float(i) / n * TAU + radians(u_xtlRot);
    vec2 nr = vec2(cos(ang), sin(ang));
    d = max(d, dot(p.xz, nr) - r);
  }
  return d;
}
float sdSeg(vec3 p, vec3 a, vec3 b, float r) {
  vec3 pa = p - a, ba = b - a;
  float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
  return length(pa - ba * h) - r;
}
float speciesSeed() { return fract(float(u_xtl) * 0.6180339887 + 0.137); }

float sdHabit(vec3 p) {
  float grow = clamp(u_xtlGrow, 0.05, 1.0);
  float r = (0.34 + 0.16 * speciesSeed()) * u_xtlSize * grow;
  float h = r * (0.75 + 0.85 * u_xtlElong);
  return max(sdPrismN(p, r, float(u_xtlFaces)), sdBiCone(p, r * 0.97, h));
}
float sdCrystal(vec3 p) {
  float d = sdHabit(p);
  /* twins: two lattices sharing a plane and disagreeing about everything else */
  for (int i = 0; i < 12; i++) {
    if (i >= u_xtlTwins) break;
    float fi = float(i) + 1.0;
    vec3 n = normalize(vec3(cos(fi * 2.1), sin(fi * 1.7) * 0.4, sin(fi * 2.1)));
    vec3 q = p - 2.0 * dot(p, n) * n;
    q = rotAxis(vec3(0.0, 1.0, 0.0), radians(u_xtlTwinAng) * (0.4 + 0.6 * hash11(fi))) * q;
    d = min(d, sdHabit(q));
  }
  /* a rock is never one crystal: stack the grains */
  float gap = max(u_xtlStackGap * 0.7, 0.05);
  for (int i = 1; i < 8; i++) {
    if (i >= u_xtlStack) break;
    float fi = float(i);
    vec3 c = vec3((hash11(fi * 3.1) - 0.5) * 0.7, fi * gap, (hash11(fi * 7.7) - 0.5) * 0.7);
    d = min(d, sdHabit(p - c) * 1.02);
  }
  /* inclusions: the fluid, the bubble, the other mineral */
  if (u_xtlInclude > 0) {
    vec3 cell = floor(p / 0.22);
    vec3 h = hash33(cell + 3.1);
    if (h.z < float(u_xtlInclude) / 64.0) {
      vec3 c = (cell + vec3(h.x, h.y, hash13(cell))) * 0.22;
      d = max(d, -sdSphere(p - c, 0.016 + 0.004 * u_xtlIncludeS));
    }
  }
  return d;
}
vec3 dominantAxis() {
  if (u_xtlAxis == 0) return vec3(0.0, 1.0, 0.0);
  if (u_xtlAxis == 1) return normalize(vec3(1.0, 0.25, 0.1));
  if (u_xtlAxis == 2) return normalize(vec3(0.1, 0.25, 1.0));
  return normalize(vec3(hash11(7.3) - 0.5, 1.0, hash11(9.1) - 0.5));
}
/* Walk-off: the extraordinary ray drifts away from its wave normal by tan ρ,
   where tan ρ = (n_e²−n_o²)sinθcosθ / (n_o²sin²θ + n_e²cos²θ). In calcite that
   is 6.2°, and it is why a calcite rhomb shows you two of everything. */
vec3 walkOff(vec3 axis, vec3 d) {
  vec2 nb = indicesAt(u_lambda * 1e-3);
  float ne = nb.y, no = nb.x;
  float cosT = clamp(dot(normalize(axis), normalize(d)), -1.0, 1.0);
  float sinT = sqrt(max(1.0 - cosT * cosT, 0.0));
  float tp = (ne * ne - no * no) * sinT * cosT / max(no * no * sinT * sinT + ne * ne * cosT * cosT, 1e-6);
  vec3 axisPerp = normalize(d - axis * dot(axis, d) * 0.999);
  return axisPerp * tp * 0.5;
}
float mapScene(vec3 p, out float mat) {
  float raw = sdCrystal(p);
  mat = 1.0;
  if (u_xtlRays > 0) {
    /* both rays leave the same point on the top face: one straight, one walking */
    vec3 axis = dominantAxis();
    vec3 dirO = normalize(vec3(0.16, -1.0, 0.10));
    vec3 dirE = normalize(dirO + walkOff(axis, dirO) * u_xtlWalk);
    vec3 a = vec3(0.0, 0.62, 0.0);
    float tO = sdSeg(p, a, a + dirO * 1.25, 0.006);
    float tE = sdSeg(p, a, a + dirE * 1.25, 0.006);
    if (tO < raw) { raw = tO; mat = 2.0; }
    if (tE < raw) { raw = tE; mat = 3.0; }
  }
  return raw;
}

/* --------------------------------------------------------- inside the stone -- */
/* the chord the ray takes, found by scanning the convex body then bisecting */
float exitT(vec3 p0, vec3 rd, float tStart) {
  float t = tStart + 0.004;
  float prev = sdCrystal(p0 + rd * t);
  for (int i = 0; i < 48; i++) {
    float tn = t + 0.012 + 0.02 * float(i);
    float d = sdCrystal(p0 + rd * tn);
    if (d > 0.0 && prev <= 0.0) {
      float a = t, b = tn;
      for (int k = 0; k < 7; k++) {
        float m = 0.5 * (a + b);
        if (sdCrystal(p0 + rd * m) < 0.0) a = m; else b = m;
      }
      return b;
    }
    prev = d;
    t = tn;
    if (t - tStart > 6.0) break;
  }
  return tStart + 0.2;
}

vec3 shadeCrystal(Hit h, vec3 rd, vec3 ro) {
  vec3 n = calcNormal(h.p, h.mat);
  vec3 V = -rd;
  vec3 L = lampDir(h.p, rd);
  float gain = lampGain(h.p, uTime);
  vec3 axis = dominantAxis();
  vec2 nb = indicesAt(u_lambda * 1e-3);

  if (h.mat > 1.5) {
    vec3 c = (h.mat < 2.5) ? vec3(0.35, 0.85, 1.0) : vec3(1.0, 0.55, 0.25);
    return c * (1.5 + 0.7 * uPulse);
  }

  /* refract into the stone, then find how far it has to travel inside */
  vec3 rdIn = refract(rd, n, 1.0 / max(nb.x, 1.001));
  if (dot(rdIn, rdIn) < 1e-6) rdIn = rd;
  float tOut = exitT(h.p, rdIn, 0.0);
  float chord = max(tOut - 0.004, 0.0);
  float chordUm = chord * (u_thick / max(u_xtlSize, 0.05)) * 0.5;

  vec3 acc = vec3(0.0);
  float wsum = 0.0;
  int bands = bandCount();
  float absorb = 0.5;
  for (int b = 0; b < 16; b++) {
    if (b >= bands) break;
    float lam = bandLambda(b, bands);
    float w = bandWeight(b, bands);
    vec4 sr = slabRetardance(axis, rdIn, chordUm, lam * 1e-3);
    J j = jSlab(TAU * sr.x / lam, sr.yz);
    float I = polarScope(j, lam) * coherenceEnv(sr.x);
    float cosT = abs(dot(normalize(axis), normalize(rdIn)));
    absorb = mix(luma(u_xtlPleochro), 1.0, cosT) * clamp(u_xtlCleave + 0.6, 0.0, 1.0);
    vec3 behind = (envHemisphere(vec3(0.0, 1.0, 0.0)) * 0.4 + u_sky * 1.2) * u_envInt * 2.2;
    acc += w * I * mix(behind, behind * u_xtlPleochro, clamp(1.0 - absorb, 0.0, 1.0)) * bandRGB(lam) * u_xtlContrast;
    wsum += w;
  }
  acc /= max(wsum, EPS);
  /* cloudy: light that scattered off inclusions and arrived without an opinion */
  acc = mix(acc, vec3(luma(acc)) * 1.2, clamp(u_xtlCloudy, 0.0, 1.0));
  /* the surface itself */
  float H = normalize(L + V);
  float sp = pow(max(dot(n, H), 0.0), u_shin * 0.6) * u_spec * 2.0;
  float fres = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 5.0);
  float sh = softShadow(h.p, L, 0.01, 3.0, shadowSharpness(), 1.0);
  vec3 surface = u_lightCol * gain * (sp + fres * 0.8);
  return acc * (0.55 + 0.6 * u_lightInt) * mix(0.72, 1.0, sh) + surface * u_lightInt;
}

vec4 render(vec2 fragCoord) {
  vec3 ro = cameraOrigin();
  vec3 rd = cameraRay(fragCoord);
  Hit h = marchRay(ro, rd, 30.0);
  vec3 col;
  if (h.ok) {
    col = shadeCrystal(h, rd, ro);
  } else {
    /* a dark table and a cool sky, so the stone has somewhere to be */
    float t = clamp(rd.y * 0.5 + 0.5, 0.0, 1.0);
    col = mix(vec3(0.015) + u_ground * 0.16, envHemisphere(vec3(0.0, 1.0, 0.0)) * 1.25, smoothstep(0.35, 1.0, t)) * u_envInt * 1.5;
    float sun = pow(max(dot(rd, normalize(vec3(cos(radians(u_lightAz)) * cos(radians(u_lightEl)),
                                             sin(radians(u_lightEl)),
                                             sin(radians(u_lightAz)) * cos(radians(u_lightEl))))), 0.0), 40.0);
    col += u_lightCol * sun * u_lightInt * 0.5;
  }
  if (u_cscopeRose > 0) col += polishRose(fragCoord, vec2(uRes.x * 0.5 - 62.0, uRes.y * 0.5 - 62.0), 34.0) * 0.8;
  /* aux: retardance along the chord, normalised for the palette */
  float aux = 0.0;
  if (h.ok && h.mat < 1.5) {
    vec3 n = calcNormal(h.p, h.mat);
    vec2 nb = indicesAt(u_lambda * 1e-3);
    vec3 rdIn = refract(rd, n, 1.0 / max(nb.x, 1.001));
    if (dot(rdIn, rdIn) < 1e-6) rdIn = rd;
    float chord = max(exitT(h.p, rdIn, 0.0) - 0.004, 0.0);
    float chordUm = chord * (u_thick / max(u_xtlSize, 0.05)) * 0.5;
    aux = clamp(chordUm * abs(nb.y - nb.x) * u_dnScale / 800.0, 0.0, 1.0);
  }
  return vec4(col, aux);
}
