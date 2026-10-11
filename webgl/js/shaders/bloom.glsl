/* =============================================================================
   bloom.glsl — bright pass and separable blur, in two small programs.
   Coherent light scattering in the eye, or in a cell: either way it is the
   only reason a bright fringe reads as bright.
   ============================================================================= */

uniform sampler2D uSrc;

vec3 brightPassMain() {
  vec2 uv = gl_FragCoord.xy / uRes.xy;
  vec2 t = 1.0 / uRes.xy;
  vec3 c = texture(uSrc, uv).rgb * 0.4;
  c += texture(uSrc, uv + vec2(t.x, t.y)).rgb * 0.15;
  c += texture(uSrc, uv + vec2(-t.x, t.y)).rgb * 0.15;
  c += texture(uSrc, uv + vec2(t.x, -t.y)).rgb * 0.15;
  c += texture(uSrc, uv + vec2(-t.x, -t.y)).rgb * 0.15;
  float l = luma(c);
  float thr = max(u_bloomThresh, 0.0);
  float w = max(l - thr, 0.0) / max(1.0 - thr, 1e-3);
  return c * w;
}

uniform vec2 uBlurDir;      /* texel-space direction, already scaled by radius */

vec3 blurMain() {
  vec2 uv = gl_FragCoord.xy / uRes.xy;
  vec3 sum = vec3(0.0);
  float wsum = 0.0;
  for (int i = -8; i <= 8; i++) {
    float fi = float(i);
    float w = exp(-fi * fi / 24.0);
    sum += texture(uSrc, uv + uBlurDir * fi).rgb * w;
    wsum += w;
  }
  return sum / max(wsum, EPS);
}
