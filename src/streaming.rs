use crate::renderer::{Upload, buffer, committed, transition, uav_barrier};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    mem::ManuallyDrop,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};
use windows::{
    Win32::Graphics::{Direct3D12::*, Dxgi::Common::*},
    core::Interface,
};

const SIZE: u32 = 16384;
const TILE: u32 = 128;
const MIPS: u16 = 15;
const BATCH: usize = 16;
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
struct Page {
    mip: u32,
    x: u32,
    y: u32,
}
struct Resident {
    slot: u32,
    last_used: u64,
    pinned: bool,
}
pub struct Streaming {
    pub texture: ID3D12Resource,
    pub min_mip: ID3D12Resource,
    feedback: ID3D12Resource,
    decoded: ID3D12Resource,
    readback: ID3D12Resource,
    readback_layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    readback_size: u64,
    heap: ID3D12Heap,
    upload: Upload,
    mip_upload: Upload,
    pages: HashMap<Page, Resident>,
    pending: HashSet<Page>,
    wanted: HashSet<Page>,
    request: Sender<Page>,
    response: Receiver<(Page, Vec<u8>)>,
    slots: u32,
    packed: D3D12_PACKED_MIP_INFO,
    initialized: bool,
    readback_ready: bool,
    frame: u64,
    pub uploaded: u64,
    pub evicted: u64,
    pub feedback_requests: u64,
    pub resident_bytes: u64,
    pub feedback_enabled: bool,
    cpu_feedback_handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    gpu_feedback_handle: D3D12_GPU_DESCRIPTOR_HANDLE,
    _clear_heap: ID3D12DescriptorHeap,
}

fn texture_desc(width: u32, height: u32, format: DXGI_FORMAT) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..Default::default()
    }
}
fn tile_data(page: Page) -> Vec<u8> {
    let mut data = vec![0u8; 65536];
    for y in 0..TILE {
        for x in 0..TILE {
            let gx = (page.x * TILE + x) << page.mip;
            let gy = (page.y * TILE + y) << page.mip;
            // Material detail is defined in virtual texel space, consistently across all mip levels.
            let zone = (gx / (SIZE / 4)) + (gy / (SIZE / 2)) * 4;
            let checker = ((gx / 256 + gy / 256) & 1) as u8;
            let grain = ((gx.wrapping_mul(1664525) ^ gy.wrapping_mul(1013904223)) >> 26) as u8;
            let weave = ((gx / 16 + gy / 16) & 1) as u8;
            let ink = if gx % 512 < 12 || gy % 512 < 12 {
                55
            } else {
                0
            };
            let value = match zone % 4 {
                0 => 225 + checker * 20,
                1 => 230 + weave * 18,
                2 => 190 + grain.min(50),
                _ => 245u8.saturating_sub(ink),
            };
            let offset = ((y * TILE + x) * 4) as usize;
            data[offset..offset + 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
    data
}
impl Streaming {
    pub fn new(
        device: &ID3D12Device5,
        descriptors: &ID3D12DescriptorHeap,
        budget_mb: u32,
    ) -> Result<Self> {
        unsafe {
            let device8: ID3D12Device8 = device.cast()?;
            let mut desc = texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
            desc.MipLevels = MIPS;
            desc.Layout = D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE;
            let mut texture = None;
            device.CreateReservedResource(
                &desc,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                None,
                &mut texture,
            )?;
            let texture: ID3D12Resource = texture.context("Reserved texture")?;
            let mut packed = D3D12_PACKED_MIP_INFO::default();
            let mut shape = D3D12_TILE_SHAPE::default();
            let mut tilings = vec![D3D12_SUBRESOURCE_TILING::default(); MIPS as usize];
            let mut count = MIPS as u32;
            device.GetResourceTiling(
                &texture,
                None,
                Some(&mut packed),
                Some(&mut shape),
                Some(&mut count),
                0,
                tilings.as_mut_ptr(),
            );
            anyhow::ensure!(
                shape.WidthInTexels == TILE && shape.HeightInTexels == TILE,
                "Unexpected tile shape: {}x{}",
                shape.WidthInTexels,
                shape.HeightInTexels
            );
            let slots = budget_mb.clamp(4, 256) * 16;
            let mut heap = None;
            device.CreateHeap(
                &D3D12_HEAP_DESC {
                    SizeInBytes: slots as u64 * 65536,
                    Properties: D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        CreationNodeMask: 1,
                        VisibleNodeMask: 1,
                        ..Default::default()
                    },
                    Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
                    ..Default::default()
                },
                &mut heap,
            )?;
            let heap = heap.context("Texture page pool")?;
            let fb_desc = D3D12_RESOURCE_DESC1 {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: SIZE as u64,
                Height: SIZE,
                DepthOrArraySize: 1,
                MipLevels: MIPS,
                Format: DXGI_FORMAT_SAMPLER_FEEDBACK_MIN_MIP_OPAQUE,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                SamplerFeedbackMipRegion: D3D12_MIP_REGION {
                    Width: TILE,
                    Height: TILE,
                    Depth: 1,
                },
                ..Default::default()
            };
            let mut feedback = None;
            device8.CreateCommittedResource2(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    CreationNodeMask: 1,
                    VisibleNodeMask: 1,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &fb_desc,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                None,
                None,
                &mut feedback,
            )?;
            let feedback = feedback.context("Sampler feedback resource")?;
            let decoded = committed(
                device,
                &texture_desc(128, 128, DXGI_FORMAT_R8_UINT),
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_RESOLVE_DEST,
            )?;
            let min_mip = committed(
                device,
                &texture_desc(128, 128, DXGI_FORMAT_R16_UINT),
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            )?;
            let mut readback_layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut readback_size = 0;
            device.GetCopyableFootprints(
                &decoded.GetDesc(),
                0,
                1,
                0,
                Some(&mut readback_layout),
                None,
                None,
                Some(&mut readback_size),
            );
            let readback = buffer(
                device,
                readback_size,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_FLAG_NONE,
            )?;
            let stride = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                as usize;
            let cpu = descriptors.GetCPUDescriptorHandleForHeapStart();
            let gpu = descriptors.GetGPUDescriptorHandleForHeapStart();
            for (index, resource, format, mips) in [
                (3, &texture, DXGI_FORMAT_R8G8B8A8_UNORM, MIPS as u32),
                (4, &min_mip, DXGI_FORMAT_R16_UINT, 1),
            ] {
                let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MipLevels: mips,
                            ..Default::default()
                        },
                    },
                };
                device.CreateShaderResourceView(
                    resource,
                    Some(&srv),
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: cpu.ptr + index * stride,
                    },
                );
            }
            let visible_feedback_handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + 5 * stride,
            };
            let gpu_feedback_handle = D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + 5 * stride as u64,
            };
            device8.CreateSamplerFeedbackUnorderedAccessView(
                &texture,
                &feedback,
                visible_feedback_handle,
            );
            let clear_heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 1,
                    ..Default::default()
                })?;
            let cpu_feedback_handle = clear_heap.GetCPUDescriptorHandleForHeapStart();
            device8.CreateSamplerFeedbackUnorderedAccessView(
                &texture,
                &feedback,
                cpu_feedback_handle,
            );
            let upload = Upload::new(device, (BATCH + MIPS as usize) * 65536)?;
            let mip_upload = Upload::new(device, 256 * 128)?;
            let (request, rx) = mpsc::channel::<Page>();
            let (tx, response) = mpsc::channel();
            thread::spawn(move || {
                let cache = std::path::Path::new("assets/vt-cache");
                let _ = std::fs::create_dir_all(cache);
                while let Ok(page) = rx.recv() {
                    let path = cache.join(format!("m{}_{}_{}.rgba", page.mip, page.x, page.y));
                    let data = std::fs::read(&path)
                        .ok()
                        .filter(|v| v.len() == 65536)
                        .unwrap_or_else(|| {
                            let v = tile_data(page);
                            let _ = std::fs::write(&path, &v);
                            v
                        });
                    if tx.send((page, data)).is_err() {
                        break;
                    }
                }
            });
            Ok(Self {
                texture,
                min_mip,
                feedback,
                decoded,
                readback,
                readback_layout,
                readback_size,
                heap,
                upload,
                mip_upload,
                pages: HashMap::new(),
                pending: HashSet::new(),
                wanted: HashSet::new(),
                request,
                response,
                slots,
                packed,
                initialized: false,
                readback_ready: false,
                frame: 0,
                uploaded: 0,
                evicted: 0,
                feedback_requests: 0,
                resident_bytes: 0,
                feedback_enabled: true,
                cpu_feedback_handle,
                gpu_feedback_handle,
                _clear_heap: clear_heap,
            })
        }
    }
    fn map(&self, queue: &ID3D12CommandQueue, page: Page, slot: u32, unmap: bool) {
        let coord = D3D12_TILED_RESOURCE_COORDINATE {
            X: page.x,
            Y: page.y,
            Z: 0,
            Subresource: page.mip,
        };
        let region = D3D12_TILE_REGION_SIZE {
            NumTiles: 1,
            UseBox: true.into(),
            Width: 1,
            Height: 1,
            Depth: 1,
        };
        let flag = if unmap {
            D3D12_TILE_RANGE_FLAG_NULL
        } else {
            D3D12_TILE_RANGE_FLAG_NONE
        };
        unsafe {
            queue.UpdateTileMappings(
                &self.texture,
                1,
                Some(&coord),
                Some(&region),
                &self.heap,
                1,
                Some(&flag),
                Some(&slot),
                Some(&1),
                D3D12_TILE_MAPPING_FLAG_NONE,
            );
        }
    }
    fn read_feedback(&mut self) -> Result<()> {
        if !self.readback_ready || !self.feedback_enabled {
            return Ok(());
        }
        unsafe {
            let mut ptr = std::ptr::null_mut();
            self.readback.Map(0, None, Some(&mut ptr))?;
            let data = std::slice::from_raw_parts(ptr.cast::<u8>(), self.readback_size as usize);
            let pitch = self.readback_layout.Footprint.RowPitch as usize;
            let mut requests = HashMap::<Page, u32>::new();
            for y in 0..128 {
                for x in 0..128 {
                    let mip = data[y * pitch + x] as u32;
                    if mip < 6 {
                        *requests
                            .entry(Page {
                                mip,
                                x: (x as u32) >> mip,
                                y: (y as u32) >> mip,
                            })
                            .or_default() += 1;
                    }
                }
            }
            self.readback
                .Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
            self.feedback_requests += requests.len() as u64;
            let pinned = self.pages.values().filter(|r| r.pinned).count() as u32
                + self.packed.NumTilesForPackedMips;
            let mut requests: Vec<_> = requests.into_iter().collect();
            // Rank visible coverage with a small residency bonus to prevent camera jitter
            // from continuously replacing equally useful pages under a tight budget.
            requests.sort_by_key(|(p, coverage)| {
                (
                    std::cmp::Reverse(*coverage + if self.pages.contains_key(p) { 4 } else { 0 }),
                    std::cmp::Reverse(p.mip),
                    p.y,
                    p.x,
                )
            });
            self.wanted = requests
                .iter()
                .take(self.slots.saturating_sub(pinned) as usize)
                .map(|(p, _)| *p)
                .collect();
            for (p, _) in requests {
                if !self.wanted.contains(&p) {
                    continue;
                }
                if let Some(r) = self.pages.get_mut(&p) {
                    r.last_used = self.frame;
                } else if self.pending.len() < 256 && self.pending.insert(p) {
                    let _ = self.request.send(p);
                }
            }
        }
        Ok(())
    }
    pub fn begin(
        &mut self,
        queue: &ID3D12CommandQueue,
        list: &ID3D12GraphicsCommandList4,
    ) -> Result<()> {
        self.frame += 1;
        self.read_feedback()?;
        let previous_evictions = self.evicted;
        let mut updates: Vec<(Page, u32, Vec<u8>)> = vec![];
        if !self.initialized {
            // The packed tail and two coarse standard mips stay resident throughout the session.
            let coord = D3D12_TILED_RESOURCE_COORDINATE {
                Subresource: self.packed.NumStandardMips as u32,
                ..Default::default()
            };
            let region = D3D12_TILE_REGION_SIZE {
                NumTiles: self.packed.NumTilesForPackedMips,
                ..Default::default()
            };
            unsafe {
                queue.UpdateTileMappings(
                    &self.texture,
                    1,
                    Some(&coord),
                    Some(&region),
                    &self.heap,
                    1,
                    Some(&D3D12_TILE_RANGE_FLAG_NONE),
                    Some(&0),
                    Some(&self.packed.NumTilesForPackedMips),
                    D3D12_TILE_MAPPING_FLAG_NONE,
                );
            }
            let mut slot = self.packed.NumTilesForPackedMips;
            for mip in 6..self.packed.NumStandardMips as u32 {
                let n = (SIZE >> mip) / TILE;
                for y in 0..n {
                    for x in 0..n {
                        let p = Page { mip, x, y };
                        self.map(queue, p, slot, false);
                        self.pages.insert(
                            p,
                            Resident {
                                slot,
                                last_used: self.frame,
                                pinned: true,
                            },
                        );
                        updates.push((p, slot, tile_data(p)));
                        slot += 1;
                    }
                }
            }
        }
        if self.feedback_enabled {
            for _ in 0..BATCH {
                let Ok((page, data)) = self.response.try_recv() else {
                    break;
                };
                self.pending.remove(&page);
                if self.pages.contains_key(&page) || !self.wanted.contains(&page) {
                    continue;
                }
                let occupied: HashSet<u32> = self.pages.values().map(|r| r.slot).collect();
                let free =
                    (self.packed.NumTilesForPackedMips..self.slots).find(|s| !occupied.contains(s));
                let slot = if let Some(s) = free {
                    s
                } else {
                    let old = self
                        .pages
                        .iter()
                        .filter(|(_, r)| !r.pinned)
                        .min_by_key(|(_, r)| r.last_used)
                        .map(|(p, r)| (*p, r.slot));
                    let Some((p, s)) = old else { break };
                    self.map(queue, p, s, true);
                    self.pages.remove(&p);
                    self.evicted += 1;
                    s
                };
                self.map(queue, page, slot, false);
                self.pages.insert(
                    page,
                    Resident {
                        slot,
                        last_used: self.frame,
                        pinned: false,
                    },
                );
                updates.push((page, slot, data));
            }
        }
        if self.evicted > previous_evictions {
            // Reusing physical tiles at different virtual addresses requires an aliasing barrier.
            let mut barrier = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    Aliasing: ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                        pResourceBefore: ManuallyDrop::new(None),
                        pResourceAfter: ManuallyDrop::new(Some(self.texture.clone())),
                    }),
                },
                ..Default::default()
            };
            unsafe {
                list.ResourceBarrier(std::slice::from_ref(&barrier));
                ManuallyDrop::drop(&mut (*barrier.Anonymous.Aliasing).pResourceAfter);
            }
        }
        if !updates.is_empty() || !self.initialized {
            transition(
                list,
                &self.texture,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
            let mut bytes = vec![0u8; (updates.len() + MIPS as usize) * 65536];
            for (i, (p, _, data)) in updates.iter().enumerate() {
                bytes[i * 65536..(i + 1) * 65536].copy_from_slice(data);
                let coord = D3D12_TILED_RESOURCE_COORDINATE {
                    X: p.x,
                    Y: p.y,
                    Z: 0,
                    Subresource: p.mip,
                };
                let region = D3D12_TILE_REGION_SIZE {
                    NumTiles: 1,
                    UseBox: true.into(),
                    Width: 1,
                    Height: 1,
                    Depth: 1,
                };
                unsafe {
                    list.CopyTiles(
                        &self.texture,
                        &coord,
                        &region,
                        &self.upload.resource,
                        i as u64 * 65536,
                        D3D12_TILE_COPY_FLAG_LINEAR_BUFFER_TO_SWIZZLED_TILED_RESOURCE,
                    );
                }
            }
            if !self.initialized {
                for mip in self.packed.NumStandardMips as u32..MIPS as u32 {
                    let n = SIZE >> mip;
                    let offset = (updates.len() + mip as usize) * 65536;
                    for y in 0..n {
                        for x in 0..n {
                            let k = offset + (y * 256 + x * 4) as usize;
                            bytes[k..k + 4].copy_from_slice(&[238, 238, 238, 255]);
                        }
                    }
                    let fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: offset as u64,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            Width: n,
                            Height: n,
                            Depth: 1,
                            RowPitch: 256,
                        },
                    };
                    copy_upload(list, &self.texture, mip, &self.upload.resource, fp);
                }
            }
            self.upload.write(&bytes)?;
            self.uploaded += updates.len() as u64;
            transition(
                list,
                &self.texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            );
        }
        self.initialized = true;
        let mut min_map = vec![0u8; 256 * 128];
        for y in 0..128u32 {
            for x in 0..128u32 {
                let mut mask = 0x7fc0u16; // All coarse standard mips and the packed tail are pinned.
                for mip in 0..6 {
                    if self.pages.contains_key(&Page {
                        mip,
                        x: x >> mip,
                        y: y >> mip,
                    }) {
                        mask |= 1 << mip;
                    }
                }
                let offset = (y * 256 + x * 2) as usize;
                min_map[offset..offset + 2].copy_from_slice(&mask.to_le_bytes());
            }
        }
        self.mip_upload.write(&min_map)?;
        transition(
            list,
            &self.min_mip,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        );
        copy_upload(
            list,
            &self.min_mip,
            0,
            &self.mip_upload.resource,
            D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: DXGI_FORMAT_R16_UINT,
                    Width: 128,
                    Height: 128,
                    Depth: 1,
                    RowPitch: 256,
                },
            },
        );
        transition(
            list,
            &self.min_mip,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        );
        unsafe {
            list.ClearUnorderedAccessViewUint(
                self.gpu_feedback_handle,
                self.cpu_feedback_handle,
                &self.feedback,
                &[255; 4],
                &[],
            );
        }
        uav_barrier(list, &self.feedback);
        self.resident_bytes =
            (self.pages.len() as u64 + self.packed.NumTilesForPackedMips as u64) * 65536;
        Ok(())
    }
    pub fn end(&mut self, list: &ID3D12GraphicsCommandList4) -> Result<()> {
        uav_barrier(list, &self.feedback);
        transition(
            list,
            &self.feedback,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_RESOLVE_SOURCE,
        );
        unsafe {
            list.ResolveSubresourceRegion(
                &self.decoded,
                0,
                0,
                0,
                &self.feedback,
                0,
                None,
                DXGI_FORMAT_R8_UINT,
                D3D12_RESOLVE_MODE_DECODE_SAMPLER_FEEDBACK,
            );
        }
        transition(
            list,
            &self.feedback,
            D3D12_RESOURCE_STATE_RESOLVE_SOURCE,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        );
        transition(
            list,
            &self.decoded,
            D3D12_RESOURCE_STATE_RESOLVE_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
        unsafe {
            let mut dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: ManuallyDrop::new(Some(self.readback.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: self.readback_layout,
                },
            };
            let mut src = D3D12_TEXTURE_COPY_LOCATION {
                pResource: ManuallyDrop::new(Some(self.decoded.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            ManuallyDrop::drop(&mut dst.pResource);
            ManuallyDrop::drop(&mut src.pResource);
        }
        transition(
            list,
            &self.decoded,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_RESOLVE_DEST,
        );
        self.readback_ready = true;
        Ok(())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"virtual_texture_size":[SIZE,SIZE],"virtual_bytes":SIZE as u64*SIZE as u64*4*4/3,
        "physical_pool_bytes":self.slots as u64*65536,"resident_bytes":self.resident_bytes,"resident_pages":self.pages.len(),"uploaded_pages":self.uploaded,
        "evicted_pages":self.evicted,"hardware_feedback_requests":self.feedback_requests,"pending_pages":self.pending.len(),"enabled":self.feedback_enabled,
        "tile_bytes":65536,"cache":"assets/vt-cache","source":"procedural material pages generated asynchronously and cached on disk"})
    }
}
pub(crate) fn copy_upload(
    list: &ID3D12GraphicsCommandList4,
    dst_resource: &ID3D12Resource,
    mip: u32,
    src_resource: &ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) {
    unsafe {
        let mut dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(dst_resource.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: mip,
            },
        };
        let mut src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(src_resource.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint,
            },
        };
        list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        ManuallyDrop::drop(&mut dst.pResource);
        ManuallyDrop::drop(&mut src.pResource);
    }
}
