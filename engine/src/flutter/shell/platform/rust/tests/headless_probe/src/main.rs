//! Phase 0 only: one engine/root isolate, no display server or native window.
use flutter_shell_core::*;
use flutter_shell_wgpu::{GpuContext, OffscreenBroker};
use std::{
    collections::BTreeMap,
    ffi::{CString, c_void},
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn FlutterRustShellGetEngineAbiVersion() -> u32;
    fn FlutterRustShellCreateTaskRunner(callbacks: FlutterRustTaskRunnerCallbacks) -> *mut c_void;
    fn FlutterRustShellRunTask(runner: *mut c_void, baton: u64) -> i32;
    fn FlutterRustShellDestroyTaskRunner(runner: *mut c_void);
    fn FlutterRustShellCreateShell(
        runner: *mut c_void,
        context: FlutterRustVulkanContextData,
        presentation: FlutterRustVulkanPresentationCallbacks,
        messages: FlutterRustPlatformMessageCallbacks,
        vsync: FlutterRustVsyncCallbacks,
        windows: FlutterRustWindowingCallbacks,
        settings: FlutterRustShellSettings,
    ) -> *mut c_void;
    fn FlutterRustShellRunShell(shell: *mut c_void) -> i32;
    fn FlutterRustShellOnVsync(shell: *mut c_void, interval: u64);
    fn FlutterRustShellSendLifecycleEvent(shell: *mut c_void, state: u32);
    fn FlutterRustShellSetViewportMetrics(
        shell: *mut c_void,
        view: FlutterRustViewId,
        metrics: FlutterRustViewMetrics,
    );
    fn FlutterRustShellRegisterExternalTexture(
        shell: *mut c_void,
        callbacks: FlutterRustExternalTextureCallbacks,
    ) -> i64;
    fn FlutterRustShellUnregisterExternalTexture(
        shell: *mut c_void,
        id: i64,
        callback: extern "C" fn(*mut c_void),
        data: *mut c_void,
    );
    fn FlutterRustShellDestroyShell(shell: *mut c_void);
    fn FlutterRustShellLockQueue(shell: *mut c_void);
    fn FlutterRustShellUnlockQueue(shell: *mut c_void);
}
struct Host {
    tasks: Mutex<BTreeMap<Instant, Vec<(usize, u64)>>>,
    wake: Condvar,
    thread: std::thread::ThreadId,
    vsync: std::sync::atomic::AtomicBool,
}
extern "C" fn schedule(data: *mut c_void, runner: *mut c_void, baton: u64, delay: u64) {
    let host = unsafe { &*data.cast::<Host>() };
    let deadline = Instant::now()
        .checked_add(Duration::from_nanos(delay))
        .unwrap_or_else(Instant::now);
    host.tasks
        .lock()
        .unwrap()
        .entry(deadline)
        .or_default()
        .push((runner as usize, baton));
    host.wake.notify_one();
}
extern "C" fn on_thread(data: *mut c_void) -> i32 {
    i32::from(unsafe { &*data.cast::<Host>() }.thread == std::thread::current().id())
}
extern "C" fn request_vsync(data: *mut c_void) {
    let host = unsafe { &*data.cast::<Host>() };
    host.vsync.store(true, std::sync::atomic::Ordering::Release);
    host.wake.notify_one();
}
extern "C" fn destroyed(_: *mut c_void) {}
unsafe extern "C" fn lock(data: *mut c_void) {
    unsafe { FlutterRustShellLockQueue(data) };
}
unsafe extern "C" fn unlock(data: *mut c_void) {
    unsafe { FlutterRustShellUnlockQueue(data) };
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none(),
        "unset display variables"
    );
    if unsafe { FlutterRustShellGetEngineAbiVersion() } != SHELL_ABI_VERSION {
        return Err("engine/runner ABI mismatch".into());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: flutter-rust-headless-probe ASSETS ICU ARTIFACT_DIR".into());
    }
    std::fs::create_dir_all(&args[2])?;
    let assets = CString::new(args[0].as_str())?;
    let icu = CString::new(args[1].as_str())?;
    let context = GpuContext::headless()?;
    eprintln!("headless Flutter probe: {:?}", context.adapter_info());
    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    let broker = OffscreenBroker::new(context.clone(), tx);
    let host = Box::new(Host {
        tasks: Mutex::new(BTreeMap::new()),
        wake: Condvar::new(),
        thread: std::thread::current().id(),
        vsync: std::sync::atomic::AtomicBool::new(false),
    });
    let runner = unsafe {
        FlutterRustShellCreateTaskRunner(FlutterRustTaskRunnerCallbacks {
            user_data: (&*host as *const Host).cast_mut().cast(),
            schedule_task: Some(schedule),
            runs_tasks_on_current_thread: Some(on_thread),
            task_runner_destroyed: Some(destroyed),
        })
    };
    if runner.is_null() {
        return Err("task runner creation failed".into());
    }
    let shell = context
        .with_vulkan_context(|raw| {
            let ie: Vec<_> = raw
                .instance_extensions
                .iter()
                .map(|x| CString::new(x.as_str()).unwrap())
                .collect();
            let de: Vec<_> = raw
                .device_extensions
                .iter()
                .map(|x| CString::new(x.as_str()).unwrap())
                .collect();
            let ip: Vec<_> = ie.iter().map(|x| x.as_ptr()).collect();
            let dp: Vec<_> = de.iter().map(|x| x.as_ptr()).collect();
            unsafe {
                FlutterRustShellCreateShell(
                    runner,
                    FlutterRustVulkanContextData {
                        get_instance_proc_addr: raw.get_instance_proc_addr as *mut c_void,
                        instance: raw.instance as *mut c_void,
                        physical_device: raw.physical_device as *mut c_void,
                        device: raw.device as *mut c_void,
                        queue: raw.queue as *mut c_void,
                        queue_family_index: raw.queue_family_index,
                        instance_extensions: ip.as_ptr(),
                        instance_extensions_count: ip.len() as u32,
                        device_extensions: dp.as_ptr(),
                        device_extensions_count: dp.len() as u32,
                    },
                    broker.callbacks(),
                    // All-null callback tables mean explicitly unsupported native services.
                    std::mem::zeroed(),
                    FlutterRustVsyncCallbacks {
                        user_data: (&*host as *const Host).cast_mut().cast(),
                        request_vsync: Some(request_vsync),
                    },
                    std::mem::zeroed(),
                    FlutterRustShellSettings {
                        assets_path: assets.as_ptr(),
                        icu_data_path: icu.as_ptr(),
                        aot_library_path: std::ptr::null(),
                    },
                )
            }
        })
        .ok_or("Vulkan extraction failed")?;
    if shell.is_null() {
        unsafe {
            FlutterRustShellDestroyTaskRunner(runner);
        }
        return Err("private Flutter engine creation failed".into());
    }
    unsafe {
        context.install_queue_lock(flutter_gpu_queue::QueueLockHooks {
            user_data: shell,
            lock,
            unlock,
        });
    }
    let ring = context.create_texture_ring(80, 80)?;
    let texture_id = unsafe { FlutterRustShellRegisterExternalTexture(shell, ring.callbacks()) };
    assert_eq!(texture_id, 1);
    ring.request_clear([1., 1., 0., 1.]);
    let result = run(shell, &host, &rx, &args[2]);
    ring.shutdown();
    let (released, completion) = std::sync::mpsc::channel();
    let release = Box::new(TextureRelease { ring, released });
    unsafe {
        FlutterRustShellUnregisterExternalTexture(
            shell,
            texture_id,
            release_texture,
            Box::into_raw(release).cast(),
        );
    }
    let release_result = completion.recv_timeout(Duration::from_secs(10));
    // Stop engine/raster before dropping callback owners and GPU targets. Keep
    // queue hooks installed until every engine GPU submission has stopped.
    unsafe {
        FlutterRustShellDestroyShell(shell);
    }
    context.clear_queue_lock();
    unsafe {
        FlutterRustShellDestroyTaskRunner(runner);
    }
    release_result?;
    result
}

struct TextureRelease {
    ring: Box<flutter_shell_wgpu::WgpuTextureRing>,
    released: std::sync::mpsc::Sender<()>,
}
extern "C" fn release_texture(data: *mut c_void) {
    let release = unsafe { Box::from_raw(data.cast::<TextureRelease>()) };
    release.ring.release_gpu_resources();
    let _ = release.released.send(());
}

fn resize(shell: *mut c_void, width: f64, height: f64, ratio: f64) {
    unsafe {
        FlutterRustShellSetViewportMetrics(
            shell,
            FlutterRustViewId::IMPLICIT,
            FlutterRustViewMetrics {
                width,
                height,
                min_width: width,
                max_width: width,
                min_height: height,
                max_height: height,
                pixel_ratio: ratio,
                display_width: 1920.,
                display_height: 1080.,
                display_refresh_rate: 60.,
            },
        );
    }
}

fn run(
    shell: *mut c_void,
    host: &Host,
    frames: &std::sync::mpsc::Receiver<Result<flutter_shell_wgpu::OffscreenFrame, String>>,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if unsafe { FlutterRustShellRunShell(shell) } == 0 {
        return Err("Flutter isolate failed to start".into());
    }
    unsafe {
        FlutterRustShellSendLifecycleEvent(shell, 1);
    }
    let sizes = [(801., 601., 1.), (1001., 751., 1.25), (801., 601., 1.)];
    let mut size_index = 0;
    let mut accepted = 0;
    let mut markers = std::collections::BTreeSet::new();
    resize(shell, sizes[0].0, sizes[0].1, sizes[0].2);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut next_pulse = Instant::now();
    loop {
        if Instant::now() >= next_pulse
            && host.vsync.swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            unsafe {
                FlutterRustShellOnVsync(shell, 16_666_667);
            }
            next_pulse = Instant::now() + Duration::from_nanos(16_666_667);
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for a Flutter frame".into());
        }
        if let Ok(frame) = frames.try_recv() {
            let frame = frame?;
            let (width, height, ratio) = sizes[size_index];
            if frame.width != width as u32 || frame.height != height as u32 {
                continue;
            }
            accepted += 1;
            let path = std::path::Path::new(output);
            std::fs::write(
                path.join(format!(
                    "frame-{}-{}x{}.rgba",
                    frame.frame, frame.width, frame.height
                )),
                &frame.rgba,
            )?;
            // The render_probe fixture has a known background and six stripes.
            let pixel = |x: usize, y: usize| {
                let x = (x as f64 * ratio) as usize;
                let y = (y as f64 * ratio) as usize;
                &frame.rgba[(y * frame.width as usize + x) * 4..][..4]
            };
            if pixel(5, 5) != [17, 34, 51, 255] {
                return Err(format!("unexpected background: {:?}", pixel(5, 5)).into());
            }
            for (i, expected) in [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
                [0, 255, 255, 255],
                [255, 0, 255, 255],
            ]
            .iter()
            .enumerate()
            {
                if pixel(40, 40 + i * 60) != expected {
                    return Err(format!("stripe {i}: {:?}", pixel(40, 40 + i * 60)).into());
                }
            }
            eprintln!(
                "Flutter frame {} generation {}: {}x{} pixel assertions passed",
                frame.frame, frame.generation, frame.width, frame.height
            );
            if pixel(340, 40) != [255, 255, 0, 255] {
                return Err(format!("plugin texture: {:?}", pixel(340, 40)).into());
            }
            if pixel(440, 40) != [128, 128, 128, 128] {
                return Err(format!("premultiplied alpha: {:?}", pixel(440, 40)).into());
            }
            let marker_offset = ((frame.width - 5) * 4) as usize;
            markers.insert(frame.rgba[marker_offset..marker_offset + 4].to_vec());
            if accepted == 4 {
                if markers.len() != 2 {
                    return Err("animated marker did not change across frames".into());
                }
                size_index += 1;
                if size_index == sizes.len() {
                    return Ok(());
                }
                accepted = 0;
                markers.clear();
                let (w, h, r) = sizes[size_index];
                resize(shell, w, h, r);
            }
        }
        let mut tasks = host.tasks.lock().unwrap();
        let now = Instant::now();
        let mut due = Vec::new();
        while tasks.first_key_value().is_some_and(|(t, _)| *t <= now) && due.len() < 64 {
            due.extend(tasks.pop_first().unwrap().1);
        }
        if due.is_empty() {
            let next = tasks
                .first_key_value()
                .map(|(t, _)| *t)
                .unwrap_or(deadline)
                .min(now + Duration::from_millis(16));
            let _ = host
                .wake
                .wait_timeout(tasks, next.saturating_duration_since(now))
                .unwrap();
        } else {
            drop(tasks);
            for (runner, baton) in due {
                unsafe {
                    FlutterRustShellRunTask(runner as *mut c_void, baton);
                }
            }
        }
    }
}
