/* =============================================================================
   march-sdf.glsl — sphere tracing for the volumetric modes (crystals, the
   liquid-crystal cell, the director stacks).
   The mode supplies:  float mapScene(vec3 p, out float mat);
   ============================================================================= */

float mapScene(vec3 p, out float mat);

Hit marchRay(vec3 ro, vec3 rd, float tmax) {
  Hit h;
  h.ok = false; h.t = 0.0; h.p = ro; h.n = vec3(0, 1, 0); h.mat = 0.0;
  float t = max(u_near, 0.0), mat = 0.0;
  for (int i = 0; i < 512; i++) {
    if (i >= u_maxSteps) break;
    vec3 p = ro + rd * t;
    float m = 0.0;
    float d = mapScene(p, m);
    if (d < u_eps * max(1.0, t)) { h.ok = true; h.t = t; h.p = p; h.mat = m; break; }
    t += d * u_stepScale;
    if (t > tmax) break;
  }
  if (!h.ok) { h.t = min(t, tmax); h.p = ro + rd * h.t; h.mat = -1.0; }
  return h;
}

vec3 calcNormal(vec3 p, out float mat) {
  float e = u_eps * 4.0;
  float m;
  vec2 k = vec2(1.0, -1.0);
  vec3 n = vec3(0.0);
  n += k.xyy * mapScene(p + k.xyy * e, m);
  n += k.yyx * mapScene(p + k.yyx * e, m);
  n += k.yxy * mapScene(p + k.yxy * e, m);
  n += k.xxx * mapScene(p + k.xxx * e, m);
  mat = 0.0;
  return normalize(n);
}

float calcAO(vec3 p, vec3 n, float mat) {
  float occ = 0.0, sca = 1.0;
  float m;
  for (int i = 0; i < 5; i++) {
    float hr = 0.02 + 0.16 * float(i) / 5.0;
    vec3 aop = p + n * hr;
    float dd = mapScene(aop, m);
    occ += (hr - dd) * sca;
    sca *= 0.72;
  }
  return clamp(1.0 - 1.6 * occ, 0.0, 1.0);
}

float softShadow(vec3 ro, vec3 rd, float mint, float maxt, float k, float mat0) {
  float res = 1.0, t = mint, m;
  for (int i = 0; i < 64; i++) {
    if (i >= int(max(8.0, u_maxSteps * 0.35))) break;
    float h = mapScene(ro + rd * t, m);
    res = min(res, k * h / max(t, 1e-4));
    t += clamp(h, 0.02, 0.5) * u_stepScale;
    if (res < 0.005 || t > maxt) break;
  }
  return clamp(res, 0.0, 1.0);
}
