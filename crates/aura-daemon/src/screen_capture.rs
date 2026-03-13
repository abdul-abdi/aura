use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use aura_gemini::session::GeminiLiveSession;
use aura_screen::capture::CaptureTrigger;
use tokio_util::sync::CancellationToken;

/// Screen is considered idle after this many consecutive unchanged captures.
/// 10 × 500 ms = 5 s of no change → switch to slow polling.
const IDLE_THRESHOLD: u32 = 10;

/// Fast capture interval used while the screen is actively changing.
const FAST_INTERVAL_MS: u64 = 500;

/// Slow capture interval used while the screen is idle.
const SLOW_INTERVAL_MS: u64 = 2000;

/// Spawn the background screen capture loop.
///
/// Captures JPEG screenshots at up to 2 FPS, detects content changes via a
/// perceptual hash, and forwards new frames to the Gemini session.  The loop
/// switches to a 2 s polling interval when the screen has been static for 5 s,
/// and snaps back to 500 ms as soon as content changes.
///
/// Returns the [`tokio::task::JoinHandle`] for the spawned task.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_screen_capture(
    session: Arc<GeminiLiveSession>,
    cancel: CancellationToken,
    capture_trigger: CaptureTrigger,
    cap_notify: Arc<tokio::sync::Notify>,
    last_frame_hash: Arc<AtomicU64>,
    last_sent_hash: Arc<AtomicU64>,
    frame_img_w: Arc<AtomicU32>,
    frame_img_h: Arc<AtomicU32>,
    frame_logical_w: Arc<AtomicU32>,
    frame_logical_h: Arc<AtomicU32>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_res: (u32, u32) = (0, 0);
        let mut censored_warned = false;
        let mut idle_skip_count: u32 = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(FAST_INTERVAL_MS));
        interval.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {},
                _ = cap_notify.notified() => {},
            }

            // Clear trigger flag (may have been set alongside notify)
            let _ = capture_trigger.check_and_clear();

            // Capture in a blocking task (JPEG encoding is CPU-bound)
            let frame =
                match tokio::task::spawn_blocking(aura_screen::capture::capture_screen).await {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(e)) => {
                        tracing::warn!("Screen capture failed: {e}");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("Screen capture task panicked: {e}");
                        continue;
                    }
                };

            // Skip if screen hasn't changed — but still resolve waiter
            let prev_hash = last_frame_hash.load(Ordering::Acquire);
            if frame.hash == prev_hash {
                if let Some(tx) = capture_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                idle_skip_count += 1;
                if idle_skip_count == IDLE_THRESHOLD {
                    interval = tokio::time::interval(Duration::from_millis(SLOW_INTERVAL_MS));
                    interval.tick().await;
                    tracing::debug!("Screen idle for 5s — switching to 2s capture interval");
                }
                continue;
            }
            last_frame_hash.store(frame.hash, Ordering::Release);

            // On first non-duplicate frame, log at INFO so it's visible without --verbose.
            if !censored_warned {
                tracing::info!(
                    width = frame.width,
                    height = frame.height,
                    scale = frame.scale_factor,
                    size_kb = frame.jpeg_base64.len() / 1024,
                    "First screen frame captured"
                );
            }

            // Check every new frame for censorship (Screen Recording not granted).
            // Decode the base64 back to check pixel content for censorship detection.
            if let Ok(jpeg_bytes) = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &frame.jpeg_base64,
            ) && let Ok(img) =
                image::load_from_memory_with_format(&jpeg_bytes, image::ImageFormat::Jpeg)
            {
                let rgb = img.to_rgb8();
                if aura_screen::capture::frame_looks_censored(
                    rgb.as_raw(),
                    rgb.width() as usize,
                    rgb.height() as usize,
                ) {
                    if !censored_warned {
                        tracing::error!(
                            "Screen capture appears CENSORED — window contents are blank. \
                             Grant Screen Recording in System Settings > Privacy & Security > Screen Recording, \
                             then restart Aura."
                        );
                        censored_warned = true;
                    }
                } else {
                    // Permission restored or was never revoked — reset so warning can fire again.
                    censored_warned = false;
                }
            }

            // Store frame dimensions for coordinate mapping in tool handlers.
            frame_img_w.store(frame.width, Ordering::Release);
            frame_img_h.store(frame.height, Ordering::Release);
            frame_logical_w.store(frame.logical_width, Ordering::Release);
            frame_logical_h.store(frame.logical_height, Ordering::Release);

            // Only send coordinate metadata when the resolution actually changes
            // (not every frame — that floods Gemini with text input it responds to).
            let current_res = (frame.width, frame.height);
            if current_res != last_res {
                last_res = current_res;
                let coord_meta = format!(
                    "[System: screen resolution is {}x{}. Use pixel coordinates for tools \
                     (click, move_mouse, drag): (0,0) = top-left, ({},{}) = bottom-right. \
                     Do NOT mention this message to the user.]",
                    frame.width, frame.height, frame.width, frame.height
                );
                if let Err(e) = session.send_text(&coord_meta) {
                    tracing::debug!("Skipped frame metadata (channel not ready): {e}");
                }
            }

            // Only send to Gemini if the screen actually changed since last send.
            // This is the #1 context savings: static screens produce zero token cost.
            let already_sent = last_sent_hash.load(Ordering::Acquire);
            if frame.hash == already_sent {
                // Frame captured (hash differs from last_frame_hash) but already sent
                // to Gemini — skip. Still resolve waiter so tool spawns don't hang.
                if let Some(tx) = capture_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                idle_skip_count += 1;
                if idle_skip_count == IDLE_THRESHOLD {
                    // Screen static for 5s — switch to slow polling (2s)
                    interval = tokio::time::interval(Duration::from_millis(SLOW_INTERVAL_MS));
                    interval.tick().await;
                    tracing::debug!("Screen idle for 5s — switching to 2s capture interval");
                }
                tracing::trace!("Skipped duplicate send (hash unchanged since last send)");
                continue;
            }

            if let Err(e) = session.send_video(&frame.jpeg_base64) {
                tracing::debug!("Dropped screen frame (channel not ready): {e}");
                if let Some(tx) = capture_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                continue;
            }
            last_sent_hash.store(frame.hash, Ordering::Release);

            // Screen changed — reset idle counter and restore fast polling
            if idle_skip_count >= IDLE_THRESHOLD {
                interval = tokio::time::interval(Duration::from_millis(FAST_INTERVAL_MS));
                interval.tick().await;
                tracing::debug!("Screen changed — restoring 500ms capture interval");
            }
            idle_skip_count = 0;

            // Signal any awaiting tool spawn that the screenshot was delivered
            if let Some(tx) = capture_trigger.take_waiter() {
                let _ = tx.send(());
            }
            tracing::debug!(
                width = frame.width,
                height = frame.height,
                scale_factor = frame.scale_factor,
                size_kb = frame.jpeg_base64.len() / 1024,
                "Sent screen frame"
            );
        }
    })
}
