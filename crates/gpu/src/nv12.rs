//! GPU RGBA8 → NV12 (BT.709 limited range) for hardware-encoded exports.
//!
//! A compute pass reads the resolved `Rgba8Unorm` output texture and writes the NV12 picture into
//! a storage buffer that is copied to a mappable staging buffer: only `w * h * 3 / 2` bytes cross
//! back to the CPU instead of `w * h * 4`, and the CPU conversion disappears. The arithmetic is
//! the CPU reference's (see `nv12.wgsl`).

pub(crate) const NV12_RING_LEN: usize = 4;

pub(crate) struct Nv12InFlight {
    slot: usize,
    size: u64,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

pub(crate) struct Nv12Stage {
    y_pipeline: wgpu::ComputePipeline,
    uv_pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// GPU-only storage buffer written by the compute shader.
    storage: Option<(wgpu::Buffer, u64)>,
    storage_gen: u64,
    /// Cached bind group keyed by `(output_gen, storage_gen)`.
    cached_bg: Option<(wgpu::BindGroup, u64, u64)>,
    /// Ring of mappable staging buffers for pipelined async readback.
    staging_slots: Vec<Option<(wgpu::Buffer, u64)>>,
}

impl Nv12Stage {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("filmcraft-nv12"),
            source: wgpu::ShaderSource::Wgsl(include_str!("nv12.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nv12-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("nv12-layout"), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = |entry: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nv12-size"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            y_pipeline: pipe("y_main"),
            uv_pipeline: pipe("uv_main"),
            bgl,
            uniform,
            storage: None,
            storage_gen: 0,
            cached_bg: None,
            staging_slots: (0..NV12_RING_LEN).map(|_| None).collect(),
        }
    }

    /// Whether a `w` x `h` picture can be converted here (whole 4-pixel words, even height).
    pub(crate) fn fits(w: u32, h: u32) -> bool {
        w >= 4 && h >= 2 && w.is_multiple_of(4) && h.is_multiple_of(2)
    }

    /// Record the conversion of `src` (the `Rgba8Unorm` output, `w` x `h`) and its copy to
    /// `staging_slots[slot]`; returns the NV12 size in bytes.
    #[expect(clippy::too_many_arguments, reason = "explicit slot and generation avoid duplicating the record path")]
    pub(crate) fn record_slot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        output_gen: u64,
        w: u32,
        h: u32,
        slot: usize,
    ) -> Option<u64> {
        if !Self::fits(w, h) || slot >= self.staging_slots.len() {
            return None;
        }
        let size = u64::from(w) * u64::from(h) * 3 / 2;
        if self.storage.as_ref().is_none_or(|b| b.1 < size) {
            let storage = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nv12-storage"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            self.storage = Some((storage, size));
            self.storage_gen = self.storage_gen.wrapping_add(1);
        }
        if self.staging_slots.get(slot)?.as_ref().is_none_or(|b| b.1 < size) {
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nv12-staging"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            *self.staging_slots.get_mut(slot)? = Some((staging, size));
        }
        let (storage, _) = self.storage.as_ref()?;
        let (staging, _) = self.staging_slots.get(slot)?.as_ref()?;
        let mut dims = [0u8; 16];
        dims[..4].copy_from_slice(&w.to_le_bytes());
        dims[4..8].copy_from_slice(&h.to_le_bytes());
        queue.write_buffer(&self.uniform, 0, &dims);
        let need_bg = self.cached_bg.as_ref().is_none_or(|(_, og, sg)| *og != output_gen || *sg != self.storage_gen);
        if need_bg {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("nv12-bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: storage.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
                ],
            });
            self.cached_bg = Some((bg, output_gen, self.storage_gen));
        }
        let (bg, _, _) = self.cached_bg.as_ref()?;
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("nv12"), timestamp_writes: None });
            pass.set_bind_group(0, bg, &[]);
            let gx = (w / 4).div_ceil(16);
            pass.set_pipeline(&self.y_pipeline);
            pass.dispatch_workgroups(gx, h.div_ceil(16), 1);
            pass.set_pipeline(&self.uv_pipeline);
            pass.dispatch_workgroups(gx, (h / 2).div_ceil(16), 1);
        }
        enc.copy_buffer_to_buffer(storage, 0, staging, 0, size);
        Some(size)
    }

    /// Initiate an asynchronous map on `staging_slots[slot]` without blocking.
    pub(crate) fn start_map(&self, slot: usize, size: u64) -> Option<Nv12InFlight> {
        let (staging, _) = self.staging_slots.get(slot)?.as_ref()?;
        let slice = staging.slice(..size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        Some(Nv12InFlight { slot, size, rx })
    }

    /// Wait for a previously started async map and copy the NV12 bytes out.
    /// Retires the slot's staging buffer on failure so a broken map is never reused.
    pub(crate) fn collect_map(&mut self, device: &wgpu::Device, ticket: Nv12InFlight) -> Option<Vec<u8>> {
        let (staging, capacity) = self.staging_slots.get_mut(ticket.slot)?.take()?;
        let wait_ok = super::wait_mapped(device, &ticket.rx).is_some();
        if !wait_ok {
            return None;
        }
        let slice = staging.slice(..ticket.size);
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity(ticket.size as usize);
        super::copy_mapped(&data, &mut out);
        drop(data);
        staging.unmap();
        if let Some(slot) = self.staging_slots.get_mut(ticket.slot) {
            *slot = Some((staging, capacity));
        }
        Some(out)
    }

    /// Record the conversion of `src` (the `Rgba8Unorm` output, `w` x `h`) and its copy to
    /// staging slot 0; returns the NV12 size in bytes.
    #[cfg(test)]
    #[expect(dead_code, reason = "helper for single-frame tests")]
    pub(crate) fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) -> Option<u64> {
        self.record_slot(device, queue, enc, src, 0, w, h, 0)
    }

    /// Wait for staging slot 0 and copy the NV12 bytes out.
    #[cfg(test)]
    #[expect(dead_code, reason = "helper for single-frame tests")]
    pub(crate) fn map(&mut self, device: &wgpu::Device, size: u64) -> Option<Vec<u8>> {
        let ticket = self.start_map(0, size)?;
        self.collect_map(device, ticket)
    }
}
