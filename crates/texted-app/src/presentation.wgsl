// The ShaderWallpaper CRT pass (Sources/ShaderWallpaper/Shaders.metal, fs_main), ported to WGSL.
// Coordinates are window pixels inside the presentation viewport. `pixel` takes the place of the
// wallpaper's integer scale: one font pixel in window pixels.
@group(0) @binding(0) var framebuffer: texture_2d<f32>;
struct Effects {
    // brightness, curvature, chromatic aberration, afterglow
    a: vec4<f32>,
    // noise, flicker, scanlines, time in seconds
    b: vec4<f32>,
    // font pixel size, enabled, animated, unused
    c: vec4<f32>,
    // viewport width and height in window pixels, unused, unused
    d: vec4<f32>,
}
@group(0) @binding(1) var<uniform> fx: Effects;
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
fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p,vec2(12.9898,78.233)))*43758.5453);
}
fn pillow(coord: vec2<f32>, res: vec2<f32>, magnitude: f32) -> vec2<f32> {
    var dc=coord/res*2.0-1.0;
    dc*=1.0+magnitude*dot(dc,dc);
    return (dc+1.0)*0.5*res;
}
// Outside the viewport is black glass, which the wallpaper leaves transparent.
fn sample_at(size: vec2<u32>, coord: vec2<f32>, res: vec2<f32>) -> vec4<f32> {
    if any(coord<vec2(0.0)) || any(coord>=res) {
        return vec4(0.0,0.0,0.0,0.0);
    }
    return texel(size,coord/res);
}
@fragment fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let size=textureDimensions(framebuffer);
    if fx.c.y==0.0 {
        return texel(size,in.uv);
    }
    let brightness=fx.a.x;
    let distortion=fx.a.y;
    let aberration=fx.a.z;
    let afterglow=fx.a.w;
    let noise_amount=fx.b.x;
    let flicker_amount=fx.b.y;
    let scanlines=fx.b.z;
    let t=fx.b.w;
    let scale=max(fx.c.x,1.0);
    let animated=fx.c.z!=0.0;
    let res=max(fx.d.xy,vec2(1.0));
    let frag=in.uv*res;

    let drift1=sin(t*0.12)*0.5+sin(t*0.07)*0.25;
    let drift2=cos(t*0.09)*0.5+sin(t*0.15)*0.25;
    var line_jitter=0.0;
    if animated && hash(vec2(t,floor(frag.y*0.5)))>0.97 {
        line_jitter=(hash(vec2(t,frag.y))-0.5)*scale*3.5;
    }
    let jittered=frag+vec2(line_jitter,0.0);
    let pillow_mag=distortion*(1.0+0.4*drift2);
    let base_coord=pillow(jittered,res,pillow_mag);
    let base=sample_at(size,base_coord,res);
    if base.a<=0.0 {
        return vec4(0.0,0.0,0.0,1.0);
    }
    var rgb=base.rgb;
    let ca=aberration*scale;
    if ca>0.001 {
        let red=sample_at(size,pillow(jittered+vec2(-ca,0.0),res,pillow_mag),res);
        let blue=sample_at(size,pillow(jittered+vec2(ca,0.0),res,pillow_mag),res);
        rgb=vec3(red.r,base.g,blue.b);
    }
    if afterglow>0.001 {
        let step=scale*(1.0+afterglow*2.0);
        let glow=(sample_at(size,base_coord+vec2(step,0.0),res).rgb
            +sample_at(size,base_coord-vec2(step,0.0),res).rgb
            +sample_at(size,base_coord+vec2(0.0,step),res).rgb
            +sample_at(size,base_coord-vec2(0.0,step),res).rgb)*0.25;
        rgb+=glow*(0.35*afterglow);
    }
    rgb*=vec3(1.02+0.02*drift1,1.0,0.98-0.02*drift1);
    let speckle=hash(vec2(frag.x*0.001,frag.y+t));
    if speckle>0.98 {
        rgb+=vec3(0.35*noise_amount*hash(frag+t));
    }
    let flicker_mag=flicker_amount*(0.05+0.02*drift1);
    let flicker=1.0+flicker_mag*sin(t*314.159);
    let hum_mag=flicker_amount*(0.04+0.02*drift2);
    let hum=hum_mag*sin(in.uv.y*8.0-t*2.0);
    rgb*=flicker+hum;
    let scan_depth=mix(1.0,0.8+0.1*drift1,scanlines);
    if i32(frag.y/1.5)%2==0 {
        rgb*=scan_depth;
    }
    rgb*=brightness;
    return vec4(clamp(rgb,vec3(0.0),vec3(1.0)),base.a);
}
