/* =============================================================================
   mode-weathering.glsl — rock coming apart, slowly.

   Six processes, one terrain. The unifying object is the reaction front: the
   weathered rind that eats inward from every fracture, thickening with water,
   heat and time. Hydrolysis is the quiet one — feldspar plus water plus carbon
   dioxide gives kaolinite plus dissolved ions, and no drama at all.

   "Geological time" here is the time base you selected in the Time tab, so you
   can run eleven thousand years of periglacial creep in six seconds.
   ============================================================================= */

float slopeBound() { return 2.6 + 2.4 * u_fCrack; }

float fractureField(vec2 p) {
  vec2 cq = p * (0.9 + 0.24 * u_fCrackScale) + fbm(p * 0.7, 2, 0.5, 2.0, 0.0) * 0.8;
  vec2 w = worley(cq);
  return w.y - w.x;   /* 0 on a fracture, larger inside a block */
}

/* how far the rind has eaten into a block: a front driven by water, heat and
   the distance to the nearest fracture */
float rindAmount(vec2 p, float t, float fracDist) {
  int wx = u_wx;
  float water = u_fRain;
  float heat = smoothstep(-6.0, 26.0, u_fTemp);
  float bio = u_fBio;
  float rate = u_fCrack * (0.35 + 0.65 * water) * (0.25 + 0.75 * heat) * (0.6 + 0.8 * bio);
  /* eleven thousand years is not one afternoon: the bed's age sets the clock */
  rate *= 0.35 + 0.75 * clamp(u_age / 40000.0, 0.0, 1.0);
  if (wx == 1) rate *= 1.4 * smoothstep(6.0, -4.0, u_fTemp);        /* freeze–thaw */
  if (wx == 2) rate *= 0.5 + 1.2 * u_fWind;                          /* abrasion */
  if (wx == 3) rate *= 0.6 + 0.9 * u_fSlope;                         /* burial */
  if (wx == 4) rate *= 0.7 + 1.3 * bio;                              /* roots, lichen */
  if (wx == 5) rate *= 0.4 + 1.5 * u_fFlux;                          /* periglacial */
  /* fronts slow as they thicken: the reaction has further to diffuse */
  float drive = rate * t * (1.0 / (1.0 + fracDist * 3.2));
  return 1.0 - exp(-drive * 0.35);
}

float heightAt(vec2 p, out float mat) {
  int wx = u_wx;
  float t = uTime * u_speed * 0.35;
  float fracDist = fractureField(p * 0.6);
  float rind = rindAmount(p, t, fracDist);

  /* the intact rock: ridged, angular, bedded */
  float rock = fbm(p * u_vScale, int(u_vOct), u_vGain, u_vLac, 0.85) * 0.5 + 0.5;
  float lam = rock * float(u_terraces) + p.x * u_laminaTilt * 0.5;
  rock = mix(rock, (floor(lam) + smoothstep(0.35, 0.7, fract(lam))) / float(u_terraces), 0.5);
  float ch = ridged(p * (1.6 * u_vScale), 4, 0.55, 2.2);

  /* fractures: the deepest set is the joint family, the finer one the shatter */
  float crack = 1.0 - smoothstep(0.0, 0.09, fracDist);
  vec2 wf = worley(p * (2.4 * u_fCrackScale));
  float crackFine = 1.0 - smoothstep(0.0, 0.05, wf.y - wf.x);
  rock -= crack * 0.10 + crackFine * 0.045 * (0.5 + u_fCrack);

  /* hydrolysis eats inward: the rind is lower, smoother and paler */
  float smoothRock = fbm(p * u_vScale * 0.4, 3, 0.5, 2.0, 0.15) * 0.5 + 0.5;
  float body = mix(rock, smoothRock * 0.85 + 0.08, clamp(rind * 1.1, 0.0, 1.0));
  body -= rind * 0.06;

  /* process-specific reshaping */
  if (wx == 1) {
    /* frost: wedging along fractures widens them, then the blocks tip */
    body -= crack * 0.06 * smoothstep(8.0, -6.0, u_fTemp);
    body += ch * 0.05 * u_fCrack;
  } else if (wx == 2) {
    /* wind: abrasion planes the windward side and piles the product downwind */
    float w = dot(p, normalize(vec2(sin(radians(u_lightAz)), cos(radians(u_lightAz)))));
    body -= smoothstep(0.0, 2.0, w) * 0.05 * u_fWind;
    body += smoothstep(-0.4, 0.6, -w) * 0.03 * u_fWind;
    body += (fbm(p * 8.0, 3, 0.5, 2.0, 0.4) * 0.5 + 0.5) * 0.02 * u_fWind;
  } else if (wx == 3) {
    /* diagenesis: burial compacts and cements; porosity dies with depth */
    body = mix(body, body * 0.94 + 0.02, clamp(u_fSlope, 0.0, 1.0));
    body -= ch * 0.03 * (1.0 - u_fSlope);
  } else if (wx == 4) {
    /* biology: roots are wedges that also deliver water to where it matters */
    float rooty = ridged(p * 2.2, 3, 0.5, 2.0);
    body -= smoothstep(0.45, 0.9, rooty) * 0.07 * u_fBio;
  } else if (wx == 5) {
    /* periglacial: freeze–thaw sorts stones into circles around fines */
    vec2 cq = p * (0.55 + 1.6 * u_fFlux);
    vec2 w = worley(cq);
    float ring = smoothstep(0.06, 0.0, abs(w.x - 0.44));
    float fine = 1.0 - smoothstep(0.0, 0.42, w.x);
    body += ring * 0.035 * u_fFlux - fine * 0.02 * u_fFlux;
    body += (w.y - w.x) * 0.01 * u_fFlux;
  }

  /* the memory field: what the ground is still holding on to */
  if (u_memoryOn > 0) {
    Mem mm = memSample(p * 0.1667 + 0.5);
    body += mm.h * u_imprint * 0.18 * u_traceMix;
  }
  mat = 1.0 + rind;                 /* 1 = intact rock, 2 = fully weathered rind */
  body -= dot(p, p) * 0.014;
  return body * u_scale;
}

vec3 weatherAlbedo(vec2 p, float mat, float crack, float grain) {
  float wx = u_wx;
  vec3 rock = u_vTint;
  vec3 clay = u_vTint2;
  if (wx == 1) rock = mix(rock, vec3(0.42, 0.45, 0.50), 0.35);      /* frost: bluish grey */
  if (wx == 2) rock = mix(rock, vec3(0.62, 0.52, 0.36), 0.30);      /* dust */
  if (wx == 3) rock = rock * 0.8;
  if (wx == 4) rock = mix(rock, vec3(0.32, 0.36, 0.26), 0.35);      /* lichen */
  if (wx == 5) rock = mix(rock, vec3(0.55, 0.54, 0.52), 0.25);
  float rind = clamp(mat - 1.0, 0.0, 1.0);
  vec3 c = mix(rock, clay, clamp(rind * 1.1, 0.0, 1.0));
  c = mix(c, c * 0.55, clamp(crack * 0.85, 0.0, 1.0));             /* fractures read dark */
  c *= 0.7 + 0.3 * grain;
  return satAdjust(c, u_vSat);
}

vec3 shadeWeather(Hit hit, vec3 rd) {
  float mm;
  vec3 n = calcNormal(hit.p, mm);
  float rind = clamp(hit.mat - 1.0, 0.0, 1.0);
  float fracDist = fractureField(hit.p.xz * 0.6);
  float crack = 1.0 - smoothstep(0.0, 0.09, fracDist);
  float grain = fbm(hit.p.xz * 60.0, 2, 0.5, 2.0, 0.0) * 0.5 + 0.5;
  vec3 albedo = weatherAlbedo(hit.p.xz, hit.mat, crack, grain);
  float ao = calcAO(hit.p, n, hit.mat);

  vec3 L = lampDir(hit.p, rd);
  vec3 L2 = normalize(vec3(cos(radians(u_light2Az)) * cos(radians(u_light2El)),
                           sin(radians(u_light2El)),
                           sin(radians(u_light2Az)) * cos(radians(u_light2El))));
  vec3 V = -rd;
  float gain = lampGain(hit.p, uTime);
  float sh = softShadow(hit.p, L, 0.02, 5.0, shadowSharpness(), hit.mat);
  float ndl = clamp(dot(n, L), 0.0, 1.0);
  float ndl2 = clamp(dot(n, L2), 0.0, 1.0);
  vec3 dif = albedo * (u_lightCol * gain * ndl * mix(1.0, sh, u_shadow)
                     + u_light2Col * u_light2Int * ndl2 * 0.8);
  vec3 amb = albedo * envHemisphere(n) * u_envInt * ao;
  vec3 H = normalize(L + V);
  float sp = pow(max(dot(n, H), 0.0), u_shin) * u_spec * (0.4 + 0.9 * clamp(u_fRain, 0.0, 1.0));
  float fres = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 5.0);
  return dif + amb + u_lightCol * sp * gain * mix(1.0, sh, 0.5) + envHemisphere(n) * fres * u_rim * 0.4;
}

vec4 render(vec2 fragCoord) {
  vec3 ro = cameraOrigin();
  vec3 rd = cameraRay(fragCoord);
  Hit h = marchRay(ro, rd, 70.0);
  vec3 sky = envHemisphere(rd) * u_envInt * 1.7;
  if (!h.ok) return vec4(sky, 0.0);
  vec3 col = shadeWeather(h, rd);
  float haze = 1.0 - exp(-h.t * 0.028);
  /* aux: how far the weathering rind has eaten in at this point */
  return vec4(mix(col, sky * 0.92, haze * 0.7), clamp(h.mat - 1.0, 0.0, 1.0));
}
