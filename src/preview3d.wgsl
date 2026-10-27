struct Uniforms {
  mvp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var skin_texture: texture_2d<f32>;
@group(0) @binding(2) var skin_sampler: sampler;

struct VertexInput {
  @location(0) position: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
};

struct VertexOutput {
  @builtin(position) clip_position: vec4<f32>,
  @location(0) normal: vec3<f32>,
  @location(1) uv: vec2<f32>,
};

@vertex
fn vs(input: VertexInput) -> VertexOutput {
  var output: VertexOutput;
  output.clip_position = uniforms.mvp * vec4<f32>(input.position, 1.0);
  output.normal = input.normal;
  output.uv = input.uv;
  return output;
}

@fragment
fn fs(input: VertexOutput) -> @location(0) vec4<f32> {
  let color = textureSample(skin_texture, skin_sampler, input.uv);
  if (color.a < 0.05) {
    discard;
  }
  // 皮肤纹理以 Unorm 采样，这里的 color 就是 sRGB 原值；Slint 显示时
  // 不再做编码，所以直接输出即可保持原图颜色。
  return color;
}
