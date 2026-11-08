@group(0) @binding(0) var framebuffer: texture_2d<f32>;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    let p = array<vec2<f32>,3>(vec2(-1.0,1.0),vec2(3.0,1.0),vec2(-1.0,-3.0));
    var out: Vertex;
    out.position=vec4(p[index],0.0,1.0);
    out.uv=(p[index]*vec2(1.0,-1.0)+vec2(1.0))*0.5;
    return out;
}
@fragment fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let size=textureDimensions(framebuffer);
    let texel=min(vec2<u32>(in.uv*vec2<f32>(size)),size-vec2<u32>(1));
    return textureLoad(framebuffer,vec2<i32>(texel),0);
}
