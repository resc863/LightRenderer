use std::mem::size_of;
use windows::Win32::Graphics::Direct3D12::*;

// OPTIONS22 is from the shipping Agility SDK 1.619.6 header, not in windows-rs 0.62.
#[repr(C)]
#[derive(Default)]
struct Options22 {
    actually_reorders: i32,
    byte_offset_views: i32,
    max_dispatch: u32,
    max_mesh_dispatch: u32,
}

#[derive(Clone, Debug)]
pub struct Capabilities {
    pub shader_model: u32,
    pub raytracing_tier: i32,
    pub mesh_shader_tier: i32,
    pub sampler_feedback_tier: i32,
    pub vrs_tier: i32,
    pub vrs_tile: u32,
    pub tiled_resources_tier: i32,
    pub ser_actually_reorders: bool,
}
impl Capabilities {
    pub fn query(device: &ID3D12Device5) -> Self {
        fn check<T>(d: &ID3D12Device5, feature: D3D12_FEATURE, value: &mut T) -> bool {
            unsafe {
                d.CheckFeatureSupport(feature, (value as *mut T).cast(), size_of::<T>() as u32)
                    .is_ok()
            }
        }
        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL::default();
        for version in (0x60..=0x6a).rev() {
            sm.HighestShaderModel = D3D_SHADER_MODEL(version);
            if check(device, D3D12_FEATURE_SHADER_MODEL, &mut sm) {
                break;
            }
        }
        let mut rt = D3D12_FEATURE_DATA_D3D12_OPTIONS5::default();
        check(device, D3D12_FEATURE_D3D12_OPTIONS5, &mut rt);
        let mut mesh = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        check(device, D3D12_FEATURE_D3D12_OPTIONS7, &mut mesh);
        let mut vrs = D3D12_FEATURE_DATA_D3D12_OPTIONS6::default();
        check(device, D3D12_FEATURE_D3D12_OPTIONS6, &mut vrs);
        let mut tiled = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        check(device, D3D12_FEATURE_D3D12_OPTIONS, &mut tiled);
        let mut ser = Options22::default();
        check(device, D3D12_FEATURE(65), &mut ser);
        Self {
            shader_model: sm.HighestShaderModel.0 as u32,
            raytracing_tier: rt.RaytracingTier.0,
            mesh_shader_tier: mesh.MeshShaderTier.0,
            sampler_feedback_tier: mesh.SamplerFeedbackTier.0,
            vrs_tier: vrs.VariableShadingRateTier.0,
            vrs_tile: vrs.ShadingRateImageTileSize,
            tiled_resources_tier: tiled.TiledResourcesTier.0,
            ser_actually_reorders: ser.actually_reorders != 0,
        }
    }
    pub fn ultimate(&self) -> bool {
        self.raytracing_tier >= 11
            && self.mesh_shader_tier >= 10
            && self.sampler_feedback_tier >= 90
            && self.vrs_tier >= 2
            && self.tiled_resources_tier >= 2
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"shader_model":format!("{}.{}",self.shader_model>>4,self.shader_model&15),
            "dxr_tier":self.raytracing_tier,"mesh_shader_tier":self.mesh_shader_tier,"sampler_feedback_tier":self.sampler_feedback_tier,
            "vrs_tier":self.vrs_tier,"vrs_tile_size":self.vrs_tile,"tiled_resources_tier":self.tiled_resources_tier,"dx12_ultimate":self.ultimate(),
            "opacity_micromaps_supported":self.raytracing_tier>=12,"ser_shader_supported":self.shader_model>=0x69 && self.raytracing_tier>0,
            "ser_actually_reorders":self.ser_actually_reorders,"omm_ser_integrated":false})
    }
}
