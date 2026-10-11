/* A rotated quad in field space, drawn without any vertex buffer:
   gl_VertexID 0..3 -> (-1,-1), (1,-1), (-1,1), (1,1) as a triangle strip. */
uniform vec3 uStampPos;      /* xy = centre in uv, z = rotation (radians) */
uniform vec2 uStampSize;     /* half-extents in uv */
out vec2 vLocal;

void main() {
  float x = (gl_VertexID == 1 || gl_VertexID == 3) ? 1.0 : -1.0;
  float y = (gl_VertexID >= 2) ? 1.0 : -1.0;
  vec2 c = vec2(x, y);
  float a = uStampPos.z;
  mat2 R = mat2(cos(a), -sin(a), sin(a), cos(a));
  vec2 p = R * (c * uStampSize);
  vec2 uv = uStampPos.xy + p;
  gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
  vLocal = c;
}
