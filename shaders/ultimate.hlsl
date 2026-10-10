#define HARDWARE_DXR 1
#define ULTIMATE 1
#include "garden.hlsl"
struct Patch { uint object;uint face;uint x;uint y;uint columns;uint rows;uint width;uint height; };
StructuredBuffer<Patch> Patches : register(t4);
Texture2D<float4> VirtualTexture : register(t5);
Texture2D<uint> ResidentMip : register(t6);
FeedbackTexture2D<SAMPLER_FEEDBACK_MIN_MIP> Feedback : register(u3);
SamplerState SurfaceSampler : register(s0);
struct Vertex { float4 position:SV_Position;float3 world:POSITION0;float3 normal:NORMAL0;nointerpolation uint object:OBJECT0; };

[outputtopology("triangle")]
[numthreads(64,1,1)]
void Mesh(uint group:SV_GroupID,uint thread:SV_GroupThreadID,out vertices Vertex verts[64],out indices uint3 tris[98]) {
    Patch patch=Patches[group];Object o=Objects[patch.object];
    uint vertices=(patch.width+1)*(patch.height+1);uint triangles=patch.width*patch.height*2;
    SetMeshOutputCounts(vertices,triangles);
    if (thread<vertices) {
        float2 uv=float2(patch.x+thread%(patch.width+1),patch.y+thread/(patch.width+1))/float2(patch.columns,patch.rows);
        float3 local,normal;
        if (patch.face==6) {
            float a=uv.x*6.28318530718,b=uv.y*3.14159265359;
            normal=float3(sin(b)*cos(a),cos(b),sin(b)*sin(a));
            local=normal*o.size.xyz;normal=normalize(normal/o.size.xyz);
        } else {
            float2 q=uv*2-1;
            if(patch.face<2) {local=float3(patch.face==0?-1:1,q.x,q.y);normal=float3(local.x,0,0);}
            else if(patch.face<4) {local=float3(q.x,patch.face==2?-1:1,q.y);normal=float3(0,local.y,0);}
            else {local=float3(q.x,q.y,patch.face==4?-1:1);normal=float3(0,0,local.z);}
            local*=o.size.xyz;
        }
        float3 world=o.centerKind.xyz+rotate(local,o.rotation);
        Vertex v;v.position=mul(ViewProjection,float4(world,1));v.world=world;v.normal=rotate(normal,o.rotation);v.object=patch.object;
        verts[thread]=v;
    }
    for(uint index=thread;index<triangles;index+=64) {
        uint quad=index/2;uint base=(quad/patch.width)*(patch.width+1)+quad%patch.width;
        tris[index]=index%2==0?uint3(base,base+1,base+patch.width+1):uint3(base+1,base+patch.width+2,base+patch.width+1);
    }
}
float2 textureUv(Vertex v) {
    Object o=Objects[v.object];float3 local=rotate(v.world-o.centerKind.xyz,float4(-o.rotation.xyz,o.rotation.w))/o.size.xyz;
    float2 uv;
    if(o.centerKind.w<0.5) uv=float2(atan2(local.z,local.x)/6.2831853+0.5,acos(clamp(local.y,-1,1))/3.14159265);
    else {float3 n=abs(v.normal);uv=n.y>0.5?local.xz:n.x>0.5?local.zy:local.xy;uv=uv*0.5+0.5;}
    uint zone=v.object%8;
    return (saturate(uv)*float2(0.248,0.498)+float2((zone%4)*0.25,(zone/4)*0.5)+0.001);
}
struct Pixel {float4 color:SV_Target0;float4 geometry:SV_Target1;};
[earlydepthstencil]
Pixel Shade(Vertex v) {
    Hit h;h.id=v.object;h.t=length(v.world-CameraTime.xyz);h.n=normalize(v.normal);
    float3 d=normalize(v.world-CameraTime.xyz);
    float3 color=surface(h,v.world,d);
    float2 uv=textureUv(v);
    float desired=clamp(VirtualTexture.CalculateLevelOfDetail(SurfaceSampler,uv),0,14);
    if(StreamSettings.y>0.5) Feedback.WriteSamplerFeedbackLevel(VirtualTexture,SurfaceSampler,uv,desired);
    // Each bit describes an actually mapped mip at this UV. A finer resident
    // page never implies that intermediate, coarser pages are resident.
    uint residentMask=ResidentMip.Load(int3(min((uint2)(uv*128),127),0));
    uint sampledMip=(uint)round(desired);
    while(sampledMip<14 && (residentMask & (1u<<sampledMip))==0) sampledMip++;
    float4 tex=VirtualTexture.SampleLevel(SurfaceSampler,uv,(float)sampledMip);
    if(v.object>0 && Objects[v.object].color.w>=0) color*=lerp(1.0,tex.rgb,0.22);
    color=lerp(color,float3(0.75,0.87,0.85),1-exp(-h.t*0.0015));
    Pixel p;p.color=float4(color,1);p.geometry=float4(h.n,h.t);return p;
}
