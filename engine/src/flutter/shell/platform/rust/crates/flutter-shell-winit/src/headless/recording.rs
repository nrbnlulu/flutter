//! Bounded per-view FFmpeg recording. No encoder work runs on UI/raster threads.
use super::*;
use flutter_shell_agent::AgentError;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    process::{Child, Command, Stdio},
    sync::{
        atomic::AtomicU64,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

pub(super) struct Sink {
    sender: SyncSender<Arc<OffscreenFrame>>,
    pub minimum_ui: AtomicU64,
    stop: AtomicBool,
    reason: Mutex<String>,
    dropped: AtomicU64,
    received: AtomicU64,
    state: Mutex<Value>,
}
impl Sink {
    pub fn publish(&self, frame: Arc<OffscreenFrame>) {
        if self.stop.load(Ordering::Acquire)
            || frame.ui_frame_number < self.minimum_ui.load(Ordering::Acquire)
        {
            return;
        }
        self.received.fetch_add(1, Ordering::Relaxed);
        if self.sender.try_send(frame).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn stop(&self, reason: &str) {
        if !self.stop.swap(true, Ordering::AcqRel) {
            *self.reason.lock() = reason.into();
        }
    }
}
pub(super) struct Recording {
    pub sink: Arc<Sink>,
    worker: Option<JoinHandle<()>>,
    pub view: i64,
}
impl Recording {
    pub fn start(
        directory: PathBuf,
        name: String,
        view: i64,
        rate: u32,
        start: Instant,
    ) -> Result<Self, AgentError> {
        if rate == 0 || rate > 60 {
            return Err(AgentError::new(
                "invalid_request",
                "recording rate must be 1..=60",
            ));
        }
        let executable = std::env::var_os("FLUTTER_RUST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let mut probe = Command::new(&executable)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                AgentError::new("unsupported_capability", format!("FFmpeg unavailable: {e}"))
            })?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if probe
                .try_wait()
                .map_err(|e| AgentError::new("unsupported_capability", e.to_string()))?
                .is_some()
            {
                break;
            }
            if Instant::now() >= deadline {
                let _ = probe.kill();
                let _ = probe.wait();
                return Err(AgentError::new(
                    "unsupported_capability",
                    "FFmpeg codec discovery timed out",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
        let output = probe
            .wait_with_output()
            .map_err(|e| AgentError::new("unsupported_capability", e.to_string()))?;
        if !output.status.success()
            || !String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some("ffv1"))
        {
            return Err(AgentError::new(
                "unsupported_capability",
                "FFmpeg does not provide the lossless ffv1 codec",
            ));
        }
        let directory = directory.join(format!("recording-{name}"));
        std::fs::create_dir(&directory)
            .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let sink = Arc::new(Sink {
            sender,
            minimum_ui: AtomicU64::new(u64::MAX),
            stop: AtomicBool::new(false),
            reason: Mutex::new("requested".into()),
            dropped: AtomicU64::new(0),
            received: AtomicU64::new(0),
            state: Mutex::new(
                json!({"name":name,"viewId":view,"status":"starting","rate":rate,"directory":directory,"segments":[]}),
            ),
        });
        let shared = sink.clone();
        let worker = thread::Builder::new()
            .name(format!("record-view-{view}"))
            .spawn(move || {
                let result = record(&shared, receiver, &directory, &executable, rate, start);
                let mut state = shared.state.lock();
                state["status"] = json!(if result.is_ok() { "complete" } else { "failed" });
                state["reason"] = json!(*shared.reason.lock());
                state["captureQueueDropped"] = json!(shared.dropped.load(Ordering::Relaxed));
                state["receivedFrames"] = json!(shared.received.load(Ordering::Relaxed));
                state["endedSessionMicros"] = json!(start.elapsed().as_micros() as u64);
                if let Err(error) = result {
                    state["error"] = json!(error.to_string());
                    state["partial"] = json!(true);
                }
                if let Err(error) = write_manifest(&directory, &state) {
                    state["status"] = json!("failed");
                    state["manifestError"] = json!(error.to_string());
                }
            })
            .map_err(|e| AgentError::new("recording_failed", e.to_string()))?;
        Ok(Self {
            sink,
            worker: Some(worker),
            view,
        })
    }
    pub fn status(&self) -> Value {
        self.sink.state.lock().clone()
    }
    pub fn is_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub fn finish(&mut self) -> Result<Value, AgentError> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| AgentError::new("recording_failed", "recording worker panicked"))?;
        }
        let status = self.status();
        if status["status"] == "failed" {
            return Err(AgentError::new(
                "recording_failed",
                "recording has partial or missing evidence",
            )
            .with_data(status));
        }
        Ok(status)
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        self.sink.stop("session_closed");
        let _ = self.finish();
    }
}
fn write_manifest(directory: &std::path::Path, value: &Value) -> std::io::Result<()> {
    let partial = directory.join("manifest.json.partial");
    std::fs::write(&partial, serde_json::to_vec_pretty(value).unwrap())?;
    std::fs::rename(partial, directory.join("manifest.json"))
}
struct Encoder {
    child: Child,
    sender: Option<SyncSender<(Arc<OffscreenFrame>, u64)>>,
    writer: Option<JoinHandle<std::io::Result<()>>>,
    path: PathBuf,
    width: u32,
    height: u32,
    generation: u64,
    start: u64,
    written: u64,
    dropped: u64,
    writer_micros: Arc<AtomicU64>,
    target_frames: Arc<AtomicU64>,
    output_frames: Arc<AtomicU64>,
    rate: u32,
    finished: bool,
}
impl Encoder {
    fn new(
        directory: &std::path::Path,
        executable: &std::ffi::OsStr,
        index: usize,
        frame: &OffscreenFrame,
        rate: u32,
        start: u64,
    ) -> Result<Self, AgentError> {
        let path = directory.join(format!("segment-{index:03}.mkv"));
        let log = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join(format!("segment-{index:03}.ffmpeg.log")))
            .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
        let mut child = Command::new(executable)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba",
                "-video_size",
                &format!("{}x{}", frame.width, frame.height),
                "-framerate",
                &rate.to_string(),
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-pix_fmt",
                "bgra",
                "-color_trc",
                "linear",
                "-fs",
                "134217728",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .map_err(|e| AgentError::new("recording_failed", e.to_string()))?;
        let mut pipe = child.stdin.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel::<(Arc<OffscreenFrame>, u64)>(2);
        let writer_micros = Arc::new(AtomicU64::new(0));
        let timing = writer_micros.clone();
        let target_frames = Arc::new(AtomicU64::new(0));
        let output_frames = Arc::new(AtomicU64::new(0));
        let target = target_frames.clone();
        let progress = output_frames.clone();
        let writer = match thread::Builder::new()
            .name("ffmpeg-input".into())
            .spawn(move || {
                let mut previous: Option<Vec<u8>> = None;
                let mut index = 0;
                while let Ok((frame, position)) = receiver.recv() {
                    let started = Instant::now();
                    let mut bytes = frame.rgba.clone();
                    for pixel in bytes.chunks_exact_mut(4) {
                        let alpha = u32::from(pixel[3]);
                        for channel in &mut pixel[..3] {
                            *channel = if alpha == 0 {
                                0
                            } else {
                                ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
                            };
                        }
                    }
                    // A skipped queue item drops source detail, never elapsed time.
                    while index < position {
                        pipe.write_all(previous.as_ref().unwrap_or(&bytes))?;
                        index += 1;
                    }
                    pipe.write_all(&bytes)?;
                    index += 1;
                    progress.store(index, Ordering::Release);
                    previous = Some(bytes);
                    timing.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                }
                if let Some(previous) = previous {
                    while index < target.load(Ordering::Acquire) {
                        pipe.write_all(&previous)?;
                        index += 1;
                    }
                    progress.store(index, Ordering::Release);
                }
                Ok(())
            }) {
            Ok(worker) => worker,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentError::new("recording_failed", error.to_string()));
            }
        };
        Ok(Self {
            child,
            sender: Some(sender),
            writer: Some(writer),
            path,
            width: frame.width,
            height: frame.height,
            generation: frame.generation,
            start,
            written: 0,
            dropped: 0,
            writer_micros,
            target_frames,
            output_frames,
            rate,
            finished: false,
        })
    }
    fn tick(&mut self, frame: Arc<OffscreenFrame>, elapsed: u64) -> Result<bool, AgentError> {
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|e| AgentError::new("recording_failed", e.to_string()))?
        {
            return Err(AgentError::new(
                "recording_failed",
                format!("FFmpeg exited during recording: {status}"),
            ));
        }
        let position = ((elapsed.saturating_sub(self.start) * u64::from(self.rate)) / 1_000_000)
            .max(self.target_frames.load(Ordering::Acquire));
        self.target_frames.store(position + 1, Ordering::Release);
        match self.sender.as_ref().unwrap().try_send((frame, position)) {
            Ok(()) => {
                self.written += 1;
                Ok(true)
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.dropped += 1;
                Ok(false)
            }
            Err(_) => Err(AgentError::new(
                "recording_failed",
                "encoder input worker disconnected",
            )),
        }
    }
    fn finalize(&mut self, end: u64) -> Result<Value, AgentError> {
        self.sender.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|e| AgentError::new("recording_failed", e.to_string()))?
            {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(AgentError::new(
                    "recording_failed",
                    "FFmpeg finalization timed out",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        };
        if let Some(writer) = self.writer.take() {
            writer
                .join()
                .map_err(|_| AgentError::new("recording_failed", "FFmpeg input worker panicked"))?
                .map_err(|e| AgentError::new("recording_failed", e.to_string()))?;
        }
        self.finished = true;
        if !status.success() {
            return Err(AgentError::new(
                "recording_failed",
                format!("FFmpeg finalization failed: {status}"),
            ));
        }
        Ok(self.metadata(end))
    }
    fn metadata(&self, end: u64) -> Value {
        json!({"path":self.path,"width":self.width,"height":self.height,"sizeGeneration":self.generation,"startSessionMicros":self.start,"endSessionMicros":end,"submittedFrames":self.written,"outputFrames":self.output_frames.load(Ordering::Acquire),"targetFrames":self.target_frames.load(Ordering::Acquire),"encoderQueueDropped":self.dropped,"writerMicros":self.writer_micros.load(Ordering::Relaxed)})
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.sender.take();
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}
fn record(
    sink: &Sink,
    incoming: Receiver<Arc<OffscreenFrame>>,
    directory: &std::path::Path,
    executable: &std::ffi::OsStr,
    rate: u32,
    start: Instant,
) -> Result<(), AgentError> {
    let started = Instant::now();
    let mut timeline = File::create(directory.join("frames.jsonl"))
        .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
    let mut latest: Option<Arc<OffscreenFrame>> = None;
    let mut encoder: Option<Encoder> = None;
    let mut segments = Vec::new();
    let interval = Duration::from_secs_f64(1. / f64::from(rate));
    let mut next = Instant::now();
    let mut ticks = 0u64;
    let mut missed = 0u64;
    let outcome = (|| -> Result<(), AgentError> {
        while !sink.stop.load(Ordering::Acquire) {
            if started.elapsed() > Duration::from_secs(600) {
                return Err(AgentError::new(
                    "recording_failed",
                    "recording exceeded 600 second retention limit",
                ));
            }
            if let Some(current) = encoder.as_mut()
                && let Some(status) = current
                    .child
                    .try_wait()
                    .map_err(|e| AgentError::new("recording_failed", e.to_string()))?
            {
                return Err(AgentError::new(
                    "recording_failed",
                    format!("FFmpeg exited unexpectedly: {status}"),
                ));
            }
            match incoming.recv_timeout(
                next.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            ) {
                Ok(frame) => latest = Some(frame),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            if Instant::now() < next {
                continue;
            }
            let now = Instant::now();
            next += interval;
            while next < now {
                next += interval;
                missed += 1;
            }
            let Some(frame) = latest.as_ref() else {
                continue;
            };
            let elapsed = start.elapsed().as_micros() as u64;
            if encoder.as_ref().is_some_and(|e| {
                e.width != frame.width
                    || e.height != frame.height
                    || e.generation != frame.generation
            }) {
                let mut old = encoder.take().unwrap();
                match old.finalize(elapsed) {
                    Ok(segment) => segments.push(segment),
                    Err(error) => {
                        segments.push(old.metadata(elapsed));
                        return Err(error);
                    }
                }
            }
            if encoder.is_none() {
                if segments.len() >= 32 {
                    return Err(AgentError::new(
                        "recording_failed",
                        "recording exceeded 32 resize segments",
                    ));
                }
                encoder = Some(Encoder::new(
                    directory,
                    executable,
                    segments.len(),
                    frame,
                    rate,
                    elapsed,
                )?);
            }
            let current = encoder.as_mut().unwrap();
            let accepted = current.tick(frame.clone(), elapsed)?;
            ticks += 1;
            let point = json!({"sessionMicros":elapsed,"sourceCompletedSessionMicros":frame.completed_at.saturating_duration_since(start).as_micros() as u64,"sourceFrameId":frame.frame,"sourceUiFrame":frame.ui_frame_number,"sizeGeneration":frame.generation,"readbackMicros":frame.readback_micros,"segment":segments.len(),"accepted":accepted,"outputFrameIndex":current.target_frames.load(Ordering::Acquire).saturating_sub(1)});
            serde_json::to_writer(&mut timeline, &point)
                .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
            timeline
                .write_all(b"\n")
                .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
            let mut state = sink.state.lock();
            state["status"] = json!("recording");
            state["segments"] = json!(segments);
            state["activeSegment"] = current.metadata(elapsed);
            state["captureQueueDropped"] = json!(sink.dropped.load(Ordering::Relaxed));
            state["schedulerMissedTicks"] = json!(missed);
            if ticks.is_multiple_of(rate as u64) {
                let bytes: u64 = std::fs::read_dir(directory)
                    .map_err(|e| AgentError::new("artifact_error", e.to_string()))?
                    .filter_map(Result::ok)
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum();
                if bytes > 128 * 1024 * 1024 {
                    return Err(AgentError::new(
                        "recording_failed",
                        "recording exceeded 128 MiB retention limit",
                    ));
                }
                write_manifest(directory, &state)
                    .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
            }
        }
        Ok(())
    })();
    let finalize = if let Some(mut current) = encoder.take() {
        let end = start.elapsed().as_micros() as u64;
        let result = current.finalize(end);
        segments.push(
            result
                .as_ref()
                .ok()
                .cloned()
                .unwrap_or_else(|| current.metadata(end)),
        );
        result.map(drop)
    } else {
        Err(AgentError::new(
            "recording_failed",
            "recording ended before its first completed frame",
        ))
    };
    let mut state = sink.state.lock();
    state["segments"] = json!(segments);
    state["activeSegment"] = Value::Null;
    state["schedulerMissedTicks"] = json!(missed);
    state["clock"] = json!("session monotonic real time");
    state["colorConvention"] = json!("linear straight-alpha RGBA8, lossless FFV1 BGRA");
    outcome.and(finalize)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_capture_delivery_drops_without_waiting_and_rejects_old_ui_frames() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let sink = Sink {
            sender,
            minimum_ui: AtomicU64::new(7),
            stop: AtomicBool::new(false),
            reason: Mutex::new(String::new()),
            dropped: AtomicU64::new(0),
            received: AtomicU64::new(0),
            state: Mutex::new(Value::Null),
        };
        let frame = |ui| {
            Arc::new(OffscreenFrame {
                completed_at: Instant::now(),
                readback_micros: 1,
                ui_frame_number: ui,
                view_id: 1,
                width: 1,
                height: 1,
                generation: 1,
                frame: ui,
                rgba: vec![0, 0, 0, 255],
            })
        };
        sink.publish(frame(6));
        assert!(receiver.try_recv().is_err());
        for _ in 0..12 {
            sink.publish(frame(7));
        }
        assert_eq!(sink.received.load(Ordering::Relaxed), 12);
        assert_eq!(sink.dropped.load(Ordering::Relaxed), 10);
        assert_eq!(receiver.try_iter().count(), 2);
        sink.stop("view_closed");
        sink.publish(frame(8));
        assert!(receiver.try_recv().is_err());
        assert_eq!(*sink.reason.lock(), "view_closed");
    }
}
