/* =============================================================================
   mode-substrata.glsl — clay, sand, and what each of them is willing to keep.

   A heightfield basin with a strata lamp. The ground carries a memory field
   (uMemory) that traces are stamped into as time passes and that decays at the
   rate you set: this slider is the difference between the first answer and the
   second, made mechanical.
   ============================================================================= */

float slopeBound() { return 2.2 + 2.0 * u_fissure; }

/* --------------------------------------------------------------- material --- */
/* 0 clay 1 bentonite 2 sand 3 sandstone 4 mudcrack 5 shale 6 loess 7 frost
   8 biofilm 9 basalt */
vec2 matParams(float m) {
  /* returns (roughness, specular gain) */
  if (m < 0.5)  return vec2(0.62, 0.55);
  if (m < 1.5)  return vec2(0.52, 0.62);
  if (m < 2.5)  return vec2(0.94, 0.14);
  if (m < 3.5)  return vec2(0.78, 0.28);
  if (m < 4.5)  return vec2(0.70, 0.34);
  if (m < 5.5)  return vec2(0.58, 0.48);
  if (m < 6.5)  return vec2(0.90, 0.18);
  if (m < 7.5)  return vec2(0.68, 0.42);
  if (m < 8.5)  return vec2(0.48, 0.34);
  return vec2(0.80, 0.24);
}
float matPlasticity(float m) {   /* how much of an impression survives */
  if (m < 0.5)  return 1.00;
  if (m < 1.5)  return 1.25;
  if (m < 2.5)  return 0.10;
  if (m < 3.5)  return 0.45;
  if (m < 4.5)  return 0.85;
  if (m < 5.5)  return 0.55;
  if (m < 6.5)  return 0.12;
  if (m < 7.5)  return 0.30;
  if (m < 8.5)  return 0.70;
  return 0.25;
}

/* ------------------------------------------------------------------ relief -- */
float heightAt(vec2 p, out float mat) {
  float m = float(u_mat);
  mat = m;
  float time = uTime;
  vec2 q = p * u_vScale;
  vec2 drift = vec2(0.0, u_vRate * time * 0.35) * u_turb;
  /* domain warp: what turns fog into landform, and landform into clay */
  vec2 w = vec2(
    fbm(q * 0.7 + drift + 11.3, 3, 0.5, 2.0, 0.0),
    fbm(q * 0.7 + drift + 27.7, 3, 0.5, 2.0, 0.0));
  q += w * u_vWarp + vec2(u_vSwirl * time * 0.05, 0.0);
  float h = fbm(q + drift, int(u_vOct), u_vGain, u_vLac, u_vRidge);
  h = h * 0.5 + 0.5;
  h = pow(max(h, 0.0), u_vGamma);

  /* laminae: each stratum is a separate weather, laid down on its own day */
  float lam = h * float(u_terraces) + p.x * u_laminaTilt + p.y * u_laminaTilt * 0.4;
  float lf = fract(lam), li = floor(lam);
  float step_ = mix(lf, smoothstep(0.30, 0.72, lf), 0.75);
  float strata = (li + step_) / max(float(u_terraces), 1.0);
  float strataWob = fbm(p * 3.1 + vec2(0.0, li * 1.7), 2, 0.5, 2.0, 0.0) * u_laminaWob;
  h = mix(h, strata + strataWob * 0.12, 0.55);

  /* desiccation and frost shatter: the ground losing its argument with water */
  float crackS = max(u_fissureScale, 0.2);
  vec2 cq = p * crackS + fbm(p * 1.3, 2, 0.5, 2.0, 0.0) * 0.6;
  vec2 f = worley(cq);
  float edge = f.y - f.x;
  float crack = 1.0 - smoothstep(0.0, u_fissureSharp / 60.0, edge);
  h -= crack * u_fissure * 0.09;
  /* a second, coarser joint set, rotated: real rock fails in families */
  vec2 f2 = worley(mat2(0.54, -0.84, 0.84, 0.54) * cq * 0.47 + 5.0);
  h -= (1.0 - smoothstep(0.0, 0.045, f2.y - f2.x)) * u_fissure * 0.05;

  /* granularity: sand is grains, clay is sheets */
  float grain = fbm(p * (0.35 * u_mGrainScale), 3, 0.55, 2.4, 1.0) * 0.5 + 0.5;
  float grainGain = (m > 1.5 && m < 2.5) ? 1.6 : ((m > 6.5) ? 0.7 : 0.35);
  h += (grain - 0.5) * 0.035 * u_mGrain * grainGain;

  /* the memory field: imprints, and how much of them is left */
  if (u_memoryOn > 0) {
    Mem mm = memSample(p * 0.1667 + 0.5);
    h += mm.h * u_imprint * 0.22 * u_traceMix * mix(1.0, 0.35, mm.age);
    h += (mm.wet - u_wet) * 0.01;
  }
  /* overall bowl so the basin reads as a basin */
  h -= dot(p, p) * 0.012;
  return h * u_scale;
}

/* ---------------------------------------------------------------- shading --- */
vec3 substrateAlbedo(vec2 p, float m, float hgt, float crack, float grain) {
  vec3 a = u_vTint, b = u_vTint2;
  float k = clamp(hgt * 1.2 + 0.5, 0.0, 1.0);
  if (m < 1.5) k = clamp(hgt * 1.6 + 0.35, 0.0, 1.0);         /* clay: pale where dry */
  else if (m < 2.5) k = clamp(grain * 1.3, 0.0, 1.0);         /* sand: grain decides */
  else if (m < 3.5) k = clamp(crack * 0.8 + hgt * 0.6, 0.0, 1.0);
  else if (m < 4.5) k = clamp(crack, 0.0, 1.0);
  else if (m < 5.5) k = clamp(hgt * 0.9 + crack * 0.4, 0.0, 1.0);
  else if (m < 7.5) k = clamp(grain * 0.7 + hgt * 0.5, 0.0, 1.0);
  vec3 c = mix(a, b, clamp(k * u_vMix, 0.0, 1.0));
  /* buried time: older beds read darker and tighter, as they should */
  c *= 1.0 - 0.22 * clamp(u_age / 200000.0, 0.0, 1.0);
  c *= 0.65 + 0.35 * (0.5 + 0.5 * fbm(p * 24.0, 2, 0.5, 2.0, 0.0));
  c = mix(c, c * 0.72, clamp(u_wet * 0.8, 0.0, 0.9));          /* water darkens */
  return satAdjust(c, u_vSat);
}

vec3 skyColor(vec3 rd) {
  float t = clamp(rd.y * 0.5 + 0.5, 0.0, 1.0);
  vec3 sky = mix(u_ground * 0.6, u_sky, smoothstep(0.35, 1.0, t));
  float sun = max(dot(rd, normalize(vec3(cos(radians(u_lightAz)) * cos(radians(u_lightEl)),
                                        sin(radians(u_lightEl)),
                                        sin(radians(u_lightAz)) * cos(radians(u_lightEl))))), 0.0);
  sky += u_lightCol * pow(sun, 24.0) * 1.6;
  sky += u_lightCol * pow(sun, 3.0) * 0.18;
  return sky * u_envInt * 1.6;
}

vec3 shadeSurface(Hit hit, vec3 rd, vec3 ro, vec2 p) {
  float m = hit.mat;
  vec2 rp = matParams(m);
  float mm;
  vec3 n = calcNormal(hit.p, mm);
  float ao = calcAO(hit.p, n, m);
  float wet = clamp(u_wet, 0.0, 1.0);

  float crackS = max(u_fissureScale, 0.2);
  vec2 f = worley(hit.p.xz * crackS + fbm(hit.p.xz * 1.3, 2, 0.5, 2.0, 0.0) * 0.6);
  float crack = 1.0 - smoothstep(0.0, u_fissureSharp / 60.0, f.y - f.x);
  float grain = fbm(hit.p.xz * (0.35 * u_mGrainScale), 3, 0.55, 2.4, 1.0) * 0.5 + 0.5;

  vec3 albedo = substrateAlbedo(hit.p.xz, m, hit.p.y, crack, grain);
  /* fresh traces read brighter, old ones sink into the body colour */
  if (u_memoryOn > 0) {
    Mem mf = memSample(hit.p.xz * 0.1667 + 0.5);
    albedo = mix(albedo, albedo * 1.35 + 0.05, clamp(abs(mf.h) * 1.2, 0.0, 0.6));
    albedo *= 1.0 - 0.25 * clamp(mf.wet, 0.0, 1.0);
  }

  vec3 L = lampDir(hit.p, rd);
  vec3 L2 = normalize(vec3(cos(radians(u_light2Az)) * cos(radians(u_light2El)),
                           sin(radians(u_light2El)),
                           sin(radians(u_light2Az)) * cos(radians(u_light2El))));
  vec3 V = -rd;
  float gain = lampGain(hit.p, uTime);
  float sh = softShadow(hit.p, L, 0.02, 6.0, shadowSharpness(), m);
  sh = mix(1.0, sh, u_shadow);
  float ndl = clamp(dot(n, L), 0.0, 1.0);
  float ndl2 = clamp(dot(n, L2), 0.0, 1.0);
  float rimMode = (u_lightMode == 3) ? 1.6 : 1.0;
  vec3 dif = albedo * u_lightCol * gain * ndl * sh;
  dif += albedo * u_light2Col * u_light2Int * ndl2 * (0.6 + 0.4 * ao);
  /* hemisphere ambient, rotatable */
  vec3 amb = envHemisphere(n) * u_envInt * ao;
  /* specular: the wet sheen that makes clay look like something biological */
  float rough = clamp(rp.x * (1.0 - 0.55 * wet) + u_pore * 0.5, 0.02, 1.0);
  float shin = u_shin * (1.0 - 0.5 * wet) + 2.0;
  vec3 H = normalize(L + V);
  float sp = pow(max(dot(n, H), 0.0), shin) * u_spec * rp.y * (1.0 - 0.4 * rough) * (0.35 + 0.65 * wet);
  vec3 spec = u_lightCol * gain * sp * sh;
  float fres = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 5.0) * u_fresnel;
  vec3 rim = envHemisphere(n) * fres * u_rim * 0.6 * rimMode;
  /* translucent skin: light that went in, wandered, and came back */
  float trans = pow(clamp(dot(-L, V), 0.0, 1.0), 3.0) * u_sss * (0.4 + 0.6 * ao);
  vec3 sss = albedo * u_lightCol * gain * trans * 0.4;
  return dif + amb + spec + rim + sss;
}

/* ------------------------------------------------------------------ render -- */
vec4 render(vec2 fragCoord) {
  vec3 ro = cameraOrigin();
  vec3 rd = cameraRay(fragCoord);
  float tmax = 60.0;
  Hit hit = marchRay(ro, rd, tmax);
  vec3 col;
  float aux = 0.5;
  if (hit.ok) {
    col = shadeSurface(hit, rd, ro, hit.p.xz);
    /* distance haze */
    float haze = 1.0 - exp(-hit.t * 0.035);
    col = mix(col, skyColor(rd) * 0.9, haze * 0.75);
    /* aux: what the ground still remembers here, 0.5 = nothing at all */
    aux = clamp(memSample(hit.p.xz * 0.1667 + 0.5).h * 0.5 + 0.5, 0.0, 1.0);
  } else {
    col = skyColor(rd);
  }
  return vec4(col, aux);
}
