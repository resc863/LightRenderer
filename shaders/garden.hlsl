struct Object { float4 centerKind; float4 size; float4 color; float4 rotation; };
StructuredBuffer<Object> Objects : register(t0);
#ifdef HARDWARE_DXR
RaytracingAccelerationStructure World : register(t1);
#endif
StructuredBuffer<uint> Font : register(t2);
StructuredBuffer<float4> Hud : register(t3);
RWTexture2D<float4> Beauty : register(u0);
RWTexture2D<float4> Geometry : register(u1);
RWTexture2D<float4> Output : register(u2);
cbuffer Frame : register(b0) {
    float4 CameraTime;
    float4 RightWidth;
    float4 UpHeight;
    float4 ForwardCount;
    float4 SunExposure;
    float4 Settings; // outline, shadows, reflections, color bands
    float4 Status; // hud line count, animation paused, hardware, help
    float4x4 ViewProjection;
    float4 StreamSettings; // ultimate enabled, streaming feedback, physics HUD, fluid HUD
};
float3 rotate(float3 v, float4 q) { return v + 2 * cross(q.xyz, cross(q.xyz, v) + q.w * v); }
struct Hit { float t; int id; float3 n; };

Hit intersectObject(float3 origin, float3 direction, uint id, float maxT) {
    Object o = Objects[id];
    float4 inv = float4(-o.rotation.xyz, o.rotation.w);
    float3 p = rotate(origin - o.centerKind.xyz, inv);
    float3 d = rotate(direction, inv);
    float3 s = o.size.xyz;
    Hit h; h.t = maxT; h.id = -1; h.n = 0;
    float t; float3 n;
    if (o.centerKind.w < 0.5) {
        float3 ps = p / s, ds = d / s;
        float a = dot(ds, ds), b = dot(ps, ds), c = dot(ps, ps) - 1;
        float disc = b*b - a*c;
        if (disc < 0) return h;
        t = (-b - sqrt(disc)) / a;
        if (t < 0.001) t = (-b + sqrt(disc)) / a;
        n = normalize((p + d*t) / (s*s));
    } else {
        float3 safeD = float3(abs(d.x) < 1e-7 ? 1e-7 : d.x, abs(d.y) < 1e-7 ? 1e-7 : d.y, abs(d.z) < 1e-7 ? 1e-7 : d.z);
        float3 nearT = (-s-p) / safeD, farT = (s-p) / safeD;
        float3 lo = min(nearT, farT), hi = max(nearT, farT);
        float enter = max(lo.x, max(lo.y,lo.z)), exitT = min(hi.x, min(hi.y,hi.z));
        if (enter > exitT || exitT < 0.001) return h;
        t = enter > 0.001 ? enter : exitT;
        float3 face = abs((p + d*t) / s);
        n = face.x > face.y && face.x > face.z ? float3(sign(p.x+d.x*t),0,0) :
            (face.y > face.z ? float3(0,sign(p.y+d.y*t),0) : float3(0,0,sign(p.z+d.z*t)));
    }
    if (t >= 0.001 && t < maxT) { h.t = t; h.id = id; h.n = rotate(n, o.rotation); }
    return h;
}

Hit trace(float3 p, float3 d, float maxT) {
    Hit best; best.t = maxT; best.id = -1; best.n = 0;
#ifdef HARDWARE_DXR
    RayDesc ray; ray.Origin = p; ray.Direction = d; ray.TMin = 0.001; ray.TMax = maxT;
    RayQuery<RAY_FLAG_NONE> query;
    query.TraceRayInline(World, RAY_FLAG_NONE, 255, ray);
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_PROCEDURAL_PRIMITIVE) {
            Hit h = intersectObject(p, d, query.CandidatePrimitiveIndex(), query.CommittedRayT());
            if (h.id >= 0) query.CommitProceduralPrimitiveHit(h.t);
        }
    }
    if (query.CommittedStatus() == COMMITTED_PROCEDURAL_PRIMITIVE_HIT)
        best = intersectObject(p, d, query.CommittedPrimitiveIndex(), maxT);
#else
    for (uint i = 0; i < (uint)ForwardCount.w; i++) {
        Hit h = intersectObject(p, d, i, best.t);
        if (h.id >= 0) best = h;
    }
#endif
    return best;
}

float3 sky(float3 d) {
    float3 c = lerp(float3(0.47,0.76,0.88), float3(0.09,0.38,0.72), saturate(d.y * 1.5));
    float sun = pow(saturate(dot(d, normalize(SunExposure.xyz))), 650);
    c += sun * float3(1.0,0.79,0.50);
    // Hand-painted cloud shapes in directional space.
    float cloud = sin(d.x*16 + d.z*8) + sin(d.x*27-d.z*13) * 0.4;
    float band = smoothstep(0.14,0.21,d.y) * (1-smoothstep(0.24,0.36,d.y));
    c = lerp(c, float3(0.98,0.98,0.93), smoothstep(0.3,0.8,cloud) * band * 0.8);
    return c;
}
float3 litSurface(Hit h, float3 p, float3 d, bool secondary) {
    Object o = Objects[h.id];
    float3 base = pow(o.color.xyz,2.2);
    float3 n = h.n;
    if (h.id == 0) {
        float waves = sin(p.x*2.2 + CameraTime.w) * sin(p.z*1.6-CameraTime.w*0.8);
        n = normalize(n + float3(waves*0.045,0,cos(p.x*1.3+p.z*2+CameraTime.w)*0.045));
        base += pow(saturate(sin(p.x*3+p.z*5+CameraTime.w)),18)*0.035;
    }
    float3 l = normalize(SunExposure.xyz);
    float diffuse = saturate(dot(n,l));
    float shade = 1;
    if (Settings.y > 0.5 && !secondary) {
        Hit blocker = trace(p+n*0.015, l, 50);
        shade = blocker.id < 0 ? 1 : 0.18;
    }
    float bands = Settings.w;
    float cel = floor(diffuse*shade*bands) / max(1, bands-1);
    cel = saturate(cel);
    float3 color = base * lerp(float3(0.38,0.40,0.56),float3(1.04,0.98,0.84),cel);
    float rim = pow(1-saturate(dot(n,-d)), 3) * smoothstep(-0.1,0.5,dot(n,l));
    color += base * rim * 0.20;
    float spec = pow(saturate(dot(reflect(-l,n),-d)),45);
    color += step(0.55,spec) * shade * max(0,o.color.w) * 0.55;
    if (o.color.w < 0) color = base * 1.25;
    return color;
}
float3 surface(Hit h, float3 p, float3 d) {
    float3 color = litSurface(h,p,d,false);
    Object o = Objects[h.id];
    float3 n = h.n;
    if (h.id == 0) n = normalize(n + float3(sin(p.x*2.2+CameraTime.w)*sin(p.z*1.6-CameraTime.w*0.8)*0.045,0,cos(p.x*1.3+p.z*2+CameraTime.w)*0.045));
    if (Settings.z > 0.5 && o.color.w > 0.04) {
        float3 rd = reflect(d,n);
        if (h.id == 0) rd.y = max(rd.y,0.025);
        rd = normalize(rd);
        Hit bounce = trace(p+n*0.016,rd,70);
        float3 reflected = bounce.id < 0 ? sky(rd) : litSurface(bounce,p+n*0.016+rd*bounce.t,rd,true);
        float fresnel = pow(1-saturate(dot(n,-d)),5);
        color = lerp(color,reflected,saturate(o.color.w+fresnel*0.25));
    }
    return color;
}

[numthreads(8,8,1)]
void Trace(uint3 tid : SV_DispatchThreadID) {
    uint2 pixel = tid.xy;
    uint width,height;
    Beauty.GetDimensions(width,height);
    if (pixel.x >= width || pixel.y >= height) return;
    float2 uv = (float2(pixel)+0.5) / float2(width,height)*2-1;
    float3 d = normalize(ForwardCount.xyz + uv.x*RightWidth.xyz - uv.y*UpHeight.xyz);
    Hit h = trace(CameraTime.xyz,d,150);
    float3 color = sky(d);
    if (h.id >= 0) {
        float3 p = CameraTime.xyz+d*h.t;
        color = surface(h,p,d);
        float fog = 1-exp(-h.t*0.0015);
        color = lerp(color,float3(0.75,0.87,0.85),fog);
    }
    Beauty[pixel] = float4(color,1);
    Geometry[pixel] = float4(h.n,h.id >= 0 ? h.t : 0);
}

float roundedBox(float2 p,float2 center,float2 extent,float r) {
    float2 q = abs(p-center)-extent+r;
    return length(max(q,0))+min(max(q.x,q.y),0)-r;
}

[numthreads(8,8,1)]
void Background(uint3 tid:SV_DispatchThreadID) {
    uint w,h;Beauty.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    float2 uv=(float2(tid.xy)+0.5)/float2(w,h)*2-1;
    Beauty[tid.xy]=float4(sky(normalize(ForwardCount.xyz+uv.x*RightWidth.xyz-uv.y*UpHeight.xyz)),1);
    Geometry[tid.xy]=0;
}

[numthreads(8,8,1)]
void Compose(uint3 tid : SV_DispatchThreadID) {
    int2 p = tid.xy;
    int2 size = int2(RightWidth.w,UpHeight.w);
    if (any(p >= size)) return;
    uint tw,th; Beauty.GetDimensions(tw,th);
    int samples = (int)tw/size.x;
    float3 color = 0;
    for (int sy=0;sy<samples;sy++) for (int sx=0;sx<samples;sx++) {
        int2 sp=p*samples+int2(sx,sy);
        float4 g = Geometry[sp];
        float edge = 0;
        if (Settings.x > 0.5 && g.w > 0) {
            float4 a = Geometry[min(sp+int2(1,0),int2(tw,th)-1)];
            float4 b = Geometry[min(sp+int2(0,1),int2(tw,th)-1)];
            float depth = max(abs(g.w-a.w),abs(g.w-b.w)) / max(1,g.w);
            float normal = max(length(g.xyz-a.xyz),length(g.xyz-b.xyz));
            edge = max(smoothstep(0.012,0.020,depth),smoothstep(0.48,0.65,normal))*0.88;
        }
        float3 c = Beauty[sp].xyz * SunExposure.w;
        c = c / (0.65+c) * 1.1;
        c = pow(saturate(c),1.0/2.2);
        color += lerp(c,float3(0.075,0.086,0.15),edge);
    }
    color /= samples*samples;
    float2 uv = float2(p)/size;
    color *= 1 - 0.15 * pow(length(uv-0.5),2);
    // Quiet frosted panels with a coral accent, typeset from a Segoe UI font atlas.
    bool physics = StreamSettings.z > 0.5;
    bool extended = StreamSettings.x > 0.5 || physics;
    float topWidth=min((float)size.x-36,extended?960:456);
    float topBottom=StreamSettings.w>0.5?184:(physics?160:(extended?134:116));
    bool top = p.y < topBottom+9 && p.x < topWidth+34;
    bool bottom = Status.w > 0.5 && p.y > size.y-128 && p.x < 750;
    if (top || bottom) {
        float y = top ? (18+topBottom)*0.5 : size.y-69;
        float w = top ? topWidth : 716;
        float halfHeight=top?(topBottom-18)*0.5:49;
        if (roundedBox(p,float2(w*0.5+18,y),float2(w*0.5,halfHeight),12)<0)
            color = lerp(color,float3(0.065,0.090,0.145),0.89);
        if (p.x >= 18 && p.x <= 21 && abs(p.y-y)<34) color = float3(0.97,0.49,0.55);
        for (uint row = 0; row < (uint)Status.x; row++) {
            float4 layout = Hud[row*2];
            float4 meta = Hud[row*2+1];
            float2 pos = (float2(p)-layout.xy)/layout.z;
            int ch = (int)floor(pos.x/20);
            if (pos.y >= 0 && pos.y < 32 && ch >= 0 && ch < (int)layout.w) {
                uint code = (uint)Hud[(uint)meta.x+ch].x;
                int2 cell = int2((int)pos.x % 20, (int)pos.y);
                uint alpha = Font[cell.y*1920+(code-32)*20+cell.x];
                color = lerp(color,meta.yzw,alpha/255.0);
            }
        }
    }
    // Center aiming dot only in free camera mode (encoded by paused status sign).
    if (Status.y < 0 && length(float2(p)-size*0.5)<2) color = 0.95;
    Output[p] = float4(saturate(color),1);
}
