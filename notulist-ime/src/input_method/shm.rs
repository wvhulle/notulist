use std::collections::VecDeque;
use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;

use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Dispatch, QueueHandle};

const SLOT_COUNT: usize = 2;

pub fn rgba_to_argb(src: &[u8], dst: &mut [u8]) {
    dst.as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
        .for_each(|(out, [red, green, blue, alpha])| {
            *out = [*blue, *green, *red, *alpha];
        });
}

pub struct ShmDoubleBuffer {
    file: File,
    _pool: WlShmPool,
    buffers: Vec<WlBuffer>,
    free: VecDeque<usize>,
    slot_bytes: usize,
}

impl ShmDoubleBuffer {
    pub fn new<D>(
        shm: &WlShm,
        queue_handle: &QueueHandle<D>,
        width: i32,
        height: i32,
    ) -> std::io::Result<Self>
    where
        D: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let stride = width * 4;
        let slot_bytes = (stride * height) as usize;
        let total_bytes = (slot_bytes * SLOT_COUNT) as i32;
        let file = tempfile::tempfile()?;
        file.set_len(total_bytes as u64)?;
        let pool = shm.create_pool(file.as_fd(), total_bytes, queue_handle, ());
        let buffers = (0..SLOT_COUNT)
            .map(|slot| {
                pool.create_buffer(
                    (slot * slot_bytes) as i32,
                    width,
                    height,
                    stride,
                    wl_shm::Format::Argb8888,
                    queue_handle,
                    (),
                )
            })
            .collect();
        Ok(Self {
            file,
            _pool: pool,
            buffers,
            free: (0..SLOT_COUNT).collect(),
            slot_bytes,
        })
    }

    pub fn acquire(&mut self) -> Option<usize> {
        self.free.pop_front()
    }

    pub fn buffer(&self, slot: usize) -> &WlBuffer {
        &self.buffers[slot]
    }

    pub fn mark_released(&mut self, buffer: &WlBuffer) {
        let released = self
            .buffers
            .iter()
            .position(|candidate| candidate == buffer);
        if let Some(slot) = released.filter(|slot| !self.free.contains(slot)) {
            self.free.push_back(slot);
        }
    }

    pub fn write_rgba(&mut self, slot: usize, rgba: &[u8]) -> std::io::Result<()> {
        let mut argb = vec![0u8; self.slot_bytes];
        rgba_to_argb(&rgba[..self.slot_bytes.min(rgba.len())], &mut argb);
        self.file
            .write_all_at(&argb, (slot * self.slot_bytes) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swizzle_moves_red_into_least_significant_byte() {
        let src = [0xaa, 0xbb, 0xcc, 0xff, 0x11, 0x22, 0x33, 0x44];
        let mut dst = [0u8; 8];
        rgba_to_argb(&src, &mut dst);
        assert_eq!(dst, [0xcc, 0xbb, 0xaa, 0xff, 0x33, 0x22, 0x11, 0x44]);
    }

    #[test]
    fn swizzle_ignores_trailing_partial_pixel() {
        let src = [0x01, 0x02, 0x03];
        let mut dst = [0u8; 4];
        rgba_to_argb(&src, &mut dst);
        assert_eq!(dst, [0, 0, 0, 0]);
    }
}
