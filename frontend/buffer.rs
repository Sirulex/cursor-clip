use memmap2::{MmapMut, MmapOptions};
use std::fs::OpenOptions;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;
use wayland_client::{protocol::{wl_shm, wl_shm_pool, wl_buffer}, QueueHandle};

use crate::frontend::state::State;

pub fn create_shared_buffer(
    shm: &wl_shm::WlShm,
    width: u32,
    height: u32,
    qhandle: &QueueHandle<State>,
) -> Result<(wl_shm_pool::WlShmPool, wl_buffer::WlBuffer), Box<dyn std::error::Error>> {
    let stride = width * 4; // 4 bytes per pixel (ARGB8888)
    let size = stride * height;

    let path = "/dev/shm/wayland-shared-buffer";
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;

    file.set_len(size as u64)?;

    let mut mmap: MmapMut = unsafe { MmapOptions::new().len(size as usize).map_mut(&file)? };

    // Fill with blue color
    for pixel in mmap.chunks_exact_mut(4) {
        pixel[0] = 0xFF; // Blue
        pixel[1] = 0x00; // Green
        pixel[2] = 0x00; // Red
        pixel[3] = 0x00; // Alpha 0xFF
    }

    let fd = file.as_raw_fd();
    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };

    // Create a pool from the file descriptor
    let pool = shm.create_pool(borrowed_fd, size as i32, qhandle, ());

    // Create a buffer from the pool
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qhandle,
        (),
    );

    Ok((pool, buffer))
}

pub fn create_red_buffer(
    shm: &wl_shm::WlShm,
    width: u32,
    height: u32,
    qhandle: &QueueHandle<State>,
) -> Result<(wl_shm_pool::WlShmPool, wl_buffer::WlBuffer), Box<dyn std::error::Error>> {
    let stride = width * 4; // 4 bytes per pixel (ARGB8888)
    let size = stride * height;

    let path = "/dev/shm/wayland-shared-buffer-red";
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;

    file.set_len(size as u64)?;

    let mut mmap: MmapMut = unsafe { MmapOptions::new().len(size as usize).map_mut(&file)? };

    // Fill with red color
    for pixel in mmap.chunks_exact_mut(4) {
        pixel[0] = 0x00; // Blue
        pixel[1] = 0x00; // Green
        pixel[2] = 0xFF; // Red
        pixel[3] = 0x00; // Alpha
    }

    let fd = file.as_raw_fd();
    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };

    // Create a pool from the file descriptor
    let pool = shm.create_pool(borrowed_fd, size as i32, qhandle, ());

    // Create a buffer from the pool
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qhandle,
        (),
    );

    Ok((pool, buffer))
}
