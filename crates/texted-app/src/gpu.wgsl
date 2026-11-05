struct Viewport { size: vec2<f32>, padding: vec2<f32> }
@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}
@vertex fn vs_main(
    @builtin(vertex_index) vertex: u32,
    @location(0) rect: vec4<f32>,
    @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> VertexOut {
    let corners = array<vec2<f32>, 6>(vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(0.,1.),vec2(1.,0.),vec2(1.,1.));
    let corner = corners[vertex];
    let pixel = rect.xy + corner * rect.zw;
    var out: VertexOut;
    out.position = vec4(pixel.x / viewport.size.x * 2. - 1., 1. - pixel.y / viewport.size.y * 2., 0., 1.);
    out.uv = mix(uv_rect.xy, uv_rect.zw, corner);
    out.color = color;
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, atlas_sampler, in.uv).r;
    return vec4(in.color.rgb, in.color.a * coverage);
}
