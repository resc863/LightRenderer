use crate::scene::Object;
use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};
use std::{
    ffi::c_void,
    mem::{ManuallyDrop, size_of},
    path::Path,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0},
        Graphics::{
            Direct3D::*,
            Direct3D12::*,
            Dxgi::{Common::*, *},
            Gdi::*,
        },
        System::Threading::{CreateEventW, WaitForSingleObject},
    },
    core::{Interface, PCWSTR, w},
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Frame {
    pub camera_time: [f32; 4],
    pub right_width: [f32; 4],
    pub up_height: [f32; 4],
    pub forward_count: [f32; 4],
    pub sun_exposure: [f32; 4],
    pub settings: [f32; 4],
    pub status: [f32; 4],
    pub view_projection: [[f32; 4]; 4],
    pub stream_settings: [f32; 4],
}

pub(crate) struct Upload {
    pub(crate) resource: ID3D12Resource,
    ptr: *mut u8,
    capacity: usize,
}
impl Upload {
    pub(crate) fn new(device: &ID3D12Device5, capacity: usize) -> Result<Self> {
        let resource = buffer(
            device,
            capacity as u64,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            D3D12_RESOURCE_FLAG_NONE,
        )?;
        let mut ptr = std::ptr::null_mut();
        unsafe {
            resource.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut ptr))?;
        }
        Ok(Self {
            resource,
            ptr: ptr.cast(),
            capacity,
        })
    }
    pub(crate) fn write(&self, data: &[u8]) -> Result<()> {
        anyhow::ensure!(data.len() <= self.capacity, "GPU upload capacity exceeded");
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr, data.len());
        }
        Ok(())
    }
    pub(crate) fn address(&self) -> u64 {
        unsafe { self.resource.GetGPUVirtualAddress() }
    }
}
impl Drop for Upload {
    fn drop(&mut self) {
        unsafe {
            self.resource.Unmap(0, None);
        }
    }
}

struct Acceleration {
    aabbs: Upload,
    instance: Upload,
    blas: ID3D12Resource,
    tlas: ID3D12Resource,
    scratch: ID3D12Resource,
}

pub struct Renderer {
    device: ID3D12Device5,
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList4,
    swap: IDXGISwapChain3,
    buffers: Vec<ID3D12Resource>,
    heap: ID3D12DescriptorHeap,
    root: ID3D12RootSignature,
    trace: ID3D12PipelineState,
    compose: ID3D12PipelineState,
    background: ID3D12PipelineState,
    pub ultimate: Option<crate::ultimate::Ultimate>,
    images: Vec<ID3D12Resource>,
    constants: Upload,
    objects: Upload,
    font: Upload,
    hud: Upload,
    acceleration: Option<Acceleration>,
    fence: ID3D12Fence,
    event: HANDLE,
    serial: u64,
    info: Option<ID3D12InfoQueue>,
    pub width: u32,
    pub height: u32,
    pub hardware: bool,
    pub adapter: String,
    pub debug_layer: bool,
    pub primitive_count: usize,
    pub samples: u32,
    pub capabilities: crate::capabilities::Capabilities,
}

pub(crate) fn buffer(
    device: &ID3D12Device5,
    size: u64,
    heap: D3D12_HEAP_TYPE,
    state: D3D12_RESOURCE_STATES,
    flags: D3D12_RESOURCE_FLAGS,
) -> Result<ID3D12Resource> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size.max(256),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    };
    committed(device, &desc, heap, state)
}
pub(crate) fn committed(
    device: &ID3D12Device5,
    desc: &D3D12_RESOURCE_DESC,
    heap: D3D12_HEAP_TYPE,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource> {
    let props = D3D12_HEAP_PROPERTIES {
        Type: heap,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut resource = None;
    unsafe {
        device.CreateCommittedResource(
            &props,
            D3D12_HEAP_FLAG_NONE,
            desc,
            state,
            None,
            &mut resource,
        )?;
    }
    resource.context("CreateCommittedResource returned no resource")
}
pub(crate) fn transition(
    list: &ID3D12GraphicsCommandList4,
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let mut barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
        ..Default::default()
    };
    unsafe {
        list.ResourceBarrier(std::slice::from_ref(&barrier));
        ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource);
    }
}
pub(crate) fn uav_barrier(list: &ID3D12GraphicsCommandList4, resource: &ID3D12Resource) {
    let mut b = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: ManuallyDrop::new(Some(resource.clone())),
            }),
        },
        ..Default::default()
    };
    unsafe {
        list.ResourceBarrier(std::slice::from_ref(&b));
        ManuallyDrop::drop(&mut (*b.Anonymous.UAV).pResource);
    }
}
fn pipeline(
    device: &ID3D12Device5,
    root: &ID3D12RootSignature,
    bytes: &[u8],
) -> Result<ID3D12PipelineState> {
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        CS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: bytes.as_ptr().cast(),
            BytecodeLength: bytes.len(),
        },
        ..Default::default()
    };
    let result = unsafe { device.CreateComputePipelineState(&desc) };
    unsafe {
        ManuallyDrop::drop(&mut desc.pRootSignature);
    }
    Ok(result?)
}
fn root_signature(device: &ID3D12Device5) -> Result<ID3D12RootSignature> {
    let ranges = [D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
        NumDescriptors: 3,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    }];
    let mut params = vec![D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: 0,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    }];
    for i in 0..4 {
        params.push(D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: i,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        });
    }
    params.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: ranges.len() as u32,
                pDescriptorRanges: ranges.as_ptr(),
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    });
    params.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: 4,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    });
    let texture_range = [D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 2,
        BaseShaderRegister: 5,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    }];
    let feedback_range = [D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
        NumDescriptors: 1,
        BaseShaderRegister: 3,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    }];
    for range in [&texture_range[..], &feedback_range[..]] {
        params.push(D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: range.as_ptr(),
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        });
    }
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        MaxLOD: f32::MAX,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        ..Default::default()
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        ..Default::default()
    };
    let mut blob = None;
    let mut error = None;
    unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut error),
        )?;
        let blob = blob.context("Empty root signature")?;
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        Ok(device.CreateRootSignature(0, bytes)?)
    }
}

impl Acceleration {
    fn geometry(&self, count: usize) -> D3D12_RAYTRACING_GEOMETRY_DESC {
        D3D12_RAYTRACING_GEOMETRY_DESC {
            Type: D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS,
            Flags: D3D12_RAYTRACING_GEOMETRY_FLAG_NONE,
            Anonymous: D3D12_RAYTRACING_GEOMETRY_DESC_0 {
                AABBs: D3D12_RAYTRACING_GEOMETRY_AABBS_DESC {
                    AABBCount: count as u64,
                    AABBs: D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE {
                        StartAddress: self.aabbs.address(),
                        StrideInBytes: 24,
                    },
                },
            },
        }
    }
    fn blas_inputs(
        geom: &D3D12_RAYTRACING_GEOMETRY_DESC,
    ) -> D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
        D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
            Type: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL,
            Flags: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_TRACE,
            NumDescs: 1,
            DescsLayout: D3D12_ELEMENTS_LAYOUT_ARRAY,
            Anonymous: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS_0 {
                pGeometryDescs: geom,
            },
        }
    }
    fn tlas_inputs(address: u64) -> D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
        D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
            Type: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL,
            Flags: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_TRACE,
            NumDescs: 1,
            DescsLayout: D3D12_ELEMENTS_LAYOUT_ARRAY,
            Anonymous: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS_0 {
                InstanceDescs: address,
            },
        }
    }
    fn new(device: &ID3D12Device5, count: usize) -> Result<Self> {
        let aabbs = Upload::new(device, count * 24)?;
        let instance = Upload::new(device, 64)?;
        let geom = D3D12_RAYTRACING_GEOMETRY_DESC {
            Type: D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS,
            Anonymous: D3D12_RAYTRACING_GEOMETRY_DESC_0 {
                AABBs: D3D12_RAYTRACING_GEOMETRY_AABBS_DESC {
                    AABBCount: count as u64,
                    AABBs: D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE {
                        StartAddress: aabbs.address(),
                        StrideInBytes: 24,
                    },
                },
            },
            ..Default::default()
        };
        let mut b = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO::default();
        let mut t = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO::default();
        unsafe {
            device
                .GetRaytracingAccelerationStructurePrebuildInfo(&Self::blas_inputs(&geom), &mut b);
            device.GetRaytracingAccelerationStructurePrebuildInfo(
                &Self::tlas_inputs(instance.address()),
                &mut t,
            );
        }
        anyhow::ensure!(
            b.ResultDataMaxSizeInBytes > 0 && t.ResultDataMaxSizeInBytes > 0,
            "DXR prebuild returned zero size"
        );
        let blas = buffer(
            device,
            b.ResultDataMaxSizeInBytes,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        )?;
        let tlas = buffer(
            device,
            t.ResultDataMaxSizeInBytes,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        )?;
        let scratch = buffer(
            device,
            b.ScratchDataSizeInBytes.max(t.ScratchDataSizeInBytes),
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        )?;
        let inst = D3D12_RAYTRACING_INSTANCE_DESC {
            Transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
            _bitfield1: 255 << 24,
            _bitfield2: 0,
            AccelerationStructure: unsafe { blas.GetGPUVirtualAddress() },
        };
        // This Windows struct is exactly 64 bytes, including the two DXR bitfields.
        instance.write(unsafe {
            std::slice::from_raw_parts(
                (&inst as *const D3D12_RAYTRACING_INSTANCE_DESC).cast::<u8>(),
                64,
            )
        })?;
        Ok(Self {
            aabbs,
            instance,
            blas,
            tlas,
            scratch,
        })
    }
    fn build(&self, list: &ID3D12GraphicsCommandList4, objects: &[Object]) -> Result<()> {
        transition(
            list,
            &self.scratch,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        );
        let bounds: Vec<_> = objects.iter().map(Object::bounds).collect();
        self.aabbs.write(bytemuck::cast_slice(&bounds))?;
        let geom = self.geometry(objects.len());
        unsafe {
            list.BuildRaytracingAccelerationStructure(
                &D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC {
                    DestAccelerationStructureData: self.blas.GetGPUVirtualAddress(),
                    Inputs: Self::blas_inputs(&geom),
                    ScratchAccelerationStructureData: self.scratch.GetGPUVirtualAddress(),
                    ..Default::default()
                },
                None,
            );
        }
        uav_barrier(list, &self.blas);
        uav_barrier(list, &self.scratch);
        unsafe {
            list.BuildRaytracingAccelerationStructure(
                &D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC {
                    DestAccelerationStructureData: self.tlas.GetGPUVirtualAddress(),
                    Inputs: Self::tlas_inputs(self.instance.address()),
                    ScratchAccelerationStructureData: self.scratch.GetGPUVirtualAddress(),
                    ..Default::default()
                },
                None,
            );
        }
        uav_barrier(list, &self.tlas);
        uav_barrier(list, &self.scratch);
        transition(
            list,
            &self.scratch,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COMMON,
        );
        Ok(())
    }
}

impl Renderer {
    pub fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        software: bool,
        debug: bool,
        budget: u32,
        primitive_count: usize,
    ) -> Result<Self> {
        unsafe {
            let mut debug_api: Option<ID3D12Debug> = None;
            let debug_layer = debug && D3D12GetDebugInterface(&mut debug_api).is_ok();
            if let Some(d) = debug_api {
                d.EnableDebugLayer();
            }
            let factory: IDXGIFactory6 = CreateDXGIFactory2(if debug_layer {
                DXGI_CREATE_FACTORY_DEBUG
            } else {
                DXGI_CREATE_FACTORY_FLAGS(0)
            })?;
            let mut selected = None;
            for index in 0..16 {
                let Ok(adapter) = factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(
                    index,
                    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
                ) else {
                    break;
                };
                if adapter.GetDesc1()?.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    continue;
                }
                let mut device: Option<ID3D12Device5> = None;
                if D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).is_ok() {
                    selected = Some((adapter, device.context("No D3D12 device")?));
                    break;
                }
            }
            let (adapter, device) =
                selected.context("No DirectX 12 GPU found. Update your graphics driver.")?;
            let name = adapter.GetDesc1()?.Description;
            let adapter = String::from_utf16_lossy(
                &name[..name.iter().position(|v| *v == 0).unwrap_or(name.len())],
            );
            let capabilities = crate::capabilities::Capabilities::query(&device);
            std::fs::create_dir_all("artifacts")?;
            std::fs::write(
                "artifacts/capabilities.json",
                serde_json::to_string_pretty(&capabilities.json())?,
            )?;
            let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS5::default();
            let hardware = device
                .CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS5,
                    (&mut options as *mut D3D12_FEATURE_DATA_D3D12_OPTIONS5).cast(),
                    size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS5>() as u32,
                )
                .is_ok()
                && options.RaytracingTier.0 >= D3D12_RAYTRACING_TIER_1_1.0
                && !software;
            let queue: ID3D12CommandQueue =
                device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })?;
            let swap: IDXGISwapChain3 = factory
                .CreateSwapChainForHwnd(
                    &queue,
                    hwnd,
                    &DXGI_SWAP_CHAIN_DESC1 {
                        Width: width,
                        Height: height,
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                        BufferCount: 2,
                        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                        ..Default::default()
                    },
                    None,
                    None,
                )?
                .cast()?;
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            let allocator: ID3D12CommandAllocator =
                device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let list: ID3D12GraphicsCommandList4 =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
            list.Close()?;
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 6,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let root = root_signature(&device)?;
            let code: &[u8] = if hardware {
                include_bytes!(concat!(env!("OUT_DIR"), "/trace_dxr.dxil"))
            } else {
                include_bytes!(concat!(env!("OUT_DIR"), "/trace_soft.dxil"))
            };
            let trace = pipeline(&device, &root, code).context("Create ray tracing pipeline")?;
            let background = pipeline(
                &device,
                &root,
                include_bytes!(concat!(env!("OUT_DIR"), "/background.dxil")),
            )?;
            let ultimate = if hardware && capabilities.ultimate() {
                Some(crate::ultimate::Ultimate::new(
                    &device,
                    &root,
                    &heap,
                    capabilities.vrs_tile,
                    capabilities.shader_model >= 0x69,
                    budget,
                )?)
            } else {
                None
            };
            let compose = pipeline(
                &device,
                &root,
                include_bytes!(concat!(env!("OUT_DIR"), "/compose.dxil")),
            )?;
            let constants = Upload::new(&device, 256)?;
            let objects = Upload::new(&device, primitive_count * size_of::<Object>())?;
            let font = Upload::new(&device, 1920 * 32 * 4)?;
            font.write(bytemuck::cast_slice(&font_atlas()?))?;
            let hud = Upload::new(&device, 16 * 2048)?;
            let acceleration = if hardware {
                Some(Acceleration::new(&device, primitive_count)?)
            } else {
                None
            };
            let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            let event = CreateEventW(None, false, false, PCWSTR::null())?;
            let info = if debug_layer {
                device.cast().ok()
            } else {
                None
            };
            let mut r = Self {
                device,
                queue,
                allocator,
                list,
                swap,
                buffers: vec![],
                heap,
                root,
                trace,
                compose,
                background,
                ultimate,
                images: vec![],
                constants,
                objects,
                font,
                hud,
                acceleration,
                fence,
                event,
                serial: 0,
                info,
                width,
                height,
                hardware,
                adapter,
                debug_layer,
                primitive_count,
                samples: if hardware { 2 } else { 1 },
                capabilities,
            };
            r.create_images()?;
            Ok(r)
        }
    }
    fn create_images(&mut self) -> Result<()> {
        unsafe {
            self.buffers = (0..2)
                .map(|i| self.swap.GetBuffer(i))
                .collect::<windows::core::Result<Vec<_>>>()?;
            let stride = self
                .device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                as usize;
            let start = self.heap.GetCPUDescriptorHandleForHeapStart().ptr;
            for (i, format) in [
                DXGI_FORMAT_R16G16B16A16_FLOAT,
                DXGI_FORMAT_R32G32B32A32_FLOAT,
                DXGI_FORMAT_R8G8B8A8_UNORM,
            ]
            .into_iter()
            .enumerate()
            {
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: (self.width * if i < 2 { self.samples } else { 1 }) as u64,
                    Height: self.height * if i < 2 { self.samples } else { 1 },
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
                        | if i < 2 {
                            D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
                        } else {
                            D3D12_RESOURCE_FLAG_NONE
                        },
                    ..Default::default()
                };
                let image = committed(
                    &self.device,
                    &desc,
                    D3D12_HEAP_TYPE_DEFAULT,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )?;
                self.device.CreateUnorderedAccessView(
                    &image,
                    None,
                    None,
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: start + i * stride,
                    },
                );
                self.images.push(image);
            }
            if let Some(u) = &mut self.ultimate {
                u.resize(
                    &self.device,
                    &self.images,
                    self.width * self.samples,
                    self.height * self.samples,
                )?;
            }
            Ok(())
        }
    }
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == self.width && height == self.height {
            return Ok(());
        }
        self.wait()?;
        self.buffers.clear();
        self.images.clear();
        unsafe {
            self.swap.ResizeBuffers(
                2,
                width,
                height,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.width = width;
        self.height = height;
        self.create_images()
    }
    pub fn toggle_antialiasing(&mut self) -> Result<()> {
        self.wait()?;
        self.samples = if self.samples == 1 { 2 } else { 1 };
        self.buffers.clear();
        self.images.clear();
        self.create_images()
    }
    fn wait(&mut self) -> Result<()> {
        unsafe {
            self.serial += 1;
            self.queue.Signal(&self.fence, self.serial)?;
            if self.fence.GetCompletedValue() < self.serial {
                self.fence.SetEventOnCompletion(self.serial, self.event)?;
                if WaitForSingleObject(self.event, 15000) != WAIT_OBJECT_0 {
                    bail!(
                        "GPU fence timeout; device status: {:?}",
                        self.device.GetDeviceRemovedReason()
                    );
                }
            }
            Ok(())
        }
    }
    pub fn draw(&mut self, frame: &Frame, objects: &[Object], hud: &[[f32; 4]]) -> Result<()> {
        anyhow::ensure!(
            objects.len() == self.primitive_count,
            "Primitive count changed"
        );
        // Deliberately serialize frames: upload memory and AS scratch are never reused while in flight.
        self.constants.write(bytemuck::bytes_of(frame))?;
        self.objects.write(bytemuck::cast_slice(objects))?;
        self.hud.write(bytemuck::cast_slice(hud))?;
        unsafe {
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, None)?;
            if let Some(a) = &self.acceleration {
                a.build(&self.list, objects)?;
            }
            self.list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            let ultimate_active = self.ultimate.as_ref().is_some_and(|u| u.enabled);
            if ultimate_active {
                self.ultimate
                    .as_mut()
                    .unwrap()
                    .streaming
                    .begin(&self.queue, &self.list)?;
            }
            self.list.SetComputeRootSignature(&self.root);
            self.list
                .SetComputeRootConstantBufferView(0, self.constants.address());
            self.list
                .SetComputeRootShaderResourceView(1, self.objects.address());
            if let Some(a) = &self.acceleration {
                self.list
                    .SetComputeRootShaderResourceView(2, a.tlas.GetGPUVirtualAddress());
            }
            self.list
                .SetComputeRootShaderResourceView(3, self.font.address());
            self.list
                .SetComputeRootShaderResourceView(4, self.hud.address());
            self.list
                .SetComputeRootDescriptorTable(5, self.heap.GetGPUDescriptorHandleForHeapStart());
            self.list.SetPipelineState(if ultimate_active {
                &self.background
            } else {
                &self.trace
            });
            self.list.Dispatch(
                (self.width * self.samples).div_ceil(8),
                (self.height * self.samples).div_ceil(8),
                1,
            );
            uav_barrier(&self.list, &self.images[0]);
            uav_barrier(&self.list, &self.images[1]);
            if ultimate_active {
                let u = self.ultimate.as_mut().unwrap();
                self.list.SetGraphicsRootSignature(&self.root);
                self.list
                    .SetGraphicsRootConstantBufferView(0, self.constants.address());
                self.list
                    .SetGraphicsRootShaderResourceView(1, self.objects.address());
                self.list.SetGraphicsRootShaderResourceView(
                    2,
                    self.acceleration
                        .as_ref()
                        .unwrap()
                        .tlas
                        .GetGPUVirtualAddress(),
                );
                self.list
                    .SetGraphicsRootShaderResourceView(6, u.patches.address());
                let start = self.heap.GetGPUDescriptorHandleForHeapStart().ptr;
                let stride = self
                    .device
                    .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                    as u64;
                self.list.SetGraphicsRootDescriptorTable(
                    7,
                    D3D12_GPU_DESCRIPTOR_HANDLE {
                        ptr: start + 3 * stride,
                    },
                );
                self.list.SetGraphicsRootDescriptorTable(
                    8,
                    D3D12_GPU_DESCRIPTOR_HANDLE {
                        ptr: start + 5 * stride,
                    },
                );
                u.draw(&self.list, &self.images)?;
                u.streaming.end(&self.list)?;
            }
            self.list.SetPipelineState(&self.compose);
            self.list
                .Dispatch(self.width.div_ceil(8), self.height.div_ceil(8), 1);
            let back = &self.buffers[self.swap.GetCurrentBackBufferIndex() as usize];
            transition(
                &self.list,
                &self.images[2],
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            transition(
                &self.list,
                back,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
            self.list.CopyResource(back, &self.images[2]);
            transition(
                &self.list,
                back,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            transition(
                &self.list,
                &self.images[2],
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            );
            self.list.Close()?;
            self.queue.ExecuteCommandLists(&[Some(self.list.cast()?)]);
            self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        self.wait()?;
        Ok(())
    }
    pub fn capture(&mut self, path: &Path) -> Result<()> {
        unsafe {
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut total = 0;
            self.device.GetCopyableFootprints(
                &self.images[2].GetDesc(),
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                Some(&mut total),
            );
            let readback = buffer(
                &self.device,
                total,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_FLAG_NONE,
            )?;
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, None)?;
            transition(
                &self.list,
                &self.images[2],
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            let mut dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: ManuallyDrop::new(Some(readback.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: footprint,
                },
            };
            let mut src = D3D12_TEXTURE_COPY_LOCATION {
                pResource: ManuallyDrop::new(Some(self.images[2].clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            self.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            ManuallyDrop::drop(&mut dst.pResource);
            ManuallyDrop::drop(&mut src.pResource);
            transition(
                &self.list,
                &self.images[2],
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            );
            self.list.Close()?;
            self.queue.ExecuteCommandLists(&[Some(self.list.cast()?)]);
            self.wait()?;
            let mut ptr: *mut c_void = std::ptr::null_mut();
            readback.Map(0, None, Some(&mut ptr))?;
            let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), total as usize);
            let result = save_bmp(
                path,
                self.width,
                self.height,
                footprint.Footprint.RowPitch as usize,
                bytes,
            );
            readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
            result
        }
    }
    pub fn diagnostics(&self) -> Vec<String> {
        let mut result = vec![];
        if let Some(info) = &self.info {
            unsafe {
                for i in 0..info.GetNumStoredMessagesAllowedByRetrievalFilter() {
                    let mut size = 0;
                    if info.GetMessage(i, None, &mut size).is_err() {
                        continue;
                    }
                    let mut storage = vec![0u64; size.div_ceil(8)];
                    let message = storage.as_mut_ptr().cast::<D3D12_MESSAGE>();
                    if info.GetMessage(i, Some(message), &mut size).is_ok() {
                        let m = &*message;
                        if m.Severity.0 <= D3D12_MESSAGE_SEVERITY_WARNING.0 {
                            let text = std::slice::from_raw_parts(
                                m.pDescription.cast::<u8>(),
                                m.DescriptionByteLength,
                            );
                            result.push(
                                String::from_utf8_lossy(text)
                                    .trim_end_matches('\0')
                                    .to_string(),
                            );
                        }
                    }
                }
            }
        }
        result
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        let _ = self.wait();
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

fn font_atlas() -> Result<Vec<u32>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        anyhow::ensure!(!dc.is_invalid(), "Create font atlas DC failed");
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 1920,
                biHeight: -32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let old_bitmap = SelectObject(dc, bitmap.into());
        let font = CreateFontW(
            24,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font.into());
        SetTextColor(dc, windows::Win32::Foundation::COLORREF(0xFFFFFF));
        SetBkColor(dc, windows::Win32::Foundation::COLORREF(0));
        std::ptr::write_bytes(bits.cast::<u8>(), 0, 1920 * 32 * 4);
        for code in 32..128 {
            let _ = TextOutW(dc, (code - 32) * 20, 0, &[code as u16]);
        }
        let _ = GdiFlush();
        let atlas = std::slice::from_raw_parts(bits.cast::<u32>(), 1920 * 32)
            .iter()
            .map(|v| v & 255)
            .collect();
        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        Ok(atlas)
    }
}
fn save_bmp(path: &Path, w: u32, h: u32, pitch: usize, pixels: &[u8]) -> Result<()> {
    let mut file = Vec::with_capacity(54 + (w * h * 4) as usize);
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&(54 + w * h * 4).to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&54u32.to_le_bytes());
    file.extend_from_slice(&40u32.to_le_bytes());
    file.extend_from_slice(&(w as i32).to_le_bytes());
    file.extend_from_slice(&(-(h as i32)).to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&32u16.to_le_bytes());
    file.extend_from_slice(&[0; 24]);
    for y in 0..h as usize {
        for p in pixels[y * pitch..y * pitch + w as usize * 4]
            .as_chunks::<4>()
            .0
        {
            file.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, file)?;
    Ok(())
}
