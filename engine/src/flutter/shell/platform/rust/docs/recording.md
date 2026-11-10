# Per-view recording

Protocol v1 provides recording.start (viewId, name, optional rate), recording.stop
(name) and recording.status (name). Names contain ASCII letters, digits, hyphens
or underscores. Rates are 1–60 fps (default 15). Recording requires a local FFmpeg
with the lossless FFV1 encoder. Set FLUTTER_RUST_FFMPEG to select an executable.
Missing executables/codecs return unsupported_capability; screenshots/assertions
continue to work. Use timeoutMs=12000 when stopping a potentially slow encoder.

Recording uses the same completed Vulkan readbacks as screenshots. Each view has
one recording, one latest frame and two bounded two-frame queues: capture delivery
and encoder delivery. Neither pipe writes nor encoding run on the UI/raster
thread. GPU mapping still runs synchronously on raster, as established by the
Phase 0 contract; counters measure that cost rather than promising no overhead.
Idle periods duplicate the last completed frame. If encoder delivery drops a
source frame, the writer duplicates its previous frame to preserve the elapsed
output timeline. Catch-up/finalization has a five-second deadline; an encoder
that cannot finish produces explicit partial evidence.

Every recording-* directory contains lossless FFV1/BGRA Matroska segments,
FFmpeg stderr, frames.jsonl and an atomically replaced manifest.json. Resize or
size-generation changes end a segment. Segments retain dimensions, generation,
common session start/end timestamps, submitted/output frame counts, queue drops
and writer time. The frame timeline retains source UI/frame IDs, GPU-completion
timestamps, readback time, output index and queue acceptance. Color is linear,
straight-alpha RGBA8, matching the PNG normalization. View closure finalizes its
stream; simultaneous views retain individual frame timestamps on the common
session clock and are not claimed to be atomic captures.

Limits: eight recordings per session, 600 seconds and 32 segments per stream,
128 MiB per stream (checked each output second and enforced per encoder file).
PNG retention is 256 captures / 128 MiB; protocol traces and each application log
are capped at 64 MiB. Exceeding a limit fails explicitly. Slow encoding drops
source detail and reports counters; it does not grow an unbounded frame queue.

Encoder absence/crash, blocked pipes, view removal and app failure have separate
outcomes. recording.status remains available after encoding fails. A failed
recording makes session health fail. Session cleanup stops all streams before
joining workers. If the runner itself crashes, the owning CLI marks unfinalized
manifests as partial, retains files and exits nonzero. A hard kill cannot produce
a normally finalized recording; partial files may lack an index or final frames.

`tests/test_recording.py` decodes output with ffprobe/FFmpeg and checks odd widths,
fractional DPR, idle duration, pointer-action pixels, resize segments, two-view
session timestamps, view closure, missing/crashing/blocked encoders and bounded
finalization. `tests/test_agent_crash.py` kills the CLI-owned runner and checks
partial recovery. Queue-bound and owner-recovery tests run without a GPU.

Local NVIDIA RTX 5060 evidence recorded 49 readbacks with 29,119 microseconds total,
1,248 microseconds maximum and zero capture-queue drops in a 2.66-second two-view
workflow (one sample, not a performance budget). Manifests retain encoder timing
and both queue-drop counters for each run; application/GPU/driver/viewport must be
compared when interpreting overhead.
