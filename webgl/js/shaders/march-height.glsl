/* =============================================================================
   march-height.glsl — tracing a heightfield.

   Rock, clay and sand are surfaces, not volumes, and a heightfield gives a much
   better cost/quality trade than general sphere tracing: one evaluation per step,
   an analytic normal from the gradients, and a horizon-based occlusion that
   reads as terrain rather than as fog.

   The mode supplies:  float heightAt(vec2 p, out float mat);
                       float slopeBound();   // safe Lipschitz bound for stepping
   ============================================================================= */

float heightAt(vec2 p, out float mat);
float slopeBound();

Hit marchRay(vec3 ro, vec3 rd, float tmax) {
  Hit h;
  h.ok = false; h.t = tmax; h.p = ro + rd * tmax; h.n = vec3(0, 1, 0); h.mat = 0.0;
  float mat = 0.0;
  float t = max(u_near, 0.0);
  float hStart = heightAt(ro.xz, mat);
  float prevD = ro.y - hStart;
  if (prevD < 0.0) {           /* camera below the surface: start at the surface */
    t = 0.0;
  }
  float lip = max(0.5, slopeBound()) * u_stepScale;
  float denom = abs(rd.y) + lip * length(rd.xz);
  for (int i = 0; i < 512; i++) {
    if (i >= u_maxSteps) break;
    vec3 p = ro + rd * t;
    float m = 0.0;
    float d = p.y - heightAt(p.xz, m);
    if (d < u_eps * max(1.0, t)) {
      /* bisect the crossing for a clean silhouette */
      float t0 = max(t - max(1e-3, 4.0 * u_eps), 0.0), t1 = t;
      float mm = m;
      for (int k = 0; k < 6; k++) {
        float tm = 0.5 * (t0 + t1);
        vec3 pm = ro + rd * tm;
        float m2 = 0.0;
        float dm = pm.y - heightAt(pm.xz, m2);
        if (dm > 0.0) { t0 = tm; } else { t1 = tm; mm = m2; }
      }
      h.ok = true; h.t = t1; h.p = ro + rd * t1; h.mat = mm;
      break;
    }
    float step = max(0.7 * d / max(denom, 1e-4), 0.35 * u_eps);
    t += min(step, tmax * 0.5);
    if (t > tmax) break;
  }
  if (!h.ok) { h.t = min(t, tmax); h.p = ro + rd * h.t; h.mat = -1.0; }
  return h;
}

vec3 calcNormal(vec3 p, out float mat) {
  float e = max(u_eps * 6.0, 1e-4);
  float m;
  float hl = heightAt(p.xz - vec2(e, 0.0), m);
  float hr = heightAt(p.xz + vec2(e, 0.0), m);
  float hd = heightAt(p.xz - vec2(0.0, e), m);
  float hu = heightAt(p.xz + vec2(0.0, e), m);
  mat = heightAt(p.xz, m);
  return normalize(vec3(hl - hr, 2.0 * e, hd - hu));
}

float calcAO(vec3 p, vec3 n, float mat) {
  /* horizon occlusion: look in a few directions and ask whether the ground rises */
  float occ = 0.0;
  float m;
  float base = heightAt(p.xz, m);
  for (int i = 0; i < 6; i++) {
    float a = (float(i) + 0.5) * 1.0472 + hash12(p.xz) * 6.2831;
    float r = 0.06 + 0.5 * float(i) / 6.0;
    vec2 dir = vec2(cos(a), sin(a)) * r;
    float hh = heightAt(p.xz + dir, m);
    occ += clamp((hh - base) / max(r, 1e-3), 0.0, 1.0);
  }
  return clamp(1.0 - 0.31 * occ, 0.0, 1.0);
}

float softShadow(vec3 ro, vec3 rd, float mint, float maxt, float k, float mat0) {
  float m;
  float res = 1.0, t = mint;
  for (int i = 0; i < 64; i++) {
    if (i >= 32) break;
    vec3 p = ro + rd * t;
    float d = p.y - heightAt(p.xz, m);
    res = min(res, k * d / max(t, 1e-4));
    t += clamp(d, 0.02, 0.6) * u_stepScale;
    if (res < 0.004 || t > maxt) break;
  }
  return clamp(res, 0.0, 1.0);
}
