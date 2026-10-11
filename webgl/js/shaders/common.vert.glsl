#version 300 es
/* One fullscreen triangle, no vertex buffer needed beyond three void-attribute draws. */
void main() {
  // gl_VertexID 0,1,2 -> (-1,-1), (3,-1), (-1,3)
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
