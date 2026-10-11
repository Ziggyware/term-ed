/* =============================================================================
   lib.glsl — maths, procedural noise, distance fields, colour science, ramps.
   No scene dependency: the compositor shader includes this file too.
   ============================================================================= */

#define PI 3.141592653589793
#define TAU 6.283185307179586
#define INV_PI 0.3183098861837907
#define EPS 1e-5
#define INF 1e6

/* ------------------------------------------------------------------ hashes -- */
float hash11(float p) { p = fract(p * 0.1031); p *= p + 33.33; p *= p + p; return fract(p); }
float hash12(vec2 p) { vec3 p3 = fract(vec3(p.xyx) * 0.1031); p3 += dot(p3, p3.yzx + 33.33); return fract((p3.x + p3.y) * p3.z); }
float hash13(vec3 p3) { p3 = fract(p3 * 0.1031); p3 += dot(p3, p3.zyx + 31.32); return fract((p3.x + p3.y) * p3.z); }
vec2 hash22(vec2 p) { vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973)); p3 += dot(p3, p3.yzx + 33.33); return fract((p3.xx + p3.yz) * p3.zy); }
vec2 hash21(float p) { vec3 p3 = fract(vec3(p) * vec3(0.1031, 0.1030, 0.0973)); p3 += dot(p3, p3.yzx + 33.33); return fract((p3.xx + p3.yz) * p3.zy); }
vec3 hash33(vec3 p3) { p3 = fract(p3 * vec3(0.1031, 0.1030, 0.0973)); p3 += dot(p3, p3.yxz + 33.33); return fract((p3.xxy + p3.yxx) * p3.zyx); }

/* ------------------------------------------------------------------- noise -- */
float vnoise(vec2 x) {
  vec2 i = floor(x), f = fract(x);
  f = f * f * (3.0 - 2.0 * f);
  float a = hash12(i), b = hash12(i + vec2(1, 0)), c = hash12(i + vec2(0, 1)), d = hash12(i + vec2(1, 1));
  return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
/* gradient noise: the workhorse for landform relief */
float gnoise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  float a = dot(hash22(i) - 0.5, f);
  float b = dot(hash22(i + vec2(1, 0)) - 0.5, f - vec2(1, 0));
  float c = dot(hash22(i + vec2(0, 1)) - 0.5, f - vec2(0, 1));
  float d = dot(hash22(i + vec2(1, 1)) - 0.5, f - vec2(1, 1));
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y) * 1.4;
}
float gnoise3(vec3 p) {
  vec3 i = floor(p), f = fract(p);
  vec3 u = f * f * (3.0 - 2.0 * f);
  float n000 = dot(hash33(i) - 0.5, f);
  float n100 = dot(hash33(i + vec3(1,0,0)) - 0.5, f - vec3(1,0,0));
  float n010 = dot(hash33(i + vec3(0,1,0)) - 0.5, f - vec3(0,1,0));
  float n110 = dot(hash33(i + vec3(1,1,0)) - 0.5, f - vec3(1,1,0));
  float n001 = dot(hash33(i + vec3(0,0,1)) - 0.5, f - vec3(0,0,1));
  float n101 = dot(hash33(i + vec3(1,0,1)) - 0.5, f - vec3(1,0,1));
  float n011 = dot(hash33(i + vec3(0,1,1)) - 0.5, f - vec3(0,1,1));
  float n111 = dot(hash33(i + vec3(1,1,1)) - 0.5, f - vec3(1,1,1));
  return mix(mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y),
             mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y), u.z) * 1.4;
}
/* Worley: cell interiors, rims, crack networks */
vec2 worley(vec2 p) {
  vec2 ip = floor(p), fp = fract(p);
  float f1 = 8.0, f2 = 8.0;
  for (int j = -1; j <= 1; j++) {
    for (int i = -1; i <= 1; i++) {
      vec2 g = vec2(float(i), float(j));
      vec2 o = hash22(ip + g);
      vec2 r = g + o - fp;
      float d = dot(r, r);
      if (d < f1) { f2 = f1; f1 = d; } else if (d < f2) { f2 = d; }
    }
  }
  return vec2(sqrt(f1), sqrt(f2));
}
/* one fractal generator to rule the whole page; ridge mixes veins in */
float fbm(vec2 p, int oct, float gain, float lac, float ridge) {
  float s = 0.0, a = 0.5, n = 0.0;
  for (int i = 0; i < 12; i++) {
    if (i >= oct) break;
    float v = gnoise(p);
    float r = (1.0 - abs(v) * 2.0) * 2.0 - 1.0;
    s += a * mix(v, r, ridge);
    n += a;
    a *= gain;
    p = p * lac + vec2(1.7, 9.2);
  }
  return s / max(n, EPS);
}
float fbm3(vec3 p, int oct, float gain, float lac, float ridge) {
  float s = 0.0, a = 0.5, n = 0.0;
  for (int i = 0; i < 10; i++) {
    if (i >= oct) break;
    float v = gnoise3(p);
    float r = (1.0 - abs(v) * 2.0) * 2.0 - 1.0;
    s += a * mix(v, r, ridge);
    n += a;
    a *= gain;
    p = p * lac + vec3(1.7, 9.2, 4.3);
  }
  return s / max(n, EPS);
}
/* ridged multifractal: cracks, vein networks, coastal outlines */
float ridged(vec2 p, int oct, float gain, float lac) {
  float s = 0.0, a = 0.5, n = 0.0, prev = 1.0;
  for (int i = 0; i < 12; i++) {
    if (i >= oct) break;
    float v = 1.0 - abs(gnoise(p));
    v = v * v;
    s += a * v * prev;
    prev = v;
    n += a;
    a *= gain;
    p = p * lac + vec2(3.1, 7.7);
  }
  return s / max(n, EPS);
}

/* ------------------------------------------------------------------- hit --- */
struct Hit { float t; vec3 p; vec3 n; float mat; bool ok; };

/* -------------------------------------------------------------------- sdf --- */
float sdSphere(vec3 p, float r) { return length(p) - r; }
float sdBox(vec3 p, vec3 b) { vec3 q = abs(p) - b; return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0); }
float sdPlane(vec3 p, vec3 n, float h) { return dot(p, n) + h; }
float sdCapsule(vec3 p, float h, float r) { p.xz = vec2(length(p.xz) - h, p.y); return sdBox(p, vec3(r)) - r; }
float sdRoundCone(vec3 p, vec3 a, vec3 b, float r1, float r2) {
  vec3 ba = b - a; float l2 = dot(ba, ba); float rr = r1 - r2;
  float a2 = l2 - rr * rr; float il2 = 1.0 / l2;
  vec3 pa = p - a, ba2 = ba;
  float y = dot(pa, ba2), z = y - l2;
  vec3 x = pa * l2 - ba2 * y;
  float x2 = dot(x, x);
  float y2 = y * y * l2, z2 = z * z * l2;
  float k = sign(rr) * rr * rr * x2;
  if (sign(z) * a2 * z2 > k) return sqrt(x2 + z2) * il2 - r2;
  if (sign(y) * a2 * y2 < k) return sqrt(x2 + y2) * il2 - r1;
  return (sqrt(x2 * a2 * il2) + y * rr) * il2 - r1;
}
/* hexagonal prism: the basal habit of micas, ice and, arguably, clay stacks */
float sdHexPrism(vec3 p, vec2 h) {
  const vec3 k = vec3(-0.8660254, 0.5, 0.57735);
  p = abs(p);
  p.xy -= 2.0 * min(dot(k.xy, p.xy), 0.0) * k.xy;
  vec2 d = vec2(length(p.xy - vec2(clamp(p.x, -k.z * h.x, k.z * h.x), h.x)) * sign(p.y - h.x), p.z - h.y);
  return min(max(d.x, d.y), 0.0) + length(max(d, 0.0));
}
float sdTetra(vec3 p, float r) { return (max(abs(p.x + p.y) - p.z, abs(p.x - p.y) + p.z) - r) * 0.57735027; }
float sdOcta(vec3 p, float s) { p = abs(p); return (p.x + p.y + p.z - s) * 0.57735027; }
float smin(float a, float b, float k) {
  if (k <= 0.0) return min(a, b);
  float h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
  return mix(b, a, h) - k * h * (1.0 - h);
}
float smax(float a, float b, float k) { return -smin(-a, -b, k); }
float sdEllipsoidApprox(vec2 p, vec2 r) {
  float k0 = length(p / r), k1 = length(p / (r * r));
  return k0 * (k0 - 1.0) / max(k1, EPS);
}

/* --------------------------------------------------------------- matrices -- */
mat3 rotAxis(vec3 axis, float angle) {
  axis = normalize(axis);
  float s = sin(angle), c = cos(angle), t = 1.0 - c;
  return mat3(
    t * axis.x * axis.x + c,        t * axis.x * axis.y - s * axis.z, t * axis.x * axis.z + s * axis.y,
    t * axis.x * axis.y + s * axis.z, t * axis.y * axis.y + c,        t * axis.y * axis.z - s * axis.x,
    t * axis.x * axis.z - s * axis.y, t * axis.y * axis.z + s * axis.x, t * axis.z * axis.z + c);
}
mat3 rotXYZ(vec3 a) {
  float cx = cos(a.x), sx = sin(a.x), cy = cos(a.y), sy = sin(a.y), cz = cos(a.z), sz = sin(a.z);
  mat3 rx = mat3(1, 0, 0, 0, cx, -sx, 0, sx, cx);
  mat3 ry = mat3(cy, 0, sy, 0, 1, 0, -sy, 0, cy);
  mat3 rz = mat3(cz, -sz, 0, sz, cz, 0, 0, 0, 1);
  return rz * ry * rx;
}

/* ---------------------------------------------------------------- colour --- */
float luma(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }
vec3 srgbEncode(vec3 c) { return mix(c * 12.92, 1.055 * pow(max(c, 0.0), vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)); }
vec3 srgbDecode(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec3 hsv2rgb(vec3 c) {
  vec3 p = abs(fract(c.xxx + vec3(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
  return c.z * mix(vec3(1.0), clamp(p - 1.0, 0.0, 1.0), c.y);
}
vec3 rgb2hsv(vec3 c) {
  vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
  vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
  vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
  float d = q.x - min(q.w, q.y);
  return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + EPS)), d / (q.x + EPS), q.x);
}
vec3 satAdjust(vec3 c, float s) {
  float l = luma(c);
  return mix(vec3(l), c, s);
}
vec3 contrastAdjust(vec3 c, float k) { return (c - 0.5) * k + 0.5; }
vec3 acesFilm(vec3 x) { /* Narkowicz’s ACES fit */
  const float a = 2.51, b = 0.03, c = 2.43, d = 0.59, e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}
vec3 reinhard(vec3 x) { return x / (1.0 + x); }
vec3 uncharted2(vec3 x) {
  const float A = 0.15, B = 0.50, C = 0.10, D = 0.20, E = 0.02, F = 0.30;
  return ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F;
}
vec3 filmic(vec3 x) {
  vec3 c = uncharted2(x * 2.0);
  return clamp(c / uncharted2(vec3(11.2)), 0.0, 1.0);
}
/* a log-encoded sigmoid in the spirit of AgX — labelled an approximation in the UI */
vec3 agxLike(vec3 c) {
  const float lo = -4.0, hi = 6.5;
  vec3 l = log2(max(c, 1e-6));
  vec3 n = clamp((l - lo) / (hi - lo), 0.0, 1.0);
  vec3 s = n * n * (3.0 - 2.0 * n);
  s = mix(s, s * s * (3.0 - 2.0 * s), 0.5);
  return srgbEncode(clamp(s, 0.0, 1.0));
}
vec3 tonemap(int id, vec3 c) {
  if (id == 0) return clamp(c, 0.0, 1.0);
  if (id == 1) return reinhard(c);
  if (id == 2) return acesFilm(c);
  if (id == 3) return filmic(c);
  return agxLike(c);
}

/* ------------------------------------------------------------------ ramps -- */
vec3 ramp10(float t, vec3 c0, vec3 c1, vec3 c2, vec3 c3, vec3 c4, vec3 c5, vec3 c6, vec3 c7, vec3 c8, vec3 c9) {
  vec3 cs[10] = vec3[10](c0, c1, c2, c3, c4, c5, c6, c7, c8, c9);
  float x = clamp(t, 0.0, 1.0) * 9.0;
  int i = int(floor(x));
  float f = fract(x);
  f = f * f * (3.0 - 2.0 * f);
  return mix(cs[clamp(i, 0, 9)], cs[clamp(i + 1, 0, 9)], f);
}
vec3 ramp6(float t, vec3 c0, vec3 c1, vec3 c2, vec3 c3, vec3 c4, vec3 c5) {
  vec3 cs[6] = vec3[6](c0, c1, c2, c3, c4, c5);
  float x = clamp(t, 0.0, 1.0) * 5.0;
  int i = int(floor(x));
  float f = fract(x);
  f = f * f * (3.0 - 2.0 * f);
  return mix(cs[clamp(i, 0, 5)], cs[clamp(i + 1, 0, 5)], f);
}
vec3 ramp(int id, float t) {
  t = clamp(t, 0.0, 1.0);
  if (id == RAMP_TURBO) return ramp10(t,
    vec3(0.19, 0.07, 0.23), vec3(0.25, 0.27, 0.67), vec3(0.27, 0.46, 0.93), vec3(0.22, 0.63, 0.99), vec3(0.11, 0.81, 0.83),
    vec3(0.24, 0.94, 0.55), vec3(0.62, 0.99, 0.24), vec3(0.98, 0.73, 0.22), vec3(0.96, 0.39, 0.12), vec3(0.48, 0.02, 0.01));
  if (id == RAMP_MAGMA) return ramp10(t,
    vec3(0.00, 0.00, 0.02), vec3(0.09, 0.06, 0.24), vec3(0.27, 0.06, 0.46), vec3(0.45, 0.12, 0.51), vec3(0.62, 0.18, 0.50),
    vec3(0.80, 0.25, 0.44), vec3(0.95, 0.38, 0.36), vec3(0.99, 0.59, 0.41), vec3(1.00, 0.79, 0.55), vec3(0.99, 0.99, 0.75));
  if (id == RAMP_INFERNO) return ramp10(t,
    vec3(0.00, 0.00, 0.02), vec3(0.11, 0.05, 0.25), vec3(0.29, 0.05, 0.42), vec3(0.47, 0.11, 0.43), vec3(0.65, 0.17, 0.38),
    vec3(0.81, 0.27, 0.27), vec3(0.93, 0.41, 0.15), vec3(0.98, 0.61, 0.02), vec3(0.97, 0.82, 0.24), vec3(0.99, 1.00, 0.64));
  if (id == RAMP_PLASMA) return ramp10(t,
    vec3(0.05, 0.03, 0.53), vec3(0.27, 0.01, 0.62), vec3(0.45, 0.00, 0.66), vec3(0.61, 0.09, 0.62), vec3(0.74, 0.22, 0.53),
    vec3(0.85, 0.34, 0.42), vec3(0.93, 0.47, 0.33), vec3(0.98, 0.62, 0.23), vec3(0.99, 0.79, 0.15), vec3(0.94, 0.98, 0.13));
  if (id == RAMP_VIRIDIS) return ramp10(t,
    vec3(0.27, 0.00, 0.33), vec3(0.28, 0.15, 0.47), vec3(0.25, 0.29, 0.54), vec3(0.19, 0.40, 0.56), vec3(0.15, 0.51, 0.56),
    vec3(0.12, 0.62, 0.54), vec3(0.42, 0.79, 0.42), vec3(0.71, 0.87, 0.17), vec3(0.83, 0.90, 0.18), vec3(0.99, 0.91, 0.15));
  if (id == RAMP_CIVIDIS) return ramp10(t,
    vec3(0.00, 0.13, 0.30), vec3(0.00, 0.20, 0.44), vec3(0.22, 0.28, 0.42), vec3(0.34, 0.36, 0.43), vec3(0.44, 0.44, 0.45),
    vec3(0.54, 0.53, 0.47), vec3(0.65, 0.62, 0.46), vec3(0.77, 0.71, 0.42), vec3(0.89, 0.81, 0.36), vec3(1.00, 0.92, 0.27));
  if (id == RAMP_SPECTRAL) {
    float h = mix(0.68, 0.0, t);
    float v = 0.45 + 0.55 * (0.5 + 0.5 * cos(TAU * (t - 0.25)));
    return hsv2rgb(vec3(h, 0.72, clamp(v, 0.0, 1.0)));
  }
  if (id == RAMP_EARTH) return ramp6(t,
    vec3(0.10, 0.07, 0.05), vec3(0.29, 0.20, 0.13), vec3(0.44, 0.32, 0.20),
    vec3(0.61, 0.47, 0.31), vec3(0.79, 0.68, 0.51), vec3(0.94, 0.92, 0.85));
  if (id == RAMP_HOLO) return ramp6(t,
    vec3(0.03, 0.05, 0.16), vec3(0.11, 0.35, 0.72), vec3(0.16, 0.72, 0.83),
    vec3(0.45, 0.95, 0.75), vec3(0.91, 0.86, 0.97), vec3(0.62, 0.35, 0.94));
  if (id == RAMP_EMBER) return ramp6(t,
    vec3(0.03, 0.01, 0.01), vec3(0.23, 0.03, 0.02), vec3(0.51, 0.10, 0.02),
    vec3(0.80, 0.30, 0.04), vec3(0.97, 0.62, 0.15), vec3(1.00, 0.96, 0.78));
  if (id == RAMP_PATINA) return ramp6(t,
    vec3(0.04, 0.09, 0.08), vec3(0.09, 0.28, 0.24), vec3(0.22, 0.52, 0.42),
    vec3(0.55, 0.74, 0.58), vec3(0.90, 0.80, 0.63), vec3(0.74, 0.51, 0.35));
  if (id == RAMP_NICHE) return ramp6(t,
    vec3(0.07, 0.09, 0.28), vec3(0.24, 0.33, 0.55), vec3(0.51, 0.62, 0.71),
    vec3(0.82, 0.79, 0.66), vec3(0.86, 0.62, 0.52), vec3(0.42, 0.18, 0.30));
  return vec3(t);
}

/* the palette select includes "none" as option 0, the ramps follow it */
vec3 paletteAt(int sel, float t) {
  return (sel > 0) ? ramp(sel - 1, t) : vec3(clamp(t, 0.0, 1.0));
}

/* ---------------------------------------------------------------- display -- */
/* sRGB-ish output with a dither to stop 8-bit banding in the deep gradient areas */
vec3 outputTransform(vec3 linear, vec2 px, float dither) {
  vec3 c = clamp(linear, 0.0, 1.0);
  vec3 s = srgbEncode(c);
  float n = (hash12(px) - 0.5) + (hash12(px + 31.7) - 0.5);
  s += n * (dither / 255.0);
  return s;
}

/* ----------------------------------------------------------------- camera -- */
/* uCamO/R/U/F are computed on the CPU so the camera maths lives in one place.

   g_cone is an aperture sample set by the supersampling loop. Physically an
   aperture point does two things at once: it shifts where the ray starts and it
   changes where that ray is aimed. Doing both is what makes defocus real, and
   doing neither is why a single ray per pixel looks like a diagram. */
vec2 g_cone = vec2(0.0);

vec3 cameraRay(vec2 fragCoord) {
  vec2 uv = (fragCoord - 0.5 * uRes.xy) / uRes.y;
  vec3 d;
  if (u_camMode == 5) {
    /* orthographic: the view a thin section gets */
    return normalize(uCamF);
  }
  d = normalize(uCamF + uCamR * (uv.x * uTanHalfFov * uAspect) + uCamU * (uv.y * uTanHalfFov));
  float aperture = max(u_dof, 0.0) * 0.02 * u_dist * 0.5;
  float cone = max(u_raySpread, 0.0) * 0.06;
  vec3 shift = (uCamR * g_cone.x + uCamU * g_cone.y) * (aperture + cone);
  if (dot(shift, shift) < 1e-12) return d;
  vec3 o = uCamO + shift;
  vec3 focusPoint = uCamO + d * max(u_dofFocus, 0.02);
  return normalize(focusPoint - o);
}
vec3 cameraOrigin() {
  float aperture = max(u_dof, 0.0) * 0.02 * u_dist * 0.5;
  float cone = max(u_raySpread, 0.0) * 0.06;
  return uCamO + (uCamR * g_cone.x + uCamU * g_cone.y) * (aperture + cone);
}
/* one sample of a jittered AA pattern, deterministic per pixel */
vec2 aaOffset(int i, int n, vec2 px) {
  if (n <= 1) return vec2(0.0);
  float fi = float(i);
  float a = fi * 2.39996323 + hash12(px) * TAU;
  float r = 0.5 * sqrt((fi + 0.5) / float(n));
  return vec2(cos(a), sin(a)) * r;
}
/* low-discrepancy disk for pupil / aperture sampling */
vec2 diskSample(int i, int n) {
  float fi = float(i) + 0.5;
  float a = fi * 2.39996323 + u_phase * TAU;
  return vec2(cos(a), sin(a)) * sqrt(fi / max(float(n), 1.0));
}
