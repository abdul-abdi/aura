# I Gave My Mac a Voice, Eyes, and Hands — in 20,000 Lines of Rust

**No SDK. No Electron. No examples. Just a WebSocket and a lot of staring at Wireshark.**

---

It's 3am on a Tuesday and I'm watching my Mac click the wrong button. Again.

I built an AI that sees my screen, hears my voice, and controls my computer. Except right now, it's clicking a search bar instead of the submit button, fifteen pixels to the left. I adjust the coordinate logic, rebuild, test. Now it's clicking the right button — but on the wrong monitor. I fix that. Now it clicks correctly but the page hasn't loaded yet so it clicks nothing. I add a settle delay. Now it works, but only in Safari. Chrome doesn't register the click the same way.

I close the laptop. Not gently — I just fold it shut.

*Maybe I should just quit.*

Let me tell you how I got here.

---

## The idea

I'd been using Gemini Live on my Android phone a lot. Just talking to it, getting instant natural replies — super conversational. One day I was at my MacBook, clicking through tabs, dragging windows, copy-pasting between apps — and a thought hit me:

**What if my Mac could do this?**

Not just chat — actually *see* my screen and *control* it. What if I gave an AI a mouse, a keyboard, and a live view of the screen? "Open Safari and search for flights to Tokyo." "What's in this spreadsheet?" "Fill out this form with my info." You just talk. It looks at the screen, figures out what's going on, and does it.

I figured two weekends, tops.

---

## No SDK. No examples. No mercy.

I chose Rust because I needed speed — real-time audio capture, screen streaming, mouse injection, keyboard simulation, all happening concurrently. I wanted a lean native binary, not a 200MB Electron wrapper.

There's no official Google SDK for Rust. The community-maintained crate didn't support the Live API. So I built the entire Gemini Live API WebSocket client from scratch, from sparse documentation and a lot of staring at protocol buffers.

The result: a Rust workspace with 10 crates — a handwritten WebSocket client for Gemini, audio capture and playback, screen capture with perceptual hashing, mouse and keyboard injection via CoreGraphics, an AppleScript bridge with safety sandboxing, SQLite and Firestore-backed memory, a native Cocoa menu bar, and an orchestrator daemon that wires it all together. 20,731 lines of Rust across 66 source files. Every WebSocket message, every CGEvent, every accessibility API call — written by hand.

---

## The bug that taught me to read binary

The first real wall wasn't architectural. It was a single line of code.

The WebSocket connection was working. Audio streaming. Video frames going out. Gemini was... silent. No responses. No errors. Just nothing. The connection was alive, messages were being sent, and the receive loop was running but never hitting my message handler.

I stared at logs for hours. Then I opened Wireshark.

Gemini was responding. Every single time. But it was sending **Binary** WebSocket frames, not Text. My receive loop was pattern-matching on `Message::Text` and silently ignoring everything else.

The fix:

```rust
let json_text = match msg {
    Message::Text(text) => Some(text),
    Message::Binary(bytes) => String::from_utf8(bytes).ok(),
    _ => None,
};
```

One line. One `Message::Binary` arm. That's the difference between "nothing works" and "everything works." The documentation doesn't mention this. I found out by watching raw packets on the wire.

If you're building on Gemini's Live API: **the server sends Binary frames.** You're welcome.

---

## Teaching a machine to see (cheaply)

Aura streams screenshots to Gemini at 2 frames per second — fast enough to follow what's happening, slow enough to not burn your API quota. But most of the time, your screen doesn't change. Sending identical frames wastes tokens and bandwidth.

So before each frame, Aura samples 8,192 pixels across the image and hashes them with FNV-1a:

```rust
fn compute_frame_hash(rgb: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let step = (rgb.len() / 8192).max(1);
    for i in (0..rgb.len()).step_by(step) {
        hash ^= rgb[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
```

If the hash matches the previous frame, it's not sent. Static screens cost zero tokens. And there's a priority system for when things *do* change: audio frames always flush before video, because a 300KB screenshot can stall the TCP socket for 50–300ms. Mic data reaches Gemini without gaps; the screenshot waits its turn.

The capture loop adapts its polling rate too — 500ms intervals when the screen is active, slowing to 1500ms when nothing has changed for 5 seconds. Less CPU, same responsiveness.

---

## The part that almost broke me

This is where I need to be honest.

The concept is simple: Gemini sees the screen, identifies a target ("the Submit button"), returns coordinates, Aura clicks there. In practice, this is a nightmare of compounding imprecision.

Gemini works with JPEG screenshots at 1920 pixels wide. The coordinates it returns are close — usually within 20 pixels — but "close" in UI interaction means "wrong." Fifteen pixels is the difference between clicking a button and clicking the text field next to it.

I built four independent strategies stacked in a fallback chain. An accessibility tree reader that clicks elements by name, bypassing coordinates entirely. PID-targeted clicks sent directly to an application's process instead of the global event stream. A vision oracle that takes a high-resolution screenshot and asks a separate Gemini model "Where *exactly* is this element?" And when all else fails, a spiral search — clicking at increasing radii around the target, hoping one of nine offsets lands on the right pixel.

The vision oracle has a lock-free circuit breaker — three consecutive failures trip it for 30 seconds so a broken oracle doesn't add 8 seconds of timeout to every click. Built entirely on atomics, no mutexes.

After all of this — the accessibility reader, the PID targeting, the oracle, the spiral search, the circuit breaker — it was still clicking the wrong thing half the time. That went on for about a week. Four hours of sleep a night, fixing one edge case only to find two more.

That was my lowest point. Not the technical difficulty — I could handle hard problems. It was the feeling that I'd built something beautiful and it couldn't do the one thing it was supposed to do.

---

## The night I almost walked away

It was a Thursday. I'd been at it since 6pm after work. Past 2am and the click accuracy had actually gotten *worse* after my latest changes. I had 43 open browser tabs, mostly Apple's Accessibility API docs in Objective-C that I was mentally translating to Rust FFI calls. I remember thinking: *I've spent more time on this than some people spend building entire products, and mine can't even click a button.*

I closed everything. Opened YouTube. Watched someone rank fast-food chicken sandwiches for twenty minutes.

Then — and I don't know exactly why — I opened the terminal again and ran it one more time. Not to fix anything. Just to hear it talk. I said "Hey, what do you think of my desktop right now?" and Aura described my cluttered screen with 43 tabs and said something about me clearly having a lot on my mind.

I laughed. Out loud, alone, at 2am.

And I thought: *this thing has a personality. It sees my screen. It talks to me like a person. I'm going to figure out the clicking part.* The hard problem wasn't "should I keep going?" — it was "can I ship something imperfect?"

I could. I did.

---

## Building voice AI on a laptop means your AI's first conversation partner is itself

Here's something nobody tells you about real-time voice AI on a MacBook: the microphone picks up the speakers.

Gemini's Live API streams audio continuously — you send your mic, it handles speech detection server-side and responds. But when Aura speaks through the speakers, that audio goes right back through the mic. Gemini hears its own response, thinks the user is speaking, and interrupts itself mid-sentence.

Google's official recommendation? "Use headphones."

That's not a product. That's a workaround.

So I built client-side echo gating. On startup, Aura listens to 500ms of ambient room noise and computes the noise floor:

```rust
pub(crate) fn calibrate_barge_in_threshold(rms_samples: &[f32]) -> f32 {
    let n = rms_samples.len() as f32;
    let mean = rms_samples.iter().sum::<f32>() / n;
    let variance = rms_samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n;
    let stddev = variance.sqrt();
    let threshold = mean + 3.0 * stddev;
    threshold.clamp(CALIBRATION_THRESHOLD_MIN, CALIBRATION_THRESHOLD_MAX)
}
```

During playback, audio below `threshold × 1.5` gets suppressed — likely speaker bleed. Three consecutive frames above threshold required to trigger interruption, filtering transient noises from actual speech. Then a 300ms reverb guard after playback stops.

The 1.5 multiplier was earned through trial and error. 1.3 interrupted in quiet rooms. 1.8 swallowed real speech. It's a constant battle between catching real speech and ignoring ghosts.

---

## The best optimization was deleting code

I had built a complete client-side Voice Activity Detection pipeline — `webrtc-vad` in `VeryAggressive` mode, 30ms frames, 150ms pre-roll, 300ms hangover. It worked. It also added 330ms of latency to every utterance.

Then I realized: Gemini already does VAD server-side. I was filtering audio before sending it to a system that filters audio.

I ripped the entire thing out. Mic audio now streams directly to Gemini with zero client-side processing. 330 milliseconds — gone. Not by adding a clever optimization, but by deleting one.

Every buffer, every filter, every gate I added to the audio path made things slower. The best version was the one with the least code in it.

---

## The things nobody sees

There's invisible engineering in Aura that has zero user impact — until it's missing.

**Session rotation.** This one surprised me. Long-lived Gemini audio sessions develop latency creep — responses get measurably slower over time. After 10 minutes, the degradation is noticeable. After 20, it's painful. Aura preemptively rotates the underlying WebSocket connection every 10 minutes, carrying the conversation forward through a resumption handle. The user hears one continuous conversation. Under the hood, it's a series of fresh sessions stitched together.

But resumption handles go stale. If the server rejects one, Aura needs to know when to stop retrying and start fresh. A poison counter tracks this: two consecutive resume attempts that die within 30 seconds triggers a full reset. Without it, a corrupted handle traps the app in an infinite reconnect loop.

**A menu bar that doesn't deadlock.** The Cocoa menu bar uses a `ReentrantMutex` instead of a standard `Mutex`. Why? Because when you call `popUpMenuPositioningItem`, Cocoa runs a *nested event loop* — which re-fires the NSTimer that polls for messages, which tries to acquire the same lock. A normal mutex deadlocks silently. No error, no crash, just a frozen app. This took me an embarrassing amount of time to figure out because there are zero Rust examples for Cocoa FFI edge cases like this.

---

## The first time it talked back

I'd been debugging for weeks. The WebSocket client was finally stable, the audio pipeline was streaming in both directions, and I'd just wired up the native audio model. I pressed the menu bar icon, said "Hey, how's it going?" — and Aura replied.

Not with text. With a voice. There was a half-second pause — just long enough for me to hold my breath — and then it spoke. Warm, conversational, slightly amused, like it had been waiting for me to ask.

I couldn't stop grinning. For a moment the room felt different — like there was someone else in it. Not typing at my Mac, not clicking prompts, not waiting for a loading spinner. Just... talking. And it heard me, and it answered.

---

## What it actually looks like now

Last week I was sitting at my desk, too lazy to fill out a form. I clicked the menu bar icon and said "Hey, can you fill this out? My name's Abdullahi, you know the rest."

Aura looked at the screen. Read the form fields from the accessibility tree. Started typing — first name, last name, email. It tabbed between fields, scrolled down to find the submit button, and clicked it. Not fifteen pixels to the left. Not on the wrong monitor. The right button, first try.

It's not perfect. Some forms trip it up. Complex multi-step flows need guidance. But most of the time, it just works — and when it does, it feels less like using a tool and more like asking someone for help.

The whole thing is a native macOS app that lives in your menu bar. The audio pipeline runs end-to-end in real time — mic capture at 48kHz, resampled to 16kHz, streamed over WebSocket, responses back at 24kHz with a 40ms jitter buffer. The memory system spans local SQLite and cloud Firestore, with three AI agents running in a single Cloud Run service that extract, consolidate, and query memories across sessions. It remembers things about you. It gets better the more you use it.

---

## What I'd do differently

**Trust the server.** When you're building on top of a powerful API, your job is to get out of its way — not to re-implement what it already does.

**Rust is incredible for this kind of work, but it's lonely.** No SDK, no examples, no Stack Overflow answers for "how to inject CGEvents from a Tokio runtime." You read Apple's Objective-C headers and figure it out yourself. The performance payoff is real — the entire audio pipeline runs in real time with headroom to spare. But every API integration is a solo expedition.

**The invisible work is the product.** CI/CD, environment management, graceful degradation, error messages that don't leak internals. Not the fun part. But without it, everything above is a demo.

---

The next time I'm at my Mac, dragging windows around, hunting through tabs — I won't be thinking about building something to fix it.

I'll just talk.

---

*Aura is open source — 20,731 lines of Rust, 10 crates, zero Electron. If you're building on Gemini's Live API, I hope these war stories save you a few hours of debugging.*
