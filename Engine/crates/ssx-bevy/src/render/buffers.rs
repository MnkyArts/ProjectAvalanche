use std::marker::PhantomData;

use bevy::render::render_resource::encase::internal::WriteInto;
use bevy::render::render_resource::{
    BindingResource, Buffer, BufferDescriptor, BufferUsages, COPY_BUFFER_ALIGNMENT, IntoBinding, ShaderType, encase,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};

pub fn filled_buffer(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    contents: &[u8],
    usage: BufferUsages,
) -> Buffer {
    let padded = padded_contents(contents);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: padded.len() as u64,
        usage: usage | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !padded.is_empty() {
        queue.write_buffer(&buffer, 0, &padded);
    }
    buffer
}

pub fn filled_uniform<T: ShaderType + WriteInto>(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    value: &T,
) -> Buffer {
    let mut bytes = encase::UniformBuffer::new(Vec::new());
    bytes.write(value).expect("encase writes a uniform into a growing Vec");
    filled_buffer(device, queue, label, bytes.as_ref(), BufferUsages::UNIFORM)
}

pub struct QueuedUniform<T> {
    buffer: Option<Buffer>,
    bytes: Vec<u8>,
    written: Vec<u8>,
    value: PhantomData<fn(&T)>,
}

impl<T> Default for QueuedUniform<T> {
    fn default() -> Self {
        QueuedUniform { buffer: None, bytes: Vec::new(), written: Vec::new(), value: PhantomData }
    }
}

impl<T: ShaderType + WriteInto> QueuedUniform<T> {
    pub fn write(&mut self, device: &RenderDevice, queue: &RenderQueue, label: &'static str, value: &T) -> bool {
        let mut encoded = encase::UniformBuffer::new(std::mem::take(&mut self.bytes));
        encoded.write(value).expect("encase writes a uniform into a growing Vec");
        self.bytes = encoded.into_inner();
        let padded = padded_contents(&self.bytes);
        let size = padded.len() as u64;
        let (buffer, made) = match &self.buffer {
            Some(b) if b.size() == size => (b, false),
            _ => {
                let made = device.create_buffer(&BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                (&*self.buffer.insert(made), true)
            }
        };
        // PORT(choice): a value whose bytes are the buffer's already is not written again (the buffer is this
        // struct's own and only `write` writes it, so it holds the bytes of its last write): every `write_buffer` is a
        // staging buffer of wgpu's, made and mapped under its allocator's lock, its copy recorded under the queue's
        // lock and the staging buffer destroyed after the frame's submission (wgpu 30.0.2 `Queue::write_buffer`;
        // 35 a frame on Mount Eddie, 40 µs each at the median while the prepare systems write in parallel, traced).
        // The shaders read the same bytes. wgpu's queue is the port's renderer; the console had none.
        if padded.is_empty() || (!made && *padded == *self.written) {
            return false;
        }
        queue.write_buffer(buffer, 0, &padded);
        self.written.clear();
        self.written.extend_from_slice(&padded);
        true
    }

    pub fn binding(&self) -> Option<BindingResource<'_>> {
        self.buffer.as_ref().map(|b| b.as_entire_binding())
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }
}

impl<'a, T: ShaderType + WriteInto> IntoBinding<'a> for &'a QueuedUniform<T> {
    fn into_binding(self) -> BindingResource<'a> {
        self.binding().expect("a uniform is written before it is bound")
    }
}

fn padded_contents(contents: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let size = contents.len().next_multiple_of(COPY_BUFFER_ALIGNMENT as usize);
    if size == contents.len() {
        return std::borrow::Cow::Borrowed(contents);
    }
    let mut padded = contents.to_vec();
    padded.resize(size, 0);
    std::borrow::Cow::Owned(padded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_are_padded_to_whole_words_as_create_buffer_init_sizes_them() {
        assert!(padded_contents(&[]).is_empty());
        assert_eq!(&*padded_contents(&[1, 2, 3, 4]), &[1, 2, 3, 4]);
        assert_eq!(&*padded_contents(&[1, 2, 3, 4, 5, 6]), &[1, 2, 3, 4, 5, 6, 0, 0]);
        assert_eq!(&*padded_contents(&[7]), &[7, 0, 0, 0]);
        let indices: Vec<u8> = [1u16, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
        assert_eq!(padded_contents(&indices).len(), 8);
    }

    fn device_and_queue() -> (RenderDevice, RenderQueue) {
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
        let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("noop adapter");
        let (device, queue) =
            bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("noop device");
        (RenderDevice::from(device), RenderQueue::new(queue))
    }

    #[test]
    fn a_uniform_is_written_when_its_bytes_differ_from_the_buffers() {
        let (device, queue) = device_and_queue();
        let mut uniform = QueuedUniform::<bevy::math::Vec4>::default();
        let a = bevy::math::Vec4::new(1.0, 2.0, 3.0, 4.0);
        assert!(uniform.write(&device, &queue, "test", &a), "the first write makes and fills the buffer");
        let buffer = uniform.buffer().expect("made").clone();
        assert!(!uniform.write(&device, &queue, "test", &a), "the same bytes: not written again");
        assert!(uniform.write(&device, &queue, "test", &(a * 2.0)), "other bytes: written");
        assert!(uniform.write(&device, &queue, "test", &a), "back to the first value: written");
        assert!(!uniform.write(&device, &queue, "test", &a));
        assert_eq!(uniform.buffer().map(|b| b.id()), Some(buffer.id()), "one buffer for one size");
    }
}
