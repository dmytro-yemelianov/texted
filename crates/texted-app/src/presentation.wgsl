@group(0) @binding(0) var framebuffer: texture_2d<f32>;
// Curvature, scanline depth, vignette and glow (CrtEffect::params); all zero draws the picture flat.
@group(0) @binding(1) var<uniform> crt: vec4<f32>;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    let p = array<vec2<f32>,3>(vec2(-1.0,1.0),vec2(3.0,1.0),vec2(-1.0,-3.0));
    var out: Vertex;
    out.position=vec4(p[index],0.0,1.0);
    out.uv=(p[index]*vec2(1.0,-1.0)+vec2(1.0))*0.5;
    return out;
}
fn texel(size: vec2<u32>, uv: vec2<f32>) -> vec4<f32> {
    let at=min(vec2<u32>(clamp(uv,vec2(0.0),vec2(1.0))*vec2<f32>(size)),size-vec2<u32>(1));
    return textureLoad(framebuffer,vec2<i32>(at),0);
}
@fragment fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let size=textureDimensions(framebuffer);
    if all(crt==vec4(0.0)) {
        return texel(size,in.uv);
    }
    // The same bend as CrtEffect::bend, so clicks land where the curved picture shows them.
    let c=in.uv*2.0-1.0;
    let bent=vec2(c.x*(1.0+crt.x*c.y*c.y),c.y*(1.0+crt.x*c.x*c.x));
    let uv=bent*0.5+0.5;
    if any(uv<vec2(0.0)) || any(uv>=vec2(1.0)) {
        return vec4(0.0,0.0,0.0,1.0);
    }
    let base=texel(size,uv);
    let step=1.0/vec2<f32>(size);
    let glow=(texel(size,uv+vec2(step.x,0.0))+texel(size,uv-vec2(step.x,0.0))
        +texel(size,uv+vec2(0.0,step.y))+texel(size,uv-vec2(0.0,step.y)))*0.25;
    // 0 at the middle of a source row, 1 at its edges.
    let row=abs(fract(uv.y*f32(size.y))-0.5)*2.0;
    let scan=1.0-crt.y*row*row;
    let vignette=clamp(1.0-crt.z*dot(bent,bent)*0.5,0.0,1.0);
    let colour=(base.rgb+glow.rgb*crt.w)*scan*vignette;
    return vec4(min(colour,vec3(1.0)),base.a);
}
