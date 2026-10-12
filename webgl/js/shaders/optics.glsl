/* =============================================================================
   optics.glsl — the physics, shared by four of the seven modes.

   Conventions
     · wavelengths in nanometres in the UI, micrometres in the index maths
     · retardance = Δn·d, carried in nanometres
     · Jones matrices are 2×2 over the complex numbers written as four vec2
       (re, im) pairs, so the shaders need no operator overloading
     · the layer normal is +z; the lab x axis is the polariser reference
     · angles arrive from the UI in degrees and are converted here

   Honesty notes, since this page is partly about how knowledge is made:
     exact        Cauchy dispersion, the index ellipsoid, Snell, the coherence
                  envelope, Jones accumulation through a stratified medium
     approximate  each liquid-crystal slice is a uniformly tilted uniaxial slab
                  (the standard Jones approximation), disclinations are drawn as
                  fields rather than solved singularities, and every geological
                  parameter is procedural by construction — as labelled in the UI
   ============================================================================= */

/* ------------------------------------------------------ 1. refractive index -- */
/* n(λ) = n(550 nm) + A·(1/λ² − 1/0.55²), the Cauchy form. The extraordinary
   axis disperses harder than the ordinary one, which is what colours the rings. */
vec2 indicesAt(float lamUm) {
  float d = u_disp * (1.0 / max(lamUm * lamUm, 1e-4) - 3.305785);
  return vec2(max(u_no + 0.80 * d, 1.005), max(u_ne + 1.25 * d, 1.005));
}
/* index of the extraordinary wave whose normal is at angle acos(cosT) to the
   optic axis:  1/n² = cos²θ/n_e² + sin²θ/n_o² */
float neff(float cosT, float ne, float no) {
  float c2 = cosT * cosT, s2 = max(1.0 - c2, 0.0);
  return inversesqrt(max(c2 / (ne * ne) + s2 / (no * no), 1e-8));
}

/* --------------------------------------------------- 2. one oblique thin slab -- */
/* Returns  xy = retardance of the slice in nanometres
              zw = the in-plane slow axis (unit vector, lab frame)
   The slow axis is the director's component perpendicular to the ray, which is
   what rotates as the ray sweeps round the pupil and therefore what draws the
   Maltese cross in a conoscopic figure. */
vec4 slabRetardance(vec3 u, vec3 d, float dzUm, float lamUm) {
  vec2 nb = indicesAt(lamUm);
  u = normalize(u); d = normalize(d);
  float cosT = clamp(abs(dot(u, d)), 0.0, 1.0);
  float ne = neff(cosT, nb.y, nb.x);
  float dn = (ne - nb.x) * u_dnScale;
  float dr = (ne - nb.y) * u_dnScale;
  vec3 e2 = cross(d, cross(u, d));              /* director ⟂ ray */
  float l = length(e2);
  vec2 axis = (l > 1e-4) ? normalize(e2.xy) : vec2(1.0, 0.0);
  /* path lengthening: the ray refracts, so it crosses more than dz of material */
  float nAvg = 0.5 * (nb.x + nb.y);
  float sinExt = clamp(length(d.xy), 0.0, 0.999);
  float cosInt = sqrt(max(1.0 - (sinExt / nAvg) * (sinExt / nAvg), 0.04));
  /* u_opd is the honest cheat: a multiplier on the optical path only */
  return vec4(vec2(dn * dzUm * 1e3 * u_opd / cosInt,
              dr * dzUm * 1e3 * u_opd / cosInt), axis);
}

/* --------------------------------------------------------- 3. Jones algebra -- */
struct J { vec2 a; vec2 b; vec2 c; vec2 d; };   /* [[a,b],[c,d]] complex */

vec2 cmul(vec2 x, vec2 y) { return vec2(x.x * y.x - x.y * y.y, x.x * y.y + x.y * y.x); }
vec2 cadd(vec2 x, vec2 y) { return x + y; }
vec2 cexp2(float ph) { return vec2(cos(ph), sin(ph)); }

J jIdentity() {
  J j; j.a = vec2(1.0, 0.0); j.b = vec2(0.0, 0.0); j.c = vec2(0.0, 0.0); j.d = vec2(1.0, 0.0); return j;
}
/* R(axis) · diag(e^{-iδ/2}, e^{+iδ/2}) · R(axis)^T, expanded so the symmetry
   J00 = c²α + s²β, J11 = s²α + c²β, J01 = J10 = cs(α−β) is visible */
J jSlab(float delta, vec2 axis) {
  float c = axis.x, s = axis.y, h = 0.5 * delta;
  vec2 al = vec2(cos(h), -sin(h));                         /* e^{-iδ/2} */
  vec2 be = vec2(cos(h), sin(h));                          /* e^{+iδ/2} */
  J j;
  j.a = cadd(vec2(c * c * al.x, c * c * al.y), vec2(s * s * be.x, s * s * be.y));
  j.d = cadd(vec2(s * s * al.x, s * s * al.y), vec2(c * c * be.x, c * c * be.y));
  j.b = vec2(c * s * (al.x - be.x), c * s * (al.y - be.y));
  j.c = j.b;
  return j;
}
J jmul(J x, J y) {
  J r;
  r.a = cadd(cmul(x.a, y.a), cmul(x.b, y.c));
  r.b = cadd(cmul(x.a, y.b), cmul(x.b, y.d));
  r.c = cadd(cmul(x.c, y.a), cmul(x.d, y.c));
  r.d = cadd(cmul(x.c, y.b), cmul(x.d, y.d));
  return r;
}
/* a quarter-wave plate at the lamp wavelength, slow axis at angle w */
J jQWP(float w, float lamNm) {
  return jSlab(PI * 0.5 * (u_lambda / max(lamNm, 1.0)), vec2(cos(w), sin(w)));
}
/* transmitted intensity for a polariser at pRad, the medium j, an analyser at
   aRad. noAnalyser = 1 sums both output components instead. */
float jonesIntensity(J j, float pRad, float aRad, int noAnalyser) {
  vec2 eP = vec2(cos(pRad), sin(pRad));
  vec2 Ex = cadd(cmul(j.a, vec2(eP.x, 0.0)), cmul(j.b, vec2(eP.y, 0.0)));
  vec2 Ey = cadd(cmul(j.c, vec2(eP.x, 0.0)), cmul(j.d, vec2(eP.y, 0.0)));
  if (noAnalyser == 1) return dot(Ex, Ex) + dot(Ey, Ey);
  vec2 eA = vec2(cos(aRad), sin(aRad));
  vec2 E = vec2(eA.x * Ex.x + eA.y * Ey.x, eA.x * Ex.y + eA.y * Ey.y);
  return dot(E, E);
}
/* The whole polariscope, including the two quarter-wave plates that turn a
   linear instrument into a circular one — which erases the isoclinic brushes
   and leaves nothing but the isochromes. */
float polarScope(J j, float lamNm) {
  float P = radians(u_polP);
  float A = radians(u_polA);
  int m = u_polMode;
  if (m == 0) A = P + PI * 0.5;              /* crossed */
  else if (m == 1) A = P;                    /* parallel */
  else if (m == 3) return 1.0;               /* no polarisers at all */
  if (u_retExtra != 0.0) {
    J comp = jSlab(TAU * u_retExtra / max(lamNm, 1.0), vec2(cos(P + 0.785398), sin(P + 0.785398)));
    j = jmul(comp, j);
  }
  if (m == 4) {
    J q1 = jQWP(P + 0.785398, lamNm);
    J q2 = jQWP(P - 0.785398, lamNm);
    j = jmul(q2, jmul(j, q1));
    A = P + PI * 0.5;
  }
  if (m == 2) return jonesIntensity(j, P, 0.0, 1.0);
  return jonesIntensity(j, P, A, 0.0);
}

/* ------------------------------------------------------------- 4. lamp model -- */
int bandCount() { return clamp(u_bands, 1, 16); }
float bandLambda(int i, int n) {
  if (n <= 1) return u_lambda;
  float f = (float(i) / float(n - 1)) * 2.0 - 1.0;
  return clamp(u_lambda + f * u_bandW * 0.5, 380.0, 780.0);
}
float bandWeight(int i, int n) {
  if (n <= 1) return 1.0;
  float f = (float(i) / float(n - 1)) * 2.0 - 1.0;
  return exp(-2.0 * f * f);
}
/* temporal coherence: fringe contrast survives while OPD < coherence length.
   A 5 nm filter remembers about 60 µm; white light about a micrometre. */
float coherenceEnv(float opdNm) {
  float lc = max(1.0, u_lambda * u_lambda / max(u_bandW, 1.0));
  float r = abs(opdNm) / lc;
  return mix(exp(-r * r * 1.6), 1.0, clamp(u_coh, 0.0, 1.0));
}
/* per-band colours: an approximate CIE-ish weighting, good enough to make the
   interference colours land where the eye expects them */
vec3 bandRGB(float lamNm) {
  float a = 1.0 / (lamNm * lamNm);
  return vec3(
    exp(-0.0005 * (lamNm - 620.0) * (lamNm - 620.0)),
    exp(-0.0012 * (lamNm - 545.0) * (lamNm - 545.0)),
    exp(-0.0014 * (lamNm - 465.0) * (lamNm - 465.0)));
}

/* -------------------------------------------------------------- 5. geology -- */
struct Mem { float h; float age; float wet; vec2 dir; };
Mem memSample(vec2 uv) {
  vec4 m = texture(uMemory, uv);
  float a = m.a * 6.2831853;
  Mem r;
  r.h = m.r * 2.0 - 1.0;      /* signed: 0 = flat ground */
  r.age = m.g;
  r.wet = m.b;
  r.dir = vec2(cos(a), sin(a));
  return r;
}

/* ------------------------------------------------- 6. liquid-crystal director -- */
vec3 lcDirector(float tiltRad, float twistRad) {
  float c = cos(tiltRad), s = sin(tiltRad);
  return vec3(s * cos(twistRad), s * sin(twistRad), c);
}
/* Freedericksz: nothing at all happens until V = Vth = π√(K11/Δε); then the
   tilt grows smoothly away from the threshold rather than jumping. */
float freedericksz(float volt) {
  float v = max(volt, 1.0001);
  return smoothstep(1.0, 2.6, v) * (0.62 + 0.38 * smoothstep(1.0, 4.5, v));
}
/* tilt as a function of depth for the common cell geometries. This is the
   function that decides whether a cell is a display, a lens or a failure. */
float lcTiltProfile(float s, float tiltRad) {
  int p = u_lcTiltProf;
  if (p == 0) return tiltRad;                                     /* uniform */
  if (p == 1) return tiltRad * (0.35 + 0.65 * sin(PI * s));       /* parabolic */
  if (p == 2) return tiltRad * s;                                 /* hybrid */
  if (p == 3) return tiltRad * (0.5 - 0.5 * cos(PI * s));         /* splay */
  if (p == 4) return tiltRad * sin(PI * s) * 0.9;                 /* bow */
  if (p == 5) return tiltRad * abs(2.0 * s - 1.0);                /* mirror */
  return tiltRad * (0.5 + 0.5 * cos(TAU * s));                    /* PAA-like */
}
float lcTwistProfile(float s, float twistRad) {
  int p = u_lcTwistProf;
  if (p == 0) return twistRad * s;                                /* linear */
  if (p == 1) { float k = floor(s * 6.0) / 6.0; return twistRad * (k + smoothstep(0.0, 0.16, fract(s * 6.0)) / 6.0); }
  if (p == 2) return twistRad * 0.5 * (1.0 - cos(PI * s));        /* π-cell */
  if (p == 3) return twistRad * smoothstep(0.15, 0.85, s);        /* threshold */
  return twistRad * s;                                            /* helix, added below */
}

/* ------------------------------------------------- 7. instrument overlay -- */
/* P, A and the sample axis, drawn rather than assumed */
/* polar optics drawn in the corner: a rose showing P, A and the optic axis */
vec3 polishRose(vec2 px, vec2 c, float R) {
  vec2 q = (px - c) / R;
  float r = length(q);
  vec3 col = vec3(0.0);
  if (r < 1.25) {
    float ring = smoothstep(0.03, 0.0, abs(r - 1.0));
    col += ring * vec3(0.5);
    float cp = cos(radians(u_polP)), sp = sin(radians(u_polP));
    float ca = cos(radians(u_polA)), sa = sin(radians(u_polA));
    float dP = abs(q.x * sp - q.y * cp) / max(r, 0.02);
    float dA = abs(q.x * sa - q.y * ca) / max(r, 0.02);
    col += vec3(0.2, 0.9, 1.0) * smoothstep(0.09, 0.0, dP) * step(r, 1.0) * 0.8;
    col += vec3(1.0, 0.6, 0.2) * smoothstep(0.09, 0.0, dA) * step(r, 1.0) * 0.8;
    col += vec3(0.6, 0.5, 1.0) * smoothstep(0.02, 0.0, abs(r - 0.62)) * 0.7;
  }
  return col;
}


/* ------------------------------------------------------------------ 8. light -- */
/* One place for the illumination, so every mode answers to the same rig. */
vec3 lampPos() {
  vec3 L = normalize(vec3(cos(radians(u_lightAz)) * cos(radians(u_lightEl)),
                          sin(radians(u_lightEl)),
                          sin(radians(u_lightAz)) * cos(radians(u_lightEl))));
  return L * max(u_lightDist, 0.05);
}
vec3 lampDir(vec3 p, vec3 rd) {
  if (u_lightMode == 0) return -rd;                       /* headlamp */
  vec3 pos = lampPos();
  if (u_lightMode == 2) pos += vec3(0.6, 0.35, 0.0) * max(u_lightDist, 0.05) * 0.5;   /* studio: a big source */
  return normalize(pos - p);
}
/* intensity, with the cycle driving it if you ask it to */
float lampGain(vec3 p, float t) {
  float pulseAmt = clamp(abs(u_lightPulse), 0.0, 1.0);
  float g = mix(1.0, 0.35 + 1.1 * uPulse, pulseAmt) * u_lightInt;
  g *= 1.0 + u_lightJitter * (hash13(p * 3.1 + t) - 0.5) * 2.0;
  if (u_lightPulse < 0.0) g *= 0.35 + 1.1 * (1.0 - uPulse);    /* inverted coupling */
  return max(g, 0.0);
}
/* hemisphere ambient with a rotatable environment */
vec3 envHemisphere(vec3 n) {
  vec3 up = rotAxis(vec3(0.0, 1.0, 0.0), radians(u_envRot)) * n;
  return mix(u_ground, u_sky, clamp(up.y * 0.5 + 0.5, 0.0, 1.0));
}
float shadowSharpness() {
  return mix(4.0, 22.0, clamp(1.0 - (u_soft * 0.6 + u_shadowSoft * 0.4), 0.0, 1.0));
}

/* ------------------------------------------------- 9. the drive, honestly --- */
/* V_eff follows the waveform with the cell's own time constant, which the CPU
   integrates (it is a first-order lag, so doing it once per frame is right).
   The DC offset is what a real cell must avoid: it electrolyses the mixture. */
float driveV() {
  return clamp(u_volt + u_voltBias * 0.35 + (uLC - 0.5) * u_voltMod * 1.5, 0.0, 3.0);
}
/* the nematic order parameter falls as the clearing point approaches; both the
   birefringence and the noise scale with it */
float lcOrderFactor() {
  return clamp(u_lcOrder, 0.0, 1.0) * (1.0 - smoothstep(55.0, 95.0, u_lcTemp));
}
