/* =============================================================================
   mode-causal.glsl — the shape of the argument, drawn as a field.

   Not a raymarch: this is a stack of planes seen at an angle, each carrying a
   scalar field, each informed by the plane below it. The pattern the whole page
   keeps rediscovering lives here as geometry — a substrate, a slow mechanism,
   an abrupt result — and the pulse climbs the stack at whatever speed you set.

   "Quantify" collapses the continuous field into discrete levels and shows the
   error bars. Mechanism and number, one slider apart.
   ============================================================================= */

float layerField(vec2 p, float layerIdx, float t) {
  float li = layerIdx;
  vec2 q = p * (0.6 + 0.4 * u_fieldScale) + vec2(0.0, li * 0.37);
  /* curl-ish vortices: causes swirl around each other */
  vec2 c = vec2(gnoise(q * 1.7 + t * 0.3), gnoise(q * 1.7 + 13.1 - t * 0.3)) * u_vortGain;
  q += c * 1.4;
  float f = fbm(q + vec2(0.0, -t * u_flowSpeed * 0.5), int(u_vOct), u_vGain, u_vLac, u_vRidge);
  f = f * 0.5 + 0.5;
  /* the mechanism: a slow accumulation that saturates */
  f = 1.0 - exp(-f * (0.6 + 1.6 * u_flowGain));
  /* the hinge: sharpen the transition, which is what makes a result abrupt */
  float g = smoothstep(0.32, 0.62, f);
  return mix(f, g, clamp(u_edgeGain * 0.35, 0.0, 0.9));
}

/* a plane in the stack: intersect the ray with y = h and return the sample */
float planeT(vec3 ro, vec3 rd, float h) { return (h - ro.y) / (abs(rd.y) < 1e-4 ? 1e-4 : rd.y); }

vec4 render(vec2 fragCoord) {
  vec3 ro = uCamO;
  vec3 rd = cameraRay(fragCoord);
  int layers = clamp(u_layer, 1, 12);
  float gap = max(u_layerGap, 0.01);
  float t = u_flowSpeed * uTime;
  float stackH = gap * float(layers);

  /* the pulse climbs the stack: one slow mechanism, arriving at the top late */
  float hPulse = fract(uTime * u_flowSpeed * 0.45 / max(stackH, 0.05)) * stackH;

  vec3 col = vec3(0.0);
  float accEdge = 0.0;
  float prev = 0.0;
  float trans = 1.0;
  float aux = 0.0;

  /* bottom layer first: it is the substrate, and everything above it is
     informed by it */
  for (int li = 0; li < 12; li++) {
    if (li >= layers) break;
    float h = float(li) * gap;
    float tp = planeT(ro, rd, h);
    if (tp <= 0.05) continue;
    vec3 p = ro + rd * tp;
    if (abs(p.x) > 3.4 * u_scale || abs(p.z) > 3.4 * u_scale) continue;

    float f0 = layerField(p.xz, float(li), t);
    float f = (li > 0) ? mix(f0, prev, clamp(u_blurIn, 0.0, 1.0)) : f0;

    /* the hinge: where a small difference decides the outcome */
    float e = abs(f - layerField(p.xz + vec2(0.06, 0.0), float(li), t))
            + abs(f - layerField(p.xz + vec2(0.0, 0.06), float(li), t));
    e *= 6.0 * (1.0 + u_edgeGain);
    accEdge = max(accEdge, e * exp(-0.3 * float(li)));
    prev = f;

    /* quantify: discrete levels, or the continuous mechanism */
    float qf = f;
    if (u_ci > 0.001) {
      float levels = mix(3.0, 48.0, clamp(u_ci, 0.0, 1.0));
      qf = mix(f, floor(f * levels) / levels, clamp(u_ci, 0.0, 1.0));
    }
    aux = qf;
    float src = (u_palSource == 5) ? clamp(e * 0.4, 0.0, 1.0) : qf;
    vec3 c = paletteAt(u_palette, src);
    c *= 0.35 + 0.9 * qf;

    float pulse = exp(-pow((hPulse - h) / max(gap * 0.6, 0.02), 2.0));
    c += u_lightCol * pulse * 0.6;

    /* the plane's own frame: infrastructure is visible infrastructure */
    vec2 a = abs(p.xz);
    float frame = smoothstep(3.42 * u_scale, 3.40 * u_scale, max(a.x, a.y));
    c += vec3(0.20, 0.30, 0.42) * frame * (0.4 + 0.6 * trans);

    float dim = exp(-0.20 * float(li) * (1.0 + u_flowDecay * 2.0));
    float alpha = clamp(u_report * 0.62 + 0.05, 0.02, 1.0) * dim;
    col += c * alpha * trans;
    trans *= mix(1.0, 0.82, dim) * (1.0 - clamp(u_flowDiff, 0.0, 1.0) * 0.28);
  }

  col += mix(u_lightCol, vec3(0.6, 0.8, 1.0), 0.5) * accEdge * 0.3;

  /* a floor and a faint grid, so the stack has somewhere to stand */
  float tg = planeT(ro, rd, -gap);
  if (tg > 0.05) {
    vec2 g = (ro + rd * tg).xz * 0.5;
    float grid = smoothstep(0.98, 1.0, max(abs(fract(g.x) - 0.5), abs(fract(g.y) - 0.5)) * 2.0);
    col += grid * vec3(0.05, 0.08, 0.12) * exp(-tg * 0.1);
  }
  col += vec3(0.012, 0.016, 0.024);
  return vec4(col, clamp(aux, 0.0, 1.0));
}
