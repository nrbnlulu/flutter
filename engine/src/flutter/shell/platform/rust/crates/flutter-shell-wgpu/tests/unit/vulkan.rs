use super::*;

#[test]
fn context_values_are_plain_owned_data() {
    let context = VulkanContextData {
        get_instance_proc_addr: 1,
        instance: 2,
        physical_device: 3,
        device: 4,
        queue: 5,
        queue_family_index: 6,
        instance_extensions: vec!["VK_KHR_surface".to_owned()],
        device_extensions: vec!["VK_KHR_swapchain".to_owned()],
    };
    assert_eq!(context.queue_family_index, 6);
    assert_eq!(context.instance_extensions.len(), 1);
}

#[test]
fn available_slots_apply_backpressure_and_reuse_returns() {
    let slots = AvailableSlots::new(3);
    assert_eq!(slots.try_take(), Ok(0));
    assert_eq!(slots.try_take(), Ok(1));
    assert_eq!(slots.try_take(), Ok(2));
    assert_eq!(slots.try_take(), Err(PluginError::Busy));
    slots.give_back(1);
    assert_eq!(slots.try_take(), Ok(1));
}

#[test]
fn shutdown_wakes_an_async_slot_waiter() {
    let slots = Arc::new(AvailableSlots::new(1));
    assert_eq!(slots.try_take(), Ok(0));
    let waiter_slots = Arc::clone(&slots);
    let waiter = std::thread::spawn(move || pollster::block_on(waiter_slots.take()));

    slots.shutdown();

    assert_eq!(
        waiter.join().expect("slot waiter panicked"),
        Err(PluginError::Shutdown)
    );
    slots.give_back(0);
    assert_eq!(slots.try_take(), Err(PluginError::Shutdown));
}

#[test]
fn upload_recycles_only_after_mapping_and_flutter_release() {
    let upload = UploadRecycle::new();
    upload.begin();
    assert!(!upload.mapped());
    assert!(upload.released());

    upload.begin();
    assert!(!upload.released());
    assert!(upload.mapped());
    assert!(!upload.mapped());

    for _ in 0..32 {
        upload.begin();
        let (mapped, released) = std::thread::scope(|scope| {
            let mapped = scope.spawn(|| upload.mapped());
            let released = upload.released();
            (mapped.join().unwrap(), released)
        });
        assert_ne!(mapped, released, "exactly one side must recycle the slot");
    }
}

#[test]
fn panicking_pixel_writer_restores_its_reservation() {
    let state = AtomicU8::new(FRAME_WRITING);
    let result = std::panic::catch_unwind(|| {
        let _reservation = PixelWriteReservation(&state);
        panic!("plugin failed while writing");
    });
    assert!(result.is_err());
    assert_eq!(state.load(Ordering::Acquire), FRAME_RESERVED);
}

// Models Flutter's semaphore consumption without a window or Impeller.
fn consume_pixel_frame(ring: &WgpuTextureRing) -> std::time::Duration {
    consume_pixel_frame_with_readback(ring, None)
}

fn consume_pixel_frame_with_readback(
    ring: &WgpuTextureRing,
    readback: Option<&wgpu::Buffer>,
) -> std::time::Duration {
    let callbacks = ring.callbacks();
    let mut frame = std::mem::MaybeUninit::uninit();
    let started = std::time::Instant::now();
    assert_eq!(
        callbacks.acquire_frame.unwrap()(callbacks.user_data, 0, 0, frame.as_mut_ptr()),
        1
    );
    let acquire_time = started.elapsed();
    // SAFETY: the successful acquire above initialized the output frame.
    let frame = unsafe { frame.assume_init() };
    let commands = readback.map(|buffer| {
        let state = ring.inner.state.lock();
        let slot = state
            .slots
            .iter()
            .find(|slot| slot.image.as_raw() == frame.image)
            .unwrap();
        let row_bytes = slot.pixels.as_ref().unwrap().lock().upload_row_bytes;
        let mut encoder = ring
            .inner
            .context
            .device
            .create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            slot.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(frame.height),
                },
            },
            slot.texture.size(),
        );
        encoder.finish()
    });
    assert!(
        ring.inner
            .context
            .queue
            .submit_with_semaphores(
                &[flutter_gpu_queue::WaitSemaphore {
                    semaphore: ash::vk::Semaphore::from_raw(frame.acquire_semaphore),
                    stage: ash::vk::PipelineStageFlags::ALL_COMMANDS,
                }],
                &[ash::vk::Semaphore::from_raw(frame.render_semaphore)],
                commands,
            )
            .is_some()
    );
    callbacks.release_frame.unwrap()(callbacks.user_data, frame);
    acquire_time
}

#[test]
#[ignore = "requires a Vulkan device; verifies uploaded pixels by GPU readback"]
fn direct_pixel_upload_preserves_padded_rows_and_can_mix_with_staged_writes() {
    let context = GpuContext::create(Headless).unwrap();
    for width in [1, 64, 65, 257] {
        let height = 3;
        let ring = context
            .create_pixel_buffer_texture_ring(width, height)
            .unwrap();
        ring.set_registration(1, |_| Ok(()));
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pixel texture readback"),
            size: u64::from(row_bytes * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        for value in 0..12 {
            let frame =
                pollster::block_on(PixelBufferTextureBackendHandle::next_frame(&*ring)).unwrap();
            if value % 2 == 0 {
                let before: Vec<_> = ring
                    .inner
                    .state
                    .lock()
                    .slots
                    .iter()
                    .map(|slot| slot.pixels.as_ref().unwrap().lock().pixels.clone())
                    .collect();
                frame
                    .write_pixels_direct(Box::new(move |mut pixels, stride| {
                        assert_eq!(stride, row_bytes as usize);
                        assert_eq!(pixels.len(), stride * height as usize);
                        for y in 0..height {
                            for x in 0..width {
                                let offset = y as usize * stride + x as usize * 4;
                                pixels
                                    .slice(offset..offset + 4)
                                    .copy_from_slice(&[x as u8, y as u8, value, 255]);
                            }
                        }
                    }))
                    .unwrap();
                // Direct writes must bypass the CPU shadow buffers entirely.
                for (slot, old) in ring.inner.state.lock().slots.iter().zip(before) {
                    assert_eq!(slot.pixels.as_ref().unwrap().lock().pixels, old);
                }
            } else {
                frame
                    .write_pixels(Box::new(move |pixels, stride| {
                        assert_eq!(stride, width as usize * 4);
                        for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                            pixel.copy_from_slice(&[
                                (i % width as usize) as u8,
                                (i / width as usize) as u8,
                                value,
                                255,
                            ]);
                        }
                    }))
                    .unwrap();
            }
            frame.present().unwrap();
            consume_pixel_frame_with_readback(&ring, Some(&readback));
            let (sender, receiver) = std::sync::mpsc::channel();
            readback.map_async(wgpu::MapMode::Read, .., move |result| {
                sender.send(result).unwrap()
            });
            context
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            receiver.recv().unwrap().unwrap();
            {
                let pixels = readback.get_mapped_range(..).unwrap();
                for y in 0..height {
                    for x in 0..width {
                        let offset = (y * row_bytes + x * 4) as usize;
                        assert_eq!(&pixels[offset..offset + 4], &[x as u8, y as u8, value, 255]);
                    }
                }
            }
            readback.unmap();
        }
        ring.release_gpu_resources();
    }
}

#[test]
#[ignore = "requires a Vulkan device; run explicitly with Vulkan validation"]
fn pixel_upload_reuses_aligned_and_unaligned_slots() {
    let context = GpuContext::create(Headless).unwrap();
    for width in [64, 65] {
        let ring = context.create_pixel_buffer_texture_ring(width, 17).unwrap();
        ring.set_registration(1, |_| Ok(()));
        let frame = PixelBufferTextureBackendHandle::try_next_frame(&*ring).unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                frame.write_pixels(Box::new(|_, _| panic!("plugin writer panic")))
            }))
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                frame.write_pixels_direct(Box::new(|_, _| panic!("direct writer panic")))
            }))
            .is_err()
        );
        frame
            .write_pixels(Box::new(|pixels, _| pixels.fill(255)))
            .unwrap();
        frame.present().unwrap();
        consume_pixel_frame(&ring);
        for value in 0..24 {
            let frame =
                pollster::block_on(PixelBufferTextureBackendHandle::next_frame(&*ring)).unwrap();
            frame
                .write_pixels(Box::new(move |pixels, row_bytes| {
                    assert_eq!(row_bytes, width as usize * 4);
                    assert_eq!(pixels.len(), row_bytes * 17);
                    pixels.fill(value);
                }))
                .unwrap();
            frame.present().unwrap();
            consume_pixel_frame(&ring);
        }
        // Unregister with mappings potentially still pending. Completion
        // callbacks must never resurrect storage or block raster teardown.
        ring.release_gpu_resources();
        assert!(ring.inner.state.lock().slots.is_empty());
        assert!(matches!(
            PixelBufferTextureBackendHandle::try_next_frame(&*ring),
            Err(PluginError::Shutdown)
        ));
    }
}

#[test]
#[ignore = "requires a Vulkan device"]
fn slow_pixel_writer_allows_other_slots_and_shutdown() {
    let context = GpuContext::create(Headless).unwrap();
    for direct in [false, true] {
        let ring = context.create_pixel_buffer_texture_ring(65, 17).unwrap();
        ring.set_registration(1, |_| Ok(()));
        let frame = PixelBufferTextureBackendHandle::try_next_frame(&*ring).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let writer_frame = Arc::clone(&frame);
        let writer = std::thread::spawn(move || {
            let wait = move || {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
            };
            if direct {
                writer_frame.write_pixels_direct(Box::new(move |mut pixels, _| {
                    wait();
                    pixels.fill(42);
                }))
            } else {
                writer_frame.write_pixels(Box::new(move |pixels, _| {
                    wait();
                    pixels.fill(42);
                }))
            }
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            ring.inner.state.try_lock().is_some(),
            "plugin work held the ring mutex"
        );
        assert_eq!(frame.present(), Err(PluginError::Busy));
        let other_frame = PixelBufferTextureBackendHandle::try_next_frame(&*ring).unwrap();
        other_frame
            .write_pixels(Box::new(|pixels, _| pixels.fill(24)))
            .unwrap();
        other_frame.present().unwrap();
        consume_pixel_frame(&ring);
        ring.release_gpu_resources();
        finish_tx.send(()).unwrap();
        assert_eq!(writer.join().unwrap(), Err(PluginError::Shutdown));
        assert!(ring.inner.state.lock().slots.is_empty());
    }
}

#[test]
#[ignore = "manual Vulkan upload benchmark; run with --ignored --nocapture"]
fn benchmark_pixel_upload() {
    let context = GpuContext::create(Headless).unwrap();
    eprintln!("GPU: {:?}", context.adapter.get_info());
    for (width, height) in [(1920, 1080), (3840, 2160)] {
        for mode in ["write_texture", "staged", "direct"] {
            let ring = context
                .create_pixel_buffer_texture_ring(width, height)
                .unwrap();
            ring.set_registration(1, |_| Ok(()));
            if mode == "write_texture" {
                for slot in &ring.inner.state.lock().slots {
                    slot.pixels.as_ref().unwrap().lock().upload.take();
                }
            }
            let mut acquire_times = Vec::new();
            let mut producer_times = Vec::new();
            let started = std::time::Instant::now();
            for value in 0..90 {
                let frame = pollster::block_on(PixelBufferTextureBackendHandle::next_frame(&*ring))
                    .unwrap();
                let producer_started = std::time::Instant::now();
                if mode == "direct" {
                    frame
                        .write_pixels_direct(Box::new(move |mut pixels, _| pixels.fill(value)))
                        .unwrap();
                } else {
                    frame
                        .write_pixels(Box::new(move |pixels, _| pixels.fill(value)))
                        .unwrap();
                }
                let producer_time = producer_started.elapsed();
                frame.present().unwrap();
                let acquire_time = consume_pixel_frame(&ring);
                if value >= 10 {
                    acquire_times.push(acquire_time.as_micros());
                    producer_times.push(producer_time.as_micros());
                }
            }
            let elapsed = started.elapsed();
            ring.release_gpu_resources();
            acquire_times.sort_unstable();
            producer_times.sort_unstable();
            eprintln!(
                "{width}x{height} {mode}: raster p50={}us p95={}us, producer p50={}us p95={}us, 90 frames={elapsed:?}",
                acquire_times[40], acquire_times[76], producer_times[40], producer_times[76]
            );
        }
    }
}
