# Aura

Voice control for my Mac. Aura sits in the menu bar, listens all the time, sees the screen, and clicks, types and runs apps for me through the Gemini Live API.

## Install / update

```bash
bash scripts/install.sh
```

Builds, installs to `/Applications/Aura.app` and launches. Needs Xcode or the Command Line Tools.

First run: click the menu bar dot, paste a Gemini API key ([get one](https://aistudio.google.com/apikey)), allow Microphone, Screen Recording and Accessibility, then Start. Grant Screen Recording, then reopen Aura if macOS asks.

**⌘⇧A** mutes and unmutes. You can also type in the panel.

## Settings

`~/.config/aura/config.toml`:

```toml
api_key = "..."
# model = "gemini-3.8-live"   # change if Google retires it
# voice = "Kore"
```

Remembered facts are stored in `~/Library/Application Support/Aura/memory.json`.

## When something's wrong

- Logs: `log stream --predicate 'subsystem == "com.aura.desktop"'`
- Permissions confused: `bash scripts/uninstall.sh`, then install again.

## Code

| File | Job |
|---|---|
| `Live.swift` | Gemini Live WebSocket. Reconnects forever and resumes the conversation. |
| `Audio.swift` | Mic and speaker with Apple's echo cancellation, so it never hears itself. |
| `Screen.swift` | Screen capture and 0–1000 coordinate mapping. |
| `Tools.swift`, `AX.swift`, `Input.swift` | What Gemini can do: accessibility elements, clicks, keys, menus, AppleScript, browser JS, memory. |
| `Aura.swift`, `Panel.swift`, `AuraApp.swift` | Wiring, the menu bar panel and the hotkey. |
| `Config.swift`, `Prompt.swift` | Settings, memory, permissions, instructions. |

`swift test` runs the unit tests.
