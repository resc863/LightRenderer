use crate::{
    detail::DetailPlan,
    renderer::{Upload, committed, transition},
    streaming::Streaming,
};
use anyhow::{Context, Result};
use std::{ffi::c_void, mem::size_of};
use windows::{
    Win32::Graphics::{Direct3D12::*, Dxgi::Common::*},
    core::Interface,
};

#[repr(C, align(8))]
struct Part<T> {
    kind: D3D12_PIPELINE_STATE_SUBOBJECT_TYPE,
    value: T,
}
impl<T> Part<T> {
    fn new(kind: D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, value: T) -> Self {
        Self { kind, value }
    }
}
#[repr(C)]
struct MeshStream {
    root: Part<*mut c_void>,
    mesh: Part<D3D12_SHADER_BYTECODE>,
    pixel: Part<D3D12_SHADER_BYTECODE>,
    blend: Part<D3D12_BLEND_DESC>,
    raster: Part<D3D12_RASTERIZER_DESC>,
    depth: Part<D3D12_DEPTH_STENCIL_DESC>,
    rtv: Part<D3D12_RT_FORMAT_ARRAY>,
    dsv: Part<DXGI_FORMAT>,
    samples: Part<DXGI_SAMPLE_DESC>,
    mask: Part<u32>,
    topology: Part<D3D12_PRIMITIVE_TOPOLOGY_TYPE>,
}
fn mesh_pipeline(
    device: &ID3D12Device5,
    root: &ID3D12RootSignature,
    sm69: bool,
) -> Result<ID3D12PipelineState> {
    let (mesh, pixel): (&[u8], &[u8]) = if sm69 {
        (
            include_bytes!(concat!(env!("OUT_DIR"), "/mesh_6_9.dxil")),
            include_bytes!(concat!(env!("OUT_DIR"), "/shade_6_9.dxil")),
        )
    } else {
        (
            include_bytes!(concat!(env!("OUT_DIR"), "/mesh_6_5.dxil")),
            include_bytes!(concat!(env!("OUT_DIR"), "/shade_6_5.dxil")),
        )
    };
    let code = |b: &[u8]| D3D12_SHADER_BYTECODE {
        pShaderBytecode: b.as_ptr().cast(),
        BytecodeLength: b.len(),
    };
    let rt = D3D12_RENDER_TARGET_BLEND_DESC {
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: 15,
        ..Default::default()
    };
    let stream = MeshStream {
        root: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE,
            root.as_raw(),
        ),
        mesh: Part::new(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS, code(mesh)),
        pixel: Part::new(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS, code(pixel)),
        blend: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND,
            D3D12_BLEND_DESC {
                RenderTarget: [rt; 8],
                ..Default::default()
            },
        ),
        raster: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER,
            D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            },
        ),
        depth: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL,
            D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_LESS,
                FrontFace: D3D12_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D12_STENCIL_OP_KEEP,
                    StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
                    StencilPassOp: D3D12_STENCIL_OP_KEEP,
                    StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                },
                BackFace: D3D12_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D12_STENCIL_OP_KEEP,
                    StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
                    StencilPassOp: D3D12_STENCIL_OP_KEEP,
                    StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                },
                ..Default::default()
            },
        ),
        rtv: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS,
            D3D12_RT_FORMAT_ARRAY {
                RTFormats: [
                    DXGI_FORMAT_R16G16B16A16_FLOAT,
                    DXGI_FORMAT_R32G32B32A32_FLOAT,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_FORMAT_UNKNOWN,
                ],
                NumRenderTargets: 2,
            },
        ),
        dsv: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL_FORMAT,
            DXGI_FORMAT_D32_FLOAT,
        ),
        samples: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_DESC,
            DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
        ),
        mask: Part::new(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_MASK, u32::MAX),
        topology: Part::new(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PRIMITIVE_TOPOLOGY,
            D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        ),
    };
    unsafe {
        Ok(device.cast::<ID3D12Device2>()?.CreatePipelineState(
            &D3D12_PIPELINE_STATE_STREAM_DESC {
                SizeInBytes: size_of::<MeshStream>(),
                pPipelineStateSubobjectStream: (&stream as *const MeshStream).cast_mut().cast(),
            },
        )?)
    }
}
pub struct Ultimate {
    pub pipeline: ID3D12PipelineState,
    pub patches: Upload,
    pub streaming: Streaming,
    pub enabled: bool,
    pub vrs_enabled: bool,
    pub detail_error: f32,
    pub patch_count: u32,
    pub triangles: u64,
    pub min_triangles: u64,
    pub max_triangles: u64,
    pub frames: u64,
    pub vrs_frames: u64,
    pub shader_model: &'static str,
    rtv: ID3D12DescriptorHeap,
    dsv: ID3D12DescriptorHeap,
    depth: Option<ID3D12Resource>,
    rate: Option<ID3D12Resource>,
    rate_upload: Option<Upload>,
    rate_footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    rate_initialized: bool,
    width: u32,
    height: u32,
    tile: u32,
}
impl Ultimate {
    pub fn new(
        device: &ID3D12Device5,
        root: &ID3D12RootSignature,
        heap: &ID3D12DescriptorHeap,
        tile: u32,
        sm69: bool,
        budget: u32,
    ) -> Result<Self> {
        unsafe {
            Ok(Self {
                pipeline: mesh_pipeline(device, root, sm69).context("Mesh Shader pipeline")?,
                patches: Upload::new(device, 32 * 65535)?,
                streaming: Streaming::new(device, heap, budget)?,
                enabled: true,
                vrs_enabled: true,
                detail_error: 0.65,
                patch_count: 0,
                triangles: 0,
                min_triangles: u64::MAX,
                max_triangles: 0,
                frames: 0,
                vrs_frames: 0,
                shader_model: if sm69 { "6.9" } else { "6.5" },
                rtv: device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: 2,
                    ..Default::default()
                })?,
                dsv: device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                    NumDescriptors: 1,
                    ..Default::default()
                })?,
                depth: None,
                rate: None,
                rate_upload: None,
                rate_footprint: Default::default(),
                rate_initialized: false,
                width: 0,
                height: 0,
                tile,
            })
        }
    }
    pub fn prepare(&mut self, plan: DetailPlan) -> Result<()> {
        anyhow::ensure!(
            plan.patches.len() <= 65535,
            "Mesh dispatch capacity exceeded"
        );
        self.patches.write(bytemuck::cast_slice(&plan.patches))?;
        self.patch_count = plan.patches.len() as u32;
        self.triangles = plan.triangles;
        self.min_triangles = self.min_triangles.min(plan.triangles);
        self.max_triangles = self.max_triangles.max(plan.triangles);
        Ok(())
    }
    pub fn resize(
        &mut self,
        device: &ID3D12Device5,
        images: &[ID3D12Resource],
        w: u32,
        h: u32,
    ) -> Result<()> {
        unsafe {
            self.width = w;
            self.height = h;
            let mut d = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: w as u64,
                Height: h,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_D32_FLOAT,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
                ..Default::default()
            };
            let clear = D3D12_CLEAR_VALUE {
                Format: DXGI_FORMAT_D32_FLOAT,
                Anonymous: D3D12_CLEAR_VALUE_0 {
                    DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                        Depth: 1.,
                        Stencil: 0,
                    },
                },
            };
            let mut depth = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    CreationNodeMask: 1,
                    VisibleNodeMask: 1,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &d,
                D3D12_RESOURCE_STATE_DEPTH_WRITE,
                Some(&clear),
                &mut depth,
            )?;
            self.depth = depth;
            device.CreateDepthStencilView(
                self.depth.as_ref(),
                None,
                self.dsv.GetCPUDescriptorHandleForHeapStart(),
            );
            let start = self.rtv.GetCPUDescriptorHandleForHeapStart().ptr;
            let stride =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
            for (i, image) in images.iter().take(2).enumerate() {
                device.CreateRenderTargetView(
                    image,
                    None,
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: start + i * stride,
                    },
                );
            }
            d.Width = w.div_ceil(self.tile) as u64;
            d.Height = h.div_ceil(self.tile);
            d.Format = DXGI_FORMAT_R8_UINT;
            d.Flags = D3D12_RESOURCE_FLAG_NONE;
            let rate = committed(
                device,
                &d,
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?;
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut bytes = 0;
            device.GetCopyableFootprints(
                &d,
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                Some(&mut bytes),
            );
            let upload = Upload::new(device, bytes as usize)?;
            let mut data = vec![0u8; bytes as usize];
            for y in 0..d.Height {
                for x in 0..d.Width as u32 {
                    let dx = (x as f32 / d.Width as f32 - 0.5).abs();
                    let dy = (y as f32 / d.Height as f32 - 0.5).abs();
                    let rate = if dx < 0.25 && dy < 0.34 {
                        D3D12_SHADING_RATE_1X1
                    } else if dx < 0.36 && dy < 0.42 {
                        D3D12_SHADING_RATE_1X2
                    } else {
                        D3D12_SHADING_RATE_2X2
                    };
                    data[(y * footprint.Footprint.RowPitch + x) as usize] = rate.0 as u8;
                }
            }
            upload.write(&data)?;
            self.rate = Some(rate);
            self.rate_upload = Some(upload);
            self.rate_footprint = footprint;
            self.rate_initialized = false;
            Ok(())
        }
    }
    pub fn draw(
        &mut self,
        list: &ID3D12GraphicsCommandList4,
        images: &[ID3D12Resource],
    ) -> Result<()> {
        unsafe {
            let list6: ID3D12GraphicsCommandList6 = list.cast()?;
            if !self.rate_initialized {
                crate::streaming::copy_upload(
                    list,
                    self.rate.as_ref().unwrap(),
                    0,
                    &self.rate_upload.as_ref().unwrap().resource,
                    self.rate_footprint,
                );
                transition(
                    list,
                    self.rate.as_ref().unwrap(),
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_SHADING_RATE_SOURCE,
                );
                self.rate_initialized = true;
            }
            for image in images.iter().take(2) {
                transition(
                    list,
                    image,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
            }
            let first = self.rtv.GetCPUDescriptorHandleForHeapStart();
            let depth = self.dsv.GetCPUDescriptorHandleForHeapStart();
            list.OMSetRenderTargets(2, Some(&first), true, Some(&depth));
            list.ClearDepthStencilView(depth, D3D12_CLEAR_FLAG_DEPTH, 1., 0, None);
            list.RSSetViewports(&[D3D12_VIEWPORT {
                Width: self.width as f32,
                Height: self.height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]);
            list.RSSetScissorRects(&[windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: self.width as i32,
                bottom: self.height as i32,
            }]);
            let combiners = [
                D3D12_SHADING_RATE_COMBINER_PASSTHROUGH,
                D3D12_SHADING_RATE_COMBINER_MAX,
            ];
            list6.RSSetShadingRate(D3D12_SHADING_RATE_1X1, Some(combiners.as_ptr()));
            list6.RSSetShadingRateImage(if self.vrs_enabled {
                self.rate.as_ref()
            } else {
                None
            });
            list.SetPipelineState(&self.pipeline);
            list6.DispatchMesh(self.patch_count, 1, 1);
            list6.RSSetShadingRateImage(None);
            for image in images.iter().take(2) {
                transition(
                    list,
                    image,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                );
            }
            self.frames += 1;
            if self.vrs_enabled {
                self.vrs_frames += 1;
            }
            Ok(())
        }
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"enabled":self.enabled,"shader_model":self.shader_model,"mesh_shader_frames":self.frames,"vrs_frames":self.vrs_frames,
        "vrs_enabled":self.vrs_enabled,"screen_error_pixels":self.detail_error,"mesh_patches":self.patch_count,"triangles":self.triangles,"min_triangles":self.min_triangles,"max_triangles":self.max_triangles,
        "geometry_source":"screen-error-driven parametric patches generated in Mesh Shader; no prebuilt LOD levels","texture_streaming":self.streaming.json()})
    }
}
