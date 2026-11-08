// The window's screen effects, ported to WGSL:
// - Pixel SSH's CRT pass (pixel-ssh crates/render-web/src/lib.rs): curvature, antenna hum, jitter,
//   chromatic aberration, afterglow, scanlines, pixel grid and analog snow, in that order;
// - ditherGL's pixel separation modes, bloom and vignette (crates/render/shaders);
// - ShaderWallpaper's flicker, mains hum and brightness (Sources/ShaderWallpaper/Shaders.metal).
// Coordinates are window pixels inside the presentation viewport. One font pixel (`pixel`, in
// window pixels) plays the part of Pixel SSH's framebuffer pixel and ditherGL's source pixel.
@group(0) @binding(0) var framebuffer: texture_2d<f32>;
struct Effects {
    // brightness, curvature, chromatic aberration, afterglow
    a: vec4<f32>,
    // scanlines, pixel grid, jitter, antenna hum
    b: vec4<f32>,
    // noise, flicker, vignette, bloom
    c: vec4<f32>,
    // separation mode, separation strength, matrix spacing, inner square (0 is off)
    d: vec4<f32>,
    // matrix gap colour, phosphor (0 colour, 1 amber, 2 green, 3 white)
    e: vec4<f32>,
    // time in seconds, font pixel size, enabled, animated
    f: vec4<f32>,
    // viewport width and height in window pixels, unused, unused
    g: vec4<f32>,
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
fn src(size: vec2<u32>, uv: vec2<f32>) -> vec3<f32> {
    return texel(size,uv).rgb;
}
// Dave Hoskins' hash12, as Pixel SSH uses it for decorrelated static.
fn hash12(p: vec2<f32>) -> f32 {
    var p3=fract(vec3(p.x,p.y,p.x)*0.1031);
    p3+=dot(p3,p3.yzx+33.33);
    return fract((p3.x+p3.y)*p3.z);
}
fn luma(c: vec3<f32>) -> f32 {
    return dot(c,vec3(0.299,0.587,0.114));
}
// GLSL's smoothstep(high, low, x) with its edges reversed, which WGSL leaves undefined.
fn falloff(low: f32, high: f32, x: f32) -> f32 {
    return 1.0-smoothstep(low,high,x);
}
fn on_edge(within: vec2<f32>, t: f32) -> f32 {
    return select(0.0,1.0,within.x<t||within.x>1.0-t||within.y<t||within.y>1.0-t);
}
@fragment fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    let size=textureDimensions(framebuffer);
    if fx.f.z==0.0 {
        return texel(size,in.uv);
    }
    let brightness=fx.a.x;
    let curvature=fx.a.y;
    let chromatic=fx.a.z;
    let afterglow=fx.a.w;
    let scanlines=fx.b.x;
    let grid=fx.b.y;
    let jitter=fx.b.z;
    let antenna=fx.b.w;
    let noise=fx.c.x;
    let flicker_amount=fx.c.y;
    let vignette=fx.c.z;
    let bloom=fx.c.w;
    let mode=i32(fx.d.x+0.5);
    let strength=clamp(fx.d.y,0.0,1.0);
    let spacing=fx.d.z;
    let inner=fx.d.w;
    let gap=fx.e.rgb;
    let phosphor=i32(fx.e.w+0.5);
    let mono=phosphor!=0;
    let t=fx.f.x;
    let pixel=max(fx.f.y,1.0);
    // Pixel SSH's u_resolution: the screen measured in font pixels.
    let res=max(fx.g.xy,vec2(1.0))/pixel;
    let one=1.0/res;
    var uv=in.uv;

    // 1. Barrel curvature with flyback anode breathing; beyond the glass is the bezel.
    if curvature>0.001 {
        let breath=1.0+0.0035*(sin(t*1.7)*cos(t*0.43));
        let cc=(uv*2.0-1.0)*breath;
        let offset=cc.yx/3.0;
        uv=(cc+cc*offset*offset*(curvature*0.28)+1.0)*0.5;
        if uv.x<0.0||uv.x>1.0||uv.y<0.0||uv.y>1.0 {
            return vec4(0.02,0.02,0.03,1.0);
        }
    }
    // 2. Antenna hum: a rolling mains ripple and sporadic RF bursts that tear the sync.
    var burst=0.0;
    var hum_wave=0.0;
    if antenna>0.001 {
        let wander=t*0.95+sin(t*0.31)*0.85+cos(t*0.17)*0.45;
        let phase=fract(uv.y*1.5-wander);
        hum_wave=exp(-phase*2.8)*0.70+sin(phase*6.28318)*0.20+sin(phase*12.56636)*0.10;
        let slot=floor(t*1.6);
        burst=step(0.78,hash12(vec2(slot,83.19)))*exp(-fract(t*1.6)*4.5);
        let band=falloff(0.0,0.14,abs(uv.y-hash12(vec2(slot,39.41))));
        let rf=(hash12(vec2(floor(uv.y*res.y),floor(t*60.0)))-0.5)*2.0;
        uv.x=clamp(uv.x+band*burst*(0.016+0.024*rf)*antenna,0.0,1.0);
    }
    // 3. Horizontal sync jitter with a non-repeating wander.
    if jitter>0.001 {
        let wander=1.0+0.35*sin(t*0.58+sin(t*0.23));
        let seed=floor(uv.y*res.y)+floor(t*60.0)*17.31;
        let line=(hash12(vec2(seed,seed*1.414))-0.5)*2.0;
        let wave=sin(uv.y*110.0+t*19.0+sin(t*0.35)*4.0);
        uv.x=clamp(uv.x+(line*0.65+wave*0.35)*(jitter*0.0035*wander),0.0,1.0);
    }
    // 4. Chromatic aberration with thermal drift; monochrome tubes have one gun and no split.
    var color=src(size,uv);
    if !mono&&chromatic>0.001 {
        let shift=vec2(chromatic*0.0035*(1.0+0.18*sin(t*0.28)),0.0);
        color=vec3(src(size,uv+shift).r,color.g,src(size,uv-shift).b);
    }
    // 5. Phosphor afterglow with a drifting persistence.
    if afterglow>0.001 {
        let o=one*1.5;
        let glow=(src(size,uv+vec2(o.x,0.0))+src(size,uv-vec2(o.x,0.0))
            +src(size,uv+vec2(0.0,o.y))+src(size,uv-vec2(0.0,o.y)))*0.25;
        color=mix(color,max(color,glow),afterglow*0.55*(1.0+0.15*sin(t*0.35)));
    }
    // 6. ditherGL's bloom: bright pixels bleed into a near and a wide ring of neighbours.
    if bloom>0.001 {
        var near=vec3(0.0);
        var near_weight=0.0;
        for (var x=-2; x<=2; x++) {
            for (var y=-2; y<=2; y++) {
                let at=vec2(f32(x),f32(y));
                let tap=src(size,uv+at*one);
                let w=exp(-dot(at,at)*0.5);
                near+=tap*smoothstep(0.6,0.8,luma(tap))*w;
                near_weight+=w;
            }
        }
        var wide=vec3(0.0);
        var wide_weight=0.0;
        for (var x=-4; x<=4; x+=2) {
            for (var y=-4; y<=4; y+=2) {
                let at=vec2(f32(x),f32(y));
                let tap=src(size,uv+at*one*1.5);
                let d=at/2.0;
                let w=exp(-dot(d,d)*0.3);
                wide+=tap*smoothstep(0.7,0.9,luma(tap))*w;
                wide_weight+=w;
            }
        }
        let total=near/near_weight*0.7+wide/wide_weight*0.3;
        color+=vec3(total.r*1.1,total.g,total.b*1.05)*bloom;
    }
    // 7. Electron beam scanlines, one beam per font pixel row, kept brighter on bright pixels.
    if scanlines>0.001 {
        let d=abs(fract(uv.y*res.y)-0.5)*2.0;
        let beam=exp(-d*d*3.5);
        color*=mix(1.0,mix(beam*1.08,1.0,luma(color)*0.35),scanlines);
    }
    // 8. Pixel grid between font pixels, then the phosphor mask: RGB aperture-grille triads on
    // colour tubes, a dot pitch on monochrome ones.
    if grid>0.001 {
        let cell=fract(uv*res);
        let edge=min(cell,1.0-cell);
        color*=mix(1.0,smoothstep(0.0,0.14,min(edge.x,edge.y)),grid*0.65);
        if mono {
            let dot_phase=(in.position.x+in.position.y*0.5)*1.5707963;
            color*=mix(1.0,0.82+0.18*sin(dot_phase),grid*0.55);
        } else {
            let triad=in.position.x*2.0943951;
            let mask=vec3(0.5+0.5*sin(triad),0.5+0.5*sin(triad+2.0943951),0.5+0.5*sin(triad+4.1887902));
            color*=mix(vec3(1.0),mask*1.35,grid*0.55);
        }
    }
    // 9. ditherGL's pixel separation, per font pixel of the bent picture. Its matrix sizes are
    // drawn at its default spacing of 0.1; other spacings widen or narrow the gap.
    if mode>0 {
        let within=fract(uv*res);
        let centre=within-0.5;
        let border=strength*1.5/pixel;
        let wider=spacing-0.1;
        var mask=1.0;
        if mode==1 {
            mask=mix(1.0,1.0-strength,on_edge(within,border));
        } else if mode==2 {
            mask=mix(1.0-strength,1.0,falloff(0.35,0.45,length(centre)));
        } else if mode==3 {
            mask=mix(1.0,1.0-strength*0.8,on_edge(within,border));
        } else if mode==4 {
            let outer=clamp(0.4-wider,0.05,0.5);
            if abs(centre.x)>outer||abs(centre.y)>outer {
                color=gap;
            } else if inner>0.0 {
                let core=outer*inner;
                if !(abs(centre.x)<core&&abs(centre.y)<core) {
                    mask=0.7;
                }
            }
        } else if mode==5 {
            let radius=clamp(0.35-wider,0.05,0.5);
            let d=length(centre);
            if d>radius {
                color=gap;
            } else {
                mask=mix(0.3,1.0,falloff(radius*0.8,radius,d));
                if inner>0.0 {
                    mask+=falloff(0.0,radius*inner*0.5,d)*0.5;
                }
            }
        } else {
            let corner=0.15;
            let outer=clamp(0.35-wider,corner+0.01,0.5);
            let a=abs(centre);
            let q=max(a-outer+corner,vec2(0.0));
            let rounded=length(q)+min(max(a.x-outer+corner,a.y-outer+corner),0.0)-corner;
            if rounded>0.0 {
                color=select(vec3(0.0,0.1,0.0),gap,gap.g>0.1);
            } else if inner>0.0 {
                mask=mix(1.0,0.85,max(a.x,a.y)/outer*inner);
            }
        }
        color*=mask;
    }
    // 10. The hum's brightness ripple and the dip of each burst.
    if antenna>0.001 {
        color*=(1.0+(hum_wave*0.35-0.12)*antenna)*(1.0-burst*0.24*antenna);
    }
    // 11. Analog snow with receiver gain drift: flakes, pepper dropouts, grain and spark streaks,
    // in the tube's phosphor colour.
    if noise>0.001 {
        let agc=1.0+0.25*sin(t*0.52+sin(t*0.19))+0.12*cos(t*0.93+cos(t*0.33));
        let amount=clamp(noise*agc,0.0,1.0);
        let raster=vec2(floor(uv.x*res.x*1.25),floor(uv.y*res.y));
        let frame=floor(t*60.0)%10000.0;
        let seed=frame*17.13;
        let clump=0.35+0.18*sin(t*0.37+1.2);
        let fine=hash12(raster+vec2(seed+19.19,seed*1.618+73.73));
        let cluster=hash12(floor(raster*vec2(0.5,1.0))+vec2(seed*2.37+37.11,seed*0.79+91.37));
        let n=mix(fine,cluster,clump);
        let flake=smoothstep(mix(0.96,0.68,amount)+0.025*sin(t*0.71),1.0,fine)*(amount*1.85);
        let drop=falloff(0.0,mix(0.04,0.28,amount),fine)*(amount*1.35);
        color*=1.0-clamp(drop,0.0,0.88);
        let grain=(n-0.5)*(amount*0.35);
        let streak=step(0.993,hash12(vec2(floor(uv.y*res.y*0.5),frame)))
            *hash12(vec2(floor(uv.x*24.0),frame*1.3))*amount*1.3;
        if phosphor==1 {
            color+=vec3(1.0,0.65,0.0)*(flake+streak+grain);
        } else if phosphor==2 {
            color+=vec3(0.0,1.0,0.0)*(flake+streak+grain);
        } else if phosphor==3 {
            color+=vec3(flake+streak+grain);
        } else {
            let tint_hash=hash12(raster+vec2(seed*3.71,41.13));
            let wander=sin(t*0.45);
            let tint=mix(vec3(1.0),vec3(0.85+0.25*tint_hash+0.1*wander,0.95,1.15-0.25*tint_hash-0.1*wander),0.25);
            color+=tint*(flake+streak)+vec3(grain);
        }
        color=clamp(color,vec3(0.0),vec3(1.0));
    }
    // 12. Pixel SSH darkens the corners of a curved tube.
    if curvature>0.001 {
        let v=uv*(1.0-uv.yx);
        color*=clamp(pow(max(v.x*v.y*15.0,0.0),0.15),0.0,1.0);
    }
    // 13. ditherGL's vignette over the flat screen.
    if vignette>0.001 {
        color*=falloff(0.3,0.8,length(in.uv-0.5)*vignette);
    }
    // 14. ShaderWallpaper's flicker and mains hum.
    if flicker_amount>0.001 {
        let drift1=sin(t*0.12)*0.5+sin(t*0.07)*0.25;
        let drift2=cos(t*0.09)*0.5+sin(t*0.15)*0.25;
        let flicker=1.0+flicker_amount*(0.05+0.02*drift1)*sin(t*314.159);
        let hum=flicker_amount*(0.04+0.02*drift2)*sin(in.uv.y*8.0-t*2.0);
        color*=flicker+hum;
    }
    color*=brightness;
    return vec4(clamp(color,vec3(0.0),vec3(1.0)),1.0);
}
