use std::num::NonZeroU64;

use bevy::render::render_resource::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutId, BindingResource, WgpuSampler, WgpuTextureView,
};
use bevy::render::renderer::RenderDevice;

const KEEP_FRAMES: u64 = 3;

#[derive(PartialEq, Eq)]
enum Bound {
    Buffer(wgpu::Buffer, u64, Option<NonZeroU64>),
    Sampler(WgpuSampler),
    View(WgpuTextureView),
}

impl Bound {
    fn of(resource: &BindingResource) -> Option<Bound> {
        match resource {
            BindingResource::Buffer(b) => Some(Bound::Buffer(b.buffer.clone(), b.offset, b.size)),
            BindingResource::Sampler(s) => Some(Bound::Sampler((*s).clone())),
            BindingResource::TextureView(v) => Some(Bound::View((*v).clone())),
            _ => None,
        }
    }

    fn is(&self, resource: &BindingResource) -> bool {
        match (self, resource) {
            (Bound::Buffer(buffer, offset, size), BindingResource::Buffer(b)) => {
                buffer == b.buffer && *offset == b.offset && *size == b.size
            }
            (Bound::Sampler(s), BindingResource::Sampler(r)) => s == *r,
            (Bound::View(v), BindingResource::TextureView(r)) => v == *r,
            _ => false,
        }
    }
}

struct Kept {
    label: &'static str,
    layout: BindGroupLayoutId,
    entries: Vec<(u32, Bound)>,
    group: BindGroup,
    used: u64,
}

impl Kept {
    fn binds(&self, label: &'static str, layout: &BindGroupLayout, entries: &[BindGroupEntry]) -> bool {
        self.layout == layout.id()
            && self.label == label
            && self.entries.len() == entries.len()
            && self.entries.iter().zip(entries).all(|((i, b), e)| *i == e.binding && b.is(&e.resource))
    }
}

#[derive(Default)]
pub struct BindGroupCache {
    frame: u64,
    kept: Vec<Kept>,
}

impl BindGroupCache {
    pub fn next_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.kept.retain(|k| frame - k.used < KEEP_FRAMES);
    }

    pub fn get(
        &mut self,
        device: &RenderDevice,
        label: &'static str,
        layout: &BindGroupLayout,
        entries: &[BindGroupEntry],
    ) -> BindGroup {
        if let Some(k) = self.kept.iter_mut().find(|k| k.binds(label, layout, entries)) {
            k.used = self.frame;
            return k.group.clone();
        }
        let group = device.create_bind_group(label, layout, entries);
        let key: Option<Vec<(u32, Bound)>> =
            entries.iter().map(|e| Bound::of(&e.resource).map(|b| (e.binding, b))).collect();
        if let Some(entries) = key {
            self.kept.push(Kept { label, layout: layout.id(), entries, group: group.clone(), used: self.frame });
        }
        group
    }
}

#[cfg(test)]
mod tests {
    use bevy::render::render_resource::{
        BindGroupEntries, BindGroupLayoutEntries, Buffer, BufferDescriptor, BufferUsages, SamplerBindingType,
        SamplerDescriptor, ShaderStages, binding_types,
    };
    use bevy::tasks::block_on;

    use super::*;

    fn device() -> RenderDevice {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::NOOP,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            display: None,
            backend_options: wgpu::BackendOptions {
                noop: wgpu::NoopBackendOptions { enable: true, ..Default::default() },
                ..Default::default()
            },
        });
        let adapter =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("noop adapter");
        let (device, _queue) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("noop device");
        RenderDevice::from(device)
    }

    fn buffer(device: &RenderDevice) -> Buffer {
        device.create_buffer(&BufferDescriptor {
            label: None,
            size: 16,
            usage: BufferUsages::UNIFORM,
            mapped_at_creation: false,
        })
    }

    #[test]
    fn kept_while_the_entries_are_the_same() {
        let device = device();
        let layout = device.create_bind_group_layout(
            None,
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    binding_types::uniform_buffer_sized(false, None),
                    binding_types::sampler(SamplerBindingType::Filtering),
                ),
            ),
        );
        let (a, b) = (buffer(&device), buffer(&device));
        let sampler = device.create_sampler(&SamplerDescriptor::default());
        let mut cache = BindGroupCache::default();
        let bind = |cache: &mut BindGroupCache, label, buffer: &Buffer| {
            cache.get(&device, label, &layout, &BindGroupEntries::sequential((buffer.as_entire_binding(), &sampler)))
        };
        cache.next_frame();
        let first = bind(&mut cache, "x", &a);
        assert_eq!(bind(&mut cache, "x", &a).id(), first.id(), "the same entries: the kept group");
        let other = bind(&mut cache, "x", &b);
        assert_ne!(other.id(), first.id(), "another buffer: another group");
        assert_ne!(bind(&mut cache, "y", &a).id(), first.id(), "another label: another group");
        for _ in 1..KEEP_FRAMES {
            cache.next_frame();
            assert_eq!(bind(&mut cache, "x", &b).id(), other.id());
        }
        assert_eq!(bind(&mut cache, "x", &a).id(), first.id(), "kept while unused for fewer than KEEP_FRAMES frames");
        for _ in 0..KEEP_FRAMES {
            cache.next_frame();
        }
        assert_ne!(bind(&mut cache, "x", &a).id(), first.id(), "dropped after KEEP_FRAMES unused frames");
    }
}
