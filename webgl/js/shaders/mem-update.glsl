/* =============================================================================
   mem-update.glsl — the second pass over the substrate memory field.

   Decay and diffusion only. This is the slider that decides whether the ground
   is clay (decay ≈ 0.02/s: an impression lasts minutes of wall-clock time and,
   in the fiction, ten thousand years) or sand (decay ≈ 1.0/s: amnesia before
   the tide even turns).

   Channels:  r = height (0.5 = flat)   g = age   b = water   a = grain angle
   ============================================================================= */

uniform sampler2D uPrev;

void memMain() {
  vec2 uv = gl_FragCoord.xy / uRes.xy;
  vec2 texel = 1.0 / uRes.xy;
  vec4 c = texture(uPrev, uv);

  if (uFrame < 0.5) {
    /* first frame: a flat, wet, unmarked world */
    outColor = vec4(0.5, 0.0, clamp(u_wet, 0.0, 1.0), 0.5);
    return;
  }

  float dt = clamp(uDT, 0.0, 0.1);
  float h = c.r * 2.0 - 1.0;

  /* diffusion: grain creep, bioturbation, rain splash, root metabolism */
  float k = clamp(u_memoryDiff, 0.0, 1.0) * 0.35;
  float hn = 0.0;
  hn += texture(uPrev, uv + vec2(texel.x, 0.0)).r * 2.0 - 1.0;
  hn += texture(uPrev, uv - vec2(texel.x, 0.0)).r * 2.0 - 1.0;
  hn += texture(uPrev, uv + vec2(0.0, texel.y)).r * 2.0 - 1.0;
  hn += texture(uPrev, uv - vec2(0.0, texel.y)).r * 2.0 - 1.0;
  hn *= 0.25;
  h = mix(h, hn, clamp(k * (0.25 + dt * 6.0), 0.0, 0.55));

  /* drift along the grain direction: wind, water, creep */
  float ang = c.a * TAU;
  vec2 dir = vec2(cos(ang), sin(ang));
  float drift = (abs(u_vRate) * 0.5 + u_fWind * 0.8) * dt * 0.02;
  float hp = texture(uPrev, uv - dir * drift).r * 2.0 - 1.0;
  h = mix(h, hp, clamp(drift * 60.0, 0.0, 0.5));

  /* decay: the whole difference between clay and sand */
  float rate = 1.0 - exp(-max(u_decay, 0.0) * dt * 1.6);
  h *= 1.0 - rate;

  /* age rises, and rises faster when the ground forgets quickly */
  float age = clamp(c.g + dt * 0.06 * (1.0 - 0.7 * rate), 0.0, 1.0);
  age *= 1.0 - rate * 0.3;

  /* water arrives as rain, leaves with warmth */
  float wetTarget = clamp(u_wet + u_fRain * 0.35 * (0.5 + 0.5 * sin(uTime * 0.23 + uv.x * 3.0)), 0.0, 1.0);
  float wetRate = 1.0 - exp(-dt * (0.35 + 0.6 * smoothstep(-10.0, 30.0, u_fTemp)));
  float wet = mix(c.b, wetTarget, clamp(wetRate, 0.0, 1.0));

  /* grain direction: flaky particles align, given time and water */
  float want = radians(u_grainDir);
  float a2 = mix(ang, want, clamp(u_aniso * dt * 0.5, 0.0, 1.0));

  outColor = vec4(clamp(h * 0.5 + 0.5, 0.0, 1.0), age, wet, fract(a2 / TAU));
}
