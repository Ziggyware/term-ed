/* =============================================================================
   mode-director.glsl — the nematic cell as a physical object.

   A cell is a 5 µm slab of rod-shaped molecules between two plates. This mode
   draws the rods themselves, using a modulus-folded lattice so that the SDF
   stays cheap while the director field still varies with depth — which is the
   only thing in this picture that matters.

   Not to scale, and proud of it: real molecules are 2 nm across in a 5000 nm
   gap, so any picture of both at once is a lie of aspect ratio.
   ============================================================================= */

#define CELLT cellThickness()

float cellThickness() { return clamp(u_thick * 0.11, 0.18, 3.2); }
float cellWidth() { return 2.0; }

/* director in scene coords: scene up is +y, optics uses +z as the layer normal */
vec3 lcDirWorld(vec3 p) {
  float s = clamp(p.y / CELLT + 0.5, 0.0, 1.0);
  float vTilt = freedericksz(u_volt) * u_voltMod;
  float pre = radians(u_prt) + radians(u_lcTilt);
  float tilt = lcTiltProfile(s, pre + vTilt * 1.35);
  float tw = lcTwistProfile(s, radians(u_lcTwist));
  /* a chiral dopant twists the director its own way through depth */
  float helix = u_lcPitch > 0.001 ? TAU * (u_thick / max(u_lcPitch, 0.001)) * s : 0.0;
  tw += helix;
  /* voltage untwists a twisted cell toward the field: the whole point of a TN */
  tw *= mix(1.0, 0.25, clamp(freedericksz(u_volt) * u_voltMod, 0.0, 1.0));
  /* drive patterning: a cell is not obliged to be uniform laterally */
  vec2 q = p.xz;
  if (u_lcVoltProf == 1) tw += q.y * 0.6;
  else if (u_lcVoltProf == 2) tw += step(0.0, sin(q.x * 6.0) * sin(q.y * 6.0)) * 1.2;
  else if (u_lcVoltProf == 3) tw += atan(q.y, q.x) * 1.5;
  else if (u_lcVoltProf == 5) tw += length(q) * 2.4;
  /* thermal wobble: nothing about a liquid crystal is quiet */
  vec3 wob = vec3(gnoise3(p * 3.7 + uTime * 0.4), gnoise3(p * 3.7 + 19.3), gnoise3(p * 3.7 + 41.7));
  vec3 u = lcDirector(tilt, tw);
  vec3 uw = normalize(vec3(u.x, u.z, u.y) + wob * 0.16 * (1.0 - u_lcOrder * 0.8));
  return uw;
}

mat3 basisAlign(vec3 u) {
  vec3 a = abs(u);
  vec3 t = (a.x < a.y && a.x < a.z) ? vec3(1, 0, 0) : ((a.y < a.z) ? vec3(0, 1, 0) : vec3(0, 0, 1));
  vec3 v1 = normalize(cross(t, u));
  vec3 v2 = cross(u, v1);
  return mat3(v1, v2, u);
}
float sdSegment(vec3 p, vec3 a, vec3 b, float r) {
  vec3 pa = p - a, ba = b - a;
  float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
  return length(pa - ba * h) - r;
}
float sdZCapsule(vec3 q, float R, float L) {
  vec2 d = vec2(length(q.xy) - R, abs(q.z) - L);
  return min(max(d.x, d.y), 0.0) + length(max(d, 0.0));
}

/* facets: 1 rod lattice · 2 spacer · 3 light ray · 4 field line · 5 defect core */
float mapScene(vec3 p, out float mat) {
  mat = 1.0;
  vec3 bound = vec3(cellWidth() * 1.3, CELLT * 1.7, cellWidth() * 1.3);
  if (any(greaterThan(abs(p), bound))) {
    vec3 q = abs(p) - bound;
    return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0) + 0.05;
  }

  /* ---- the rod lattice: fold space, then orient per lattice cell ---- */
  float R = max(u_lcRodRad * 0.5 * CELLT * 1.6, 0.012);
  float L = max(u_lcRodLen * 0.35 * CELLT, R * 1.4);
  float spacing = max(R * 7.4, 0.06);
  vec3 gp = p / spacing;
  vec3 cell = floor(gp);
  vec3 f = (gp - cell - 0.5) * spacing;
  vec3 centre = (cell + 0.5) * spacing;
  vec3 u = lcDirWorld(centre);
  float dRod = sdZCapsule(transpose(basisAlign(u)) * f, R, L);

  float d = dRod;
  mat = 1.0;

  /* ---- ball spacers: real cells are held apart by sprinkles ---- */
  for (int i = 0; i < 4; i++) {
    vec2 h = hash22(vec2(float(i) * 3.7 + 1.3, 0.7));
    vec3 c = vec3((h.x - 0.5) * cellWidth() * 1.5, 0.0, (h.y - 0.5) * cellWidth() * 1.5);
    float ds = length(p - c) - CELLT * 0.42;
    if (ds < d) { d = ds; mat = 2.0; }
  }

  /* ---- electric field lines: vertical, and denser as the field strengthens -- */
  if (u_lcStress > 0.01) {
    vec2 fold = mod(p.xz + cellWidth(), 2.0 * cellWidth() / 9.0) - cellWidth() / 9.0;
    float dd = length(fold) - 0.006 * (0.4 + u_lcStress);
    if (abs(p.y) < CELLT * 1.5 && dd < d) { d = dd; mat = 4.0; }
  }

  /* ---- light paths through the cell: straight above, bent inside ---- */
  if (u_lcShowRays > 0) {
    vec3 dir = normalize(vec3(cos(radians(u_lightAz + 90.0)), -0.55, sin(radians(u_lightAz + 90.0))));
    /* inside the cell the ray refracts toward the normal by Snell — the bend is
       exaggerated because a real 5 µm cell bends almost nothing */
    vec3 dirIn = normalize(mix(dir, vec3(dir.x * 0.72, -1.0, dir.z * 0.72), 0.35));
    for (int k = 0; k < 3; k++) {
      float off = (float(k) - 1.0) * 0.2;
      vec2 perp = normalize(vec2(dir.z, -dir.x));
      vec3 baseXZ = vec3(perp.x * off, 0.0, perp.y * off);
      vec3 a = baseXZ + dir * (CELLT + 1.1) + vec3(0.0, CELLT * 0.5, 0.0);
      vec3 b = baseXZ - dir * 0.1 + vec3(0.0, CELLT * 0.5, 0.0);
      vec3 c = baseXZ + dirIn * CELLT - vec3(0.0, CELLT * 0.5, 0.0);
      vec3 e = baseXZ + dir * 1.1 - vec3(0.0, CELLT * 0.5, 0.0);
      float dr = min(min(sdSegment(p, a, b, 0.004), sdSegment(p, b, c, 0.004)), sdSegment(p, c, e, 0.004));
      if (dr < d) { d = dr; mat = 3.0; }
    }
  }
  return d * u_stepScale * 0.9 + 0.0;
}

vec3 calcRodsColour(vec3 p, vec3 n) {
  vec3 u = lcDirWorld(p);
  float az = atan(u.z, u.x);
  float tilt = asin(clamp(abs(u.y), 0.0, 1.0));
  /* colour = azimuth of the director, brightness = how far it has tilted */
  vec3 c = hsv2rgb(vec3(fract(az / TAU + 0.5), 0.62, 1.0));
  return mix(vec3(0.35), c, 0.35 + 0.65 * (0.35 + 0.65 * tilt / (PI * 0.5)));
}

vec3 shadeScene(Hit h, vec3 rd, vec3 ro) {
  vec3 mat = vec3(h.mat);
  vec3 n = calcNormal(h.p, h.mat);
  vec3 L = lampDir(h.p, rd);
  vec3 V = -rd;
  float gain = lampGain(h.p, uTime);
  float ao = calcAO(h.p, n, h.mat);
  vec3 albedo = vec3(0.6);
  bool emissive = false;
  if (mat.x < 1.5) albedo = calcRodsColour(h.p, n) * 1.15;
  else if (mat.x < 2.5) albedo = vec3(0.85, 0.86, 0.9);
  else if (mat.x < 3.5) { albedo = u_lightCol * 2.2; emissive = true; }
  else if (mat.x < 4.5) albedo = mix(vec3(0.2, 0.35, 0.9), vec3(0.9, 0.35, 0.9), clamp(u_volt, 0.0, 2.0) * 0.5);
  float ndl = clamp(dot(n, L), 0.0, 1.0);
  vec3 col;
  if (emissive) {
    col = albedo * (1.1 + 0.5 * uPulse);
  } else {
    float sh = softShadow(h.p, L, 0.01, 3.0, shadowSharpness(), h.mat);
    vec3 H = normalize(L + V);
    float sp = pow(max(dot(n, H), 0.0), u_shin) * u_spec;
    float fres = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 4.0) * u_fresnel;
    col = albedo * (u_lightCol * gain * ndl * mix(1.0, sh, u_shadow)
                  + envHemisphere(n) * u_envInt * ao * 1.4);
    col += u_lightCol * gain * sp * mix(1.0, sh, 0.6);
    col += mix(u_sky, vec3(1.0), 0.3) * fres * u_rim * 0.5;
  }
  return col;
}

vec4 render(vec2 fragCoord) {
  vec3 ro = cameraOrigin();
  vec3 rd = cameraRay(fragCoord);
  Hit h = marchRay(ro, rd, 40.0);
  vec3 col;
  float aux = 0.5;
  if (h.ok) {
    /* aux: how far the director has tilted from the surface (0 = flat, 1 = up) */
    aux = clamp(asin(clamp(abs(lcDirWorld(h.p).y), 0.0, 1.0)) / (PI * 0.5), 0.0, 1.0);
    col = shadeScene(h, rd, ro);
    float haze = 1.0 - exp(-h.t * 0.06);
    col = mix(col, u_ground * 0.25 * u_envInt, haze * 0.5);
  } else {
    col = mix(u_ground * 0.35, u_sky * 0.5, clamp(rd.y * 2.0 + 0.5, 0.0, 1.0)) * u_envInt;
  }

  if (u_lcShowSurf > 0) {
    /* glass, ITO and the polariser sheets, composited analytically */
    for (int i = 0; i < 4; i++) {
      float side = (i < 2) ? 1.0 : -1.0;
      float layer = (i == 0 || i == 2) ? 0.0 : 1.0;
      float y0 = side * (CELLT * 0.5 + 0.012 + layer * 0.02);
      float y1 = y0 + side * (layer > 0.5 ? 0.012 : 0.035);
      float t0 = (y0 - ro.y) / (abs(rd.y) < 1e-4 ? 1e-4 : rd.y);
      float t1 = (y1 - ro.y) / (abs(rd.y) < 1e-4 ? 1e-4 : rd.y);
      float ta = min(t0, t1), tb = max(t0, t1);
      vec3 hp = ro + rd * max(ta, 1e-3);
      if (tb > 0.0 && ta > 0.0 && abs(hp.x) < cellWidth() && abs(hp.z) < cellWidth()) {
        vec3 n = vec3(0.0, side, 0.0);
        float fres = 0.04 + 0.5 * pow(1.0 - clamp(abs(dot(n, rd)), 0.0, 1.0), 5.0);
        vec3 tint = (layer > 0.5) ? vec3(0.30, 0.34, 0.42) : vec3(0.86, 0.90, 0.95);
        if (layer > 0.5) {
          /* polariser: show the transmission axis as stripes, the way a diagram would */
          vec2 q = ro.xz + rd.xz * (ta + (tb - ta) * 0.5);
          float ang = radians(i < 2 ? u_polP : u_polA);
          vec2 ax = vec2(cos(ang), sin(ang));
          float stripe = 0.5 + 0.5 * cos(dot(q, vec2(-ax.y, ax.x)) * 900.0 + uTime * 0.6);
          tint = mix(vec3(0.05, 0.06, 0.08), vec3(0.55, 0.62, 0.75), stripe);
        }
        float a = clamp(0.18 + fres, 0.0, 1.0);
        col = mix(col, tint * (u_lightInt * 0.6 + u_envInt), a);
      }
    }
  }
  return vec4(col, aux);
}
