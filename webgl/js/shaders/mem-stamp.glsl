/* =============================================================================
   mem-stamp.glsl — paint one trace into the substrate memory field.

   Drawn as a rotated quad in field space, additive, with the fragment shader
   deciding the cross-section: a root presses, a shell merely rests, a foot
   loads in two places, a fracture keeps going in a direction it did not choose.
   Which of those it is changes how much of the ground's memory the trace takes.
   ============================================================================= */

in vec2 vLocal;
uniform vec3 uStampPos;        /* xy = centre in field uv, z = rotation */
uniform vec2 uStampSize;
uniform int uStampKind;
uniform float uStampDepth;
uniform float uStampSeed;

float taper(float x) { return smoothstep(-1.0, -0.35, x) * smoothstep(1.0, 0.35, x); }

void stampMain() {
/*  vec2 p = vLocal;
  if (p.x * p.x + p.y * p.y > 1.9) discard;
  float seed = uStampSeed;
  float groove = 0.0;
  float wetAdd = 0.0;
  int kind = uStampKind;

  if (kind == 0) {
    // a root: long, tapered, with the two ridges that roots always leave 
    float w = 0.34 * (1.0 - 0.5 * abs(p.x));
    groove = exp(-pow(p.y / max(w, 1e-3), 2.0)) * taper(p.x);
    groove += 0.22 * exp(-pow((abs(p.y) - 0.62) / 0.24, 2.0)) * taper(p.x);
  } else if (kind == 1) {
    // a shell: an object resting on the surface rather than pressing into it 
    float d = length(p / vec2(0.62, 0.95));
    groove = (d < 1.0 ? 0.25 : 0.0) + 0.35 * exp(-pow((d - 1.0) / 0.14, 2.0));
    groove *= 0.4;
  } else if (kind == 2) {
    // a foot: heel and ball load, the toes barely register 
    float heel = exp(-pow(length(p - vec2(-0.35, 0.0)) / 0.34, 2.0));
    float ball = exp(-pow(length(p - vec2(0.22, 0.0)) / 0.30, 2.0));
    groove = max(heel, ball) * 0.85;
    for (int i = 0; i < 5; i++) {
      float fi = float(i);
      vec2 c = vec2(0.62, (fi - 2.0) * 0.24 + 0.05 * sin(fi * 2.0 + seed));
      groove += exp(-pow(length(p - c) / 0.11, 2.0)) * 0.55;
    }
  } else if (kind == 3) {
    // a fracture: jagged, thin, and it continues past the frame 
    float zig = 0.18 * sin(p.x * 6.0 + seed * 5.0) + 0.08 * sin(p.x * 17.0 + seed * 2.0);
    groove = exp(-pow((p.y - zig) / 0.075, 2.0)) * taper(p.x * 1.3);
    wetAdd = groove * 0.35;
  } else if (kind == 4) {
    // a rootless feeding trace, branching under the surface 
    float trunk = exp(-pow(p.y / 0.16, 2.0)) * taper(p.x);
    float branch = exp(-pow((p.y - 0.3 * sin(p.x * 4.0 + seed * 4.0)) / 0.09, 2.0)) * smoothstep(0.15, 0.9, abs(p.x));
    groove = trunk + branch * 0.7;
  } else {
    // a needle: one hailstone, one grain arriving fast 
    groove = exp(-pow(p.y / 0.05, 2.0)) * taper(p.x * 1.2);
  }

  float depth = uStampDepth * max(u_traceMix, 0.05);
  float h = -groove * depth * 0.5;         // pressed in, so negative 
  wetAdd = max(wetAdd, groove * 0.15);
  // r accumulates the height delta (0.5-encoded), g the freshness, b the water 
  outColor = vec4(h * 0.5, groove * 0.8, wetAdd, 0.0);
  */
  outColor = vec4(1.0,0.0,0.0,1.0);
}
