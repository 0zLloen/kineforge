# KineForge research: mouse input semantics (Windows / Linux / macOS / Rust / HID / Browser)

**Scope.** What each platform API actually delivers for mouse motion, how close it is to the sensor, what timestamps exist,
and how KineForge should capture and label it. Research only: nothing installed, no permissions or settings changed.

**Dating and verification.**
- §1 (Windows), §4 (Rust) and §6 (Browser) were researched on **2026-11-13**. All other sections and the merged parts were researched on **2026-11-14**.
- Every claim carries a URL and, for source code, a file path with line numbers at a pinned commit.
- **UNVERIFIED** marks anything not confirmed from a primary source.
- "(local)" marks read-only observations on the user's machine.

**Pinned sources for §2, §3 and §5 (accessed 2026-11-14)**

| Project | Commit / version |
|---|---|
| Linux | `torvalds/linux` master `97d73c825122` (v7.4-rc2, 2026-11-13); prefix **K** = `https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/` |
| libinput | main `0685c9787e59` (1.32.0+, 2026-11-12); **LI** = `https://gitlab.freedesktop.org/libinput/libinput/-/blob/0685c9787e59e5540c515ab74b118047f84fdd66/` |
| X.Org server | main `f6839e1d9bef` (2026-11-10); **XS** = `https://gitlab.freedesktop.org/xorg/xserver/-/blob/main/` (line numbers at that commit) |
| xf86-input-libinput | master `ac862672e4d0` (2025-11-05) |
| xorgproto | master `6d3907b6d5dc` |
| wayland-protocols | main `3eab2dccf4bf` (2026-10-29) |
| wlroots | master `72956e71dfcd` |
| sway | `1652c54b73f6` |
| KWin | `c90230b7be5c` |
| Mutter | main `be02dfae4ad9` |
| libei | `a9bf31da06f0` |
| xdg-desktop-portal | `632a43d9d2f4` |
| Apple IOHIDFamily | `apple-oss-distributions/IOHIDFamily` tag `IOHIDFamily-2238.120.5` |
| macOS SDK headers | `phracker/MacOSX-SDKs` `MacOSX11.3.sdk` |
| hidapi | `libusb/hidapi` master `a4ad72a40e76` (2026-11-12) |

---

## 0. Decisive findings

1. **No mainstream API delivers a device-side timestamp for a mouse.**
   - USB HID mouse reports carry no time field.
   - Every timestamp is a host-side instant, and each source means something different:
     - Linux evdev: kernel time when the report is flushed at `SYN_REPORT`. µs, clock selectable, default REALTIME.
     - libinput: that same time on CLOCK_MONOTONIC, µs.
     - Wayland `relative_motion` (`utime`): µs, base unspecified. On wlroots/sway it is **ms×1000**.
     - X11: X server `GetTimeInMillis` when the event is posted, `CLOCK_MONOTONIC_COARSE`, ms.
     - macOS IOHID: AbsoluteTime per report.
     - Windows Raw Input: **nothing**; the app takes a QPC reading when it dequeues.
     - GameInput: µs per reading, base UNVERIFIED.
     - Browsers: coarsened page clock.
   - KineForge must store `(value, clock, resolution, meaning)` and never pretend these are comparable.

2. **True device counts (HID input report X/Y) can be read only on:**
   - **Linux via hidraw.** It is permission-gated. Locally a user udev rule opens the Glorious receiver's hidraw nodes, so this works now. The kernel delivers the same report to evdev at the same time.
   - **macOS via IOHID.** Needs Input Monitoring (TCC) and a *non-seize* open. hidapi seizes by default.
   - Not on **Windows**: the OS opens mouse collections for exclusive use.
   - Not in **browsers**: WebHID always protects Mouse/Pointer input reports.
   - Even HID counts already include firmware processing (CPI scaling, angle snapping, smoothing, lift-off cut, motion sync).

3. **Tier-2 "OS raw" sources are close, but each has a named contamination path:**
   - **Linux evdev** = HID logical values unchanged, one packet per report. Zero-valued axes are dropped, and all-zero reports vanish. HID-BPF can rewrite reports before evdev *and* hidraw.
   - **libinput `dx_unaccelerated`** = device units plus configured rotation. It is *not* DPI-normalised; only the accelerated deltas are normalised. Lua plugins (≥ 1.30, opt-in per compositor) can rewrite the evdev frames.
   - **Xorg `XI_RawMotion`**: `axisvalues_raw` comes from libinput unaccelerated. Raw events are **global and need no permissions** under XI ≥ 2.1. Under **Xwayland** they are Wayland `dx_unaccel` × surface scale, delivered only while focused.
   - **Wayland `dx_unaccel`**: the protocol itself says it "does not represent 'raw' events". In practice it is libinput unaccel on Mutter, KWin and wlroots.
   - **Windows Raw Input** bypasses Enhanced Pointer Precision. Kernel filter drivers (RawAccel) and RDP/VM absolute packets can still contaminate it.
   - **macOS** `kCGEventUnacceleratedPointerMovementX` (public field 170) and GCMouse ("raw mouse movement delta values"): units versus HID counts are UNVERIFIED.

4. **Browsers never yield device counts.**
   - `unadjustedMovement` works only on Windows, macOS and ChromeOS (Chrome); Windows and macOS (Firefox ≥ 152); macOS (Safari ≥ 18.4).
   - **Linux is always OS-accelerated**, and per source code both installed browsers reject `unadjustedMovement` with `NotSupportedError` (runtime check E1 pending).
   - Units differ by engine: Chrome DIP, Firefox CSS px or platform units, Safari double.
   - Timestamps are coarsened: Chrome 100 µs (5 µs with COI); Firefox 1 ms + jitter (20 µs with COI); Safari 1 ms.

5. **Off-the-shelf Rust layers are unsuitable as the measurement stream.**
   - **winit:**
     - No input timestamps at all.
     - On Windows, absolute RDP/VM packets are emitted as deltas (#2884).
     - macOS uses accelerated `NSEvent.deltaX`.
     - Wayland drops `utime` and the device id.
   - **SDL3:**
     - Windows timestamps are interpolated.
     - X11 relative timestamps leak X-server ms into the ns field (source-verified, not run).
     - macOS 3.4 deltas are OS-processed.
   - **Conclusion:** use winit/SDL for windows and cursor grab only, plus dedicated capture backends (§8).

6. **Windows 11 throttles and coalesces raw input for background listeners.** Microsoft's 2023 developer blog confirms this. The "125 Hz" figure is UNVERIFIED (forum quote only).
   - High polling rates (4–8 kHz) need batched `GetRawInputBuffer` draining on a dedicated thread. Godot measured an 8 kHz mouse dropping a game to < 1 FPS without it.

7. **Local machine (read-only, 2026-11-14):**
   - **Software:** Linux Mint X11/Cinnamon, kernel 7.0.0-42 (`CONFIG_HZ=1000`), libinput 1.25.0, Xorg 21.1.12, xf86-input-libinput 1.4.0.
   - **Mouse configuration:** libinput profile **adaptive**, speed 0. No `MOUSE_DPI` udev property.
   - **Receiver:** the Glorious receiver `258a:2031` is **USB full-speed, interrupt EP 0x81, bInterval 1 ms**, so at most 1000 reports/s through this receiver.
   - **Report format:** X/Y are **16-bit signed relative** (−32767..32767). There is no Resolution Multiplier, so hi-res wheel values are multiples of 120.
   - **Access:**
     - `/dev/input/event7` is `root:input 0660` with no ACL, so it is **not readable**.
     - `XI_RawMotion` **is** readable.
     - `/dev/hidraw2` (the mouse TLC) **is** readable (0666 + uaccess via `/etc/udev/rules.d/99-glorious-series-one-pro.rules`).
   - **Consequence:** a dual-source validation rig (HID ground truth vs XI2 raw vs browser) is possible today **without any permission change**.

---
## 1. Windows (researched 2026-11-13; covers §1 plus Windows parts of §5 and §7)

All URLs accessed 2026-11-13. "MS-SDK" = github.com/MicrosoftDocs/sdk-api (source of learn.microsoft.com/windows/win32/api pages), "MS-Win32" = github.com/MicrosoftDocs/win32 (inputdev conceptual pages, ms.date 07/14/2025), "MS-Drv" = github.com/MicrosoftDocs/windows-driver-docs (branch `staging`). Source pins: SDL `fff6448f523f` (2026-11-12), winit `f0abec90a3f9` (2026-11-13), RawAccel `master` (pushed 2026-04-23). **UNVERIFIED** = no primary source found.

### 1.0 Data path (where counts can be altered)

```
sensor+MCU firmware (CPI scaling, angle snap, smoothing, LOD, motion sync)  ← already in "HID counts"
  → USB/BT HID input report (X/Y logical, buttons, wheel) @ report rate
  → HIDCLASS.sys → MOUHID.sys (maps HID usages → X/Y/buttons/wheel = MOUSE_INPUT_DATA)
  → [optional vendor upper filters on mouhid or mouclass — can rewrite LastX/LastY]
  → MOUCLASS.sys queue (MOUSE_INPUT_DATA packets)
  → win32k Raw Input Thread ─┬→ WM_INPUT / GetRawInputBuffer   (no ballistics)
                             ├→ pointer ballistics (EPP / speed slider) → cursor → WM_MOUSEMOVE, WM_POINTER, GetMouseMovePointsEx
                             └→ (GameInput: separate runtime "built directly on top of the Windows hardware interfaces")
```
- MOUHID "Converts HID usages into mouse commands (X/Y, buttons, wheel)"; MOUCLASS "supports both absolute and relative pointing devices"; vendors may add "upper filter to kbdhid/mouhid" or "upper filter to kbdclass/mouclass"; "The system opens all keyboard and mouse collections for its exclusive use." — MS-Drv `hid/keyboard-and-mouse-hid-client-drivers.md` (ms.date 06/26/2024) https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/keyboard-and-mouse-hid-client-drivers
- `MOUSE_INPUT_DATA` "contains one packet of mouse input data"; Mouclass transfers these to the Win32 subsystem on IRP_MJ_READ. https://learn.microsoft.com/en-us/windows/win32/api/ntddmou/ns-ntddmou-mouse_input_data
- Whether MOUHID applies any scaling to relative X/Y (e.g. HID Physical/Unit or Resolution Multiplier on X/Y) is not documented → **UNVERIFIED**; believed pass-through of logical values, one MOUSE_INPUT_DATA per input report. Verify with a USB capture vs `lLastX` (experiment W1).
- **Proof that filters contaminate "raw" input:** RawAccel ("kernel mode mouse accel", "allows for the acceleration of mouse input in the raw input stream") hooks the mouclass service callback and rewrites `it->LastX = out_x` with a carried sub-count remainder; it skips packets flagged `MOUSE_MOVE_ABSOLUTE`; optional `set_extra_info` packs the *original* (short)LastX/LastY into `ExtraInformation`. https://github.com/RawAccelOfficial/rawaccel/blob/master/driver/driver.cpp (L47–138), ReadMe https://github.com/RawAccelOfficial/rawaccel/blob/master/ReadMe.md. Same class of risk: Interception-style filters, vendor suites (G HUB/Synapse) — **UNVERIFIED** whether any given suite rewrites motion.

### 1.1 Raw Input registration

| Item | Fact | Source |
|---|---|---|
| Mouse TLC | Usage Page 0x01, Usage 0x02 | RID_DEVICE_INFO_MOUSE remarks, MS-SDK `ns-winuser-rid_device_info_mouse.md` |
| Default delivery | "sends raw input … to the registered application as long as it has the window focus" | RAWINPUTDEVICE, https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawinputdevice |
| `hwndTarget=NULL` | "raw input events follow the keyboard focus" | same |
| `RIDEV_INPUTSINK` 0x100 | receive input "even when the caller is not in the foreground"; `hwndTarget` must be set | same |
| `RIDEV_EXINPUTSINK` 0x1000 | background delivery "only if the foreground application does not process it" (Vista+) | same |
| `RIDEV_NOLEGACY` 0x30 | suppresses legacy WM_MOUSEMOVE/WM_LBUTTONDOWN… for that TLC (mouse/keyboard only) | same |
| `RIDEV_CAPTUREMOUSE` 0x200 | click doesn't activate other window; requires NOLEGACY | same |
| `RIDEV_DEVNOTIFY` 0x2000 | WM_INPUT_DEVICE_CHANGE arrival/removal | same |
| `RIDEV_REMOVE` 0x1 | must have `hwndTarget=NULL` | same |
| **One registrant per class per process** | "Only one window per raw input device class may be registered … within a process (the window passed in the last call…). … should not be used from a library" | https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerrawinputdevices |
| wParam | `GET_RAWINPUT_CODE_WPARAM`: `RIM_INPUT`=0 (foreground; must call DefWindowProc for cleanup), `RIM_INPUTSINK`=1 (background) | https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-input , …/nf-winuser-get_rawinput_code_wparam |

**KineForge consequence:** winit's win32 backend also registers mice (`DeviceEvents::WhenFocused → RIDEV_DEVNOTIFY`, `Always → RIDEV_DEVNOTIFY|RIDEV_INPUTSINK`, `Never → RIDEV_REMOVE`) — winit `winit-win32/src/raw_input.rs` L133–146. A KineForge raw-input thread in the same process will steal/lose the registration depending on call order (last call wins; a later winit `RIDEV_REMOVE` would remove KineForge's). Own raw input exclusively: set winit device events to `Never` *before* registering, then register from the capture thread.

### 1.2 RAWMOUSE semantics (https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse)

| Field | Semantics |
|---|---|
| `usFlags` | `MOUSE_MOVE_RELATIVE` 0x00; `MOUSE_MOVE_ABSOLUTE` 0x01; `MOUSE_VIRTUAL_DESKTOP` 0x02; `MOUSE_ATTRIBUTES_CHANGED` 0x04; `MOUSE_MOVE_NOCOALESCE` 0x08 = "This mouse movement event was not coalesced. Mouse movement events can be coalesced by default." Note RELATIVE==0 ⇒ test `!(usFlags & ABSOLUTE)`, never `flags & RELATIVE`. |
| `lLastX/lLastY` (LONG) | relative: "movement relative to the previous mouse event"; +right/+down. absolute: "normalized absolute coordinates between 0 and 65,535" → primary monitor, or whole virtual desktop if `MOUSE_VIRTUAL_DESKTOP`. Doc gives `MulDiv(lLastX, SM_CX…, USHRT_MAX)` sample. |
| Ballistics | "In contrast to legacy WM_MOUSEMOVE … Raw Input mouse events is not subject to the effects of the mouse speed set in the Control Panel" |
| `usButtonFlags` | RI_MOUSE_BUTTON_1..5 DOWN/UP transitions (0x0001–0x0200), `RI_MOUSE_WHEEL` 0x0400, `RI_MOUSE_HWHEEL` 0x0800 (not XP) — transitions, not state |
| `usButtonData` | signed short wheel delta; WHEEL_DELTA=120 per notch; finer wheels "send more messages per rotation, but with a smaller value in each" (doc also says "will be a multiple of WHEEL_DELTA" — self-contradictory; accept non-multiples) |
| `ulRawButtons` | "raw state of the mouse buttons. The Win32 subsystem does not use this member." (device-dependent; often 0 — **UNVERIFIED** per device) |
| `ulExtraInformation` | device-specific; pen/touch-synthesized mouse has `(x & 0xFFFFFF00)==0xFF515700` (MI_WP_SIGNATURE) https://learn.microsoft.com/en-us/windows/win32/tablet/system-events-and-mouse-messages ; RawAccel may store original counts here (above) |
| `header.hDevice` | per-device handle → multi-mouse; "can be zero if an input is received from a precision touchpad". Injected (SendInput) events commonly arrive with hDevice=0 — **UNVERIFIED** in docs. |

Device info: `GetRawInputDeviceInfo(hDevice, RIDI_DEVICENAME | RIDI_DEVICEINFO | RIDI_PREPARSEDDATA)`; set `RID_DEVICE_INFO.cbSize` first. `RID_DEVICE_INFO_MOUSE`: `dwId` (0x80 HID mouse, 0x100 HID wheel mouse, 0x8000 horiz wheel), `dwNumberOfButtons`, `dwSampleRate` ("data points per second. This information may not be applicable for every mouse device"), `fHasHorizontalWheel`. https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rid_device_info_mouse . `dwSampleRate` for USB HID mice is a PS/2-era field; do **not** trust it as report rate — measure inter-arrival (exp. W4). RIDI_DEVICENAME gives the `\\?\HID#VID_…&PID_…` interface path (VID/PID/collection for provenance).

#### Absolute mode (RDP, VMs, tablets)
- Remote Desktop and tablets deliver `MOUSE_MOVE_ABSOLUTE`; SDL documents RDP behaviour (SetCursorPos calls cached/coalesced, user motion coalesced with warps, so it synthesizes relative motion by differencing + recentre "wobble") — SDL `src/video/windows/SDL_windowsevents.c` L609–660; SDL also honours undocumented `usFlags & 0x40` as "raw coordinates" (L621) → **UNVERIFIED** flag.
- VM integration (Hyper-V/VMware/VirtualBox mouse integration) → absolute: **UNVERIFIED** (no primary doc found); detect at runtime instead.
- **winit bug (verified in source):** `if util::has_flag(mouse.usFlags, MOUSE_MOVE_RELATIVE)` with `has_flag(b,f) = b & f == f` and `MOUSE_MOVE_RELATIVE == 0` is always true, so absolute 0–65535 coordinates are emitted as `PointerMotion { delta }` — `winit-win32/src/event_loop.rs` L2667–2672, `util.rs` L46–51; open issue https://github.com/rust-windowing/winit/issues/2884 (2023-06-16, "Mouse Motion incorrect when using Remote Desktop").
- KineForge rule: any `MOUSE_MOVE_ABSOLUTE` packet ⇒ provenance `os_absolute_pointer` (never device counts); also check `GetSystemMetrics(SM_REMOTESESSION)`.

### 1.3 Reading: standard vs buffered; high polling rates

- Standard: GetMessage → `GetRawInputData(lParam, RID_INPUT, …)` one RAWINPUT at a time. Buffered: `GetRawInputBuffer` returns an array, walk with `NEXTRAWINPUTBLOCK`; "useful for high-frequency devices such as mice at 1000Hz". **Important:** "GetMessage removes the current WM_INPUT from the raw input queue before returning. As a result, GetRawInputBuffer will not see the current event" → pattern: read lParam event with GetRawInputData, then drain with GetRawInputBuffer; or never retrieve WM_INPUT (range-filtered PeekMessage `0..WM_INPUT-1`, `WM_INPUT+1..0xFFFF`) and drain on a timer; "Don't use RIDEV_DEVNOTIFY with this pattern" (WM_INPUT_DEVICE_CHANGE goes via raw input queue → thread spins). MS-Win32 `inputdev/about-raw-input.md`, `inputdev/using-raw-input.md` (ms.date 07/14/2025) https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input , https://learn.microsoft.com/en-us/windows/win32/inputdev/using-raw-input
- `GetRawInputBuffer`: returns count, `(UINT)-1` on error (grow buffer on ERROR_INSUFFICIENT_BUFFER); pointer DWORD-aligned; **WOW64 note:** "do not use *pcbSize, use *pcbSize * 8 instead… you must align the RAWINPUT structure by 8 bytes" (header is 24 bytes for 64-bit layout). https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getrawinputbuffer . `NEXTRAWINPUTBLOCK` aligns to `sizeof(QWORD)` on _WIN64, DWORD otherwise (mingw-w64 `winuser.h` L6399–6404, https://github.com/mingw-w64/mingw-w64/blob/master/mingw-w64-headers/include/winuser.h). SDL handles the WOW64 case by `rawinput_offset = sizeof(RAWINPUTHEADER) + 8` (SDL_windowsevents.c L823–830). Rust: build x86_64 only; avoid WOW64.
- Legacy mouse messages: "When mouse messages are posted faster than a thread can process them, the system discards all but the most recent mouse message" (WM_MOUSEMOVE coalescing) — https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input
- **CPU cost at 4–8 kHz (engineering source, not MS):** Godot 4.7.2/4.8-dev3 (2026-08-24, H. Locurcio) — on Win11 24H2 the per-app input throughput limit is "somewhere between 1 kHz and 2 kHz" on most CPUs; apps that register raw input still get both WM_MOUSEMOVE and WM_INPUT unless NOLEGACY; fix = keep WM_INPUT queued and drain buffered once per frame, dispatch WM_MOUSEMOVE at most once/frame. Measured: captured mouse @8 kHz "under 1 FPS → 1703 FPS"; visible @8 kHz 173 → 1103 FPS (9950X3D). https://godotengine.org/article/fixing-high-polling-rate-mice-on-windows/
- SDL3 pattern (verified source): dedicated thread `SDLRawInput`, message-only window (`HWND_MESSAGE`), `SetThreadPriority(THREAD_PRIORITY_TIME_CRITICAL)`, `MsgWaitForMultipleObjects(…, QS_RAWINPUT)`, drain with `GetRawInputBuffer` — `src/video/windows/SDL_windowsrawinput.c` L60–195.

### 1.4 Timestamps (none from the device)

| Source | Domain / unit | Resolution / meaning |
|---|---|---|
| RAWINPUT / RAWINPUTHEADER | **no timestamp field** (dwType,dwSize,hDevice,wParam) | — https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawinputheader |
| `GetMessageTime()` | ms since system start, LONG, wraps; "time the message was created (that is, placed in the thread's message queue)" | Only for messages retrieved by GetMessage (useless with GetRawInputBuffer drains). Effective granularity: tick count; `GetTickCount` resolution "typically in the range of 10 milliseconds to 16 milliseconds" — that GetMessageTime shares this granularity is **UNVERIFIED**. https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmessagetime , https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-gettickcount |
| QPC at receipt (app-side) | QueryPerformanceCounter, host monotonic | <1 µs typical; "independent of … any external time reference" https://learn.microsoft.com/en-us/windows/win32/sysinfo/acquiring-high-resolution-time-stamps . Measures *dequeue* time, includes scheduling/queue latency; one value per drain batch unless reading per message. |
| `POINTER_INFO.PerformanceCount` (WM_POINTER; mouse needs `EnableMouseInPointer(TRUE)`, once per process) | QPC "when the pointer message was received"; calibrated to digitizer scan time only for touch hardware | cursor-space (ballistics-applied) positions; `historyCount` = inputs coalesced into the message, retrievable via GetPointerInfoHistory. https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-pointer_info , …/nf-winuser-enablemouseinpointer |
| `PointerPoint.Timestamp` (WinRT) | µs "relative to the system boot time" | https://learn.microsoft.com/en-us/uwp/api/windows.ui.input.pointerpoint.timestamp |
| `GetMouseMovePointsEx` | 64-entry history of cursor positions (display or 0–65535 points) + ms time | cursor-space; https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmousemovepointsex |
| GameInput `IGameInputReading::GetTimestamp` | uint64 µs, "when the input was made"; compare with `IGameInput::GetCurrentTimestamp` | Only per-reading timestamp available for mouse on Windows. Clock base (QPC-derived?) and whether it is driver-arrival vs service-arrival: **UNVERIFIED**. https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/interfaces/igameinputreading/methods/igameinputreading_gettimestamp |

- **SDL3 synthesizes raw-input timestamps** by linear interpolation across each drained batch: `time = poll_finish - delta*(mouse_total-mouse_index)/mouse_total` (SDL_windowsevents.c L875) — i.e. *derived*, evenly spaced, not measured. SDL's GameInput path converts µs with a single offset captured at init (`timestamp_offset = SDL_NS_TO_US(now) - GetCurrentTimestamp()`, SDL_windowsgameinput.cpp L282–285, used L300/L319) — no drift correction.
- KineForge: record `t_receipt_qpc` per event (per message if read individually, else per batch with `batch_id`+index), `clock=QPC`, `resolution=1/QPF`, `meaning=host_dequeue`; never interpolate into the measured field (store interpolations separately, flagged derived).

### 1.5 Enhanced Pointer Precision (EPP) and what bypasses it

- XP+ ballistics (archived MS white paper "Pointer Ballistics for Windows XP", updated 2002-10-31): mickeys converted to physical units assuming **USB 125 Hz and 400 mickey/inch** defaults and screen refresh rate/resolution; gain looked up from a 5-point (6 incl. origin) transfer curve on the **vector magnitude** of per-packet X/Y, interpolated; 16.16 fixed point; "divided remainder mouse counts have to be preserved and added into the next counts" (sub-pixel carry); curve stored in registry; algorithm "resides between ring0 and ring3". https://web.archive.org/web/20110623221617/http://msdn.microsoft.com/en-us/windows/hardware/gg463319.aspx  ⇒ because velocity = counts/packet assuming 125 Hz, EPP output depends on report rate (UNVERIFIED whether modern Windows corrects for actual rate).
- Legacy (pre-XP) threshold doubling ×2/×4 still documented on MOUSEINPUT remarks for SendInput relative motion; settings via `SystemParametersInfo(SPI_GETMOUSE)` → 3 ints (threshold1, threshold2, acceleration) and `SPI_GETMOUSESPEED` 1..20 (default 10). https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput , …/nf-winuser-systemparametersinfoa . Mapping "EPP checkbox on ⇔ SPI_GETMOUSE[2]≠0" is folk knowledge → **UNVERIFIED** primary.
- Bypass matrix: **Raw Input (WM_INPUT): bypasses** (RAWMOUSE remarks; "WM_INPUT has no ballistics applied", "read directly from the HID stack") — https://learn.microsoft.com/en-us/windows/win32/dxtecharts/taking-advantage-of-high-dpi-mouse-movement . **WM_MOUSEMOVE / WM_POINTER / GetMouseMovePointsEx / GetCursorPos: ballistics applied + pixel-quantised** ("no WM_MOUSEMOVE message is generated" for sub-pixel motion, same article). **DirectInput: internally "creates a second thread to read WM_INPUT data"** → unaccelerated but more overhead, "discouraged" for mouse (same article). GameInput: deltas "reported natively as a series of delta values" — no explicit "no ballistics" statement → **UNVERIFIED but expected raw** (exp. W3). WinRT `MouseDevice.MouseMoved`/`MouseDelta`: docs only say "change in screen location of the mouse pointer" → raw vs accelerated **UNVERIFIED** (exp. W3).
- None of these bypass kernel filter drivers (RawAccel etc.) or firmware.

### 1.6 Windows 11 background raw-input throttling

- **Primary (MS Windows Developer Blog, Sharif Farag, 2023-05-26, "Reduced game stutter with high report rate mice"):** "throttling and coalescing background raw mouse listeners and capping their message rate"; test on Surface Laptop Studio with a 1000 Hz mouse. No number given. https://blogs.windows.com/windowsdeveloper/2023/05/26/delivering-delightful-performance-for-more-than-one-billion-users-worldwide/
- The figure "**125** messages/s for a 1000 Hz mouse, each containing the combined payload" comes from a Microsoft engineer quoted on Blur Busters forums (https://forums.blurbusters.com/viewtopic.php?p=94890, 403 to fetchers) → **UNVERIFIED (secondary)**. Shipped publicly with Win11 22H2 KB5028185 (2023-07-11) per press (techpowerup.com/311357) → **UNVERIFIED** vs MS release notes. Not in Windows 10 (secondary).
- Reports (2025–26) that some games are misclassified as background on 24H2 and get ~125 Hz (EA forums BF6 thread) → anecdotal.
- What "background" means operationally (RIM_INPUTSINK recipients vs non-foreground process) is undocumented; coalesced packets presumably sum lLastX/Y ("combined payload") — whether sums are lossless and whether `MOUSE_MOVE_NOCOALESCE` is cleared on coalesced packets: **UNVERIFIED** (exp. W2).
- KineForge consequence: capture in the background (e.g. alongside a third-party game) loses temporal resolution (~8 ms bins) though counts may be preserved; tag every packet with `RIM_INPUT`/`RIM_INPUTSINK` and a `foreground` flag; treat background segments as `aggregation=os_coalesced`.

### 1.7 GameInput (mouse)

- Availability: "PC via NuGet … back to Windows 10 19H1"; Microsoft.GameInput latest **4.0.306 (published 2026-10-28)** on nuget.org (registration API). "built directly on top of the Windows hardware interfaces", lock-free, "synchronized to a common time base". https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-overview (ms.date 2026-10-29)
- `GameInputMouseState {buttons, positions, positionX, positionY, absolutePositionX, absolutePositionY, wheelX, wheelY}` (int64); positionX/Y = "Cumulative … change"; "Calculate deltas by subtracting"; trackpads appear as one virtual mouse. https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/structs/gameinputmousestate
- "Individual deltas are accessible when iterating through all intermediate readings"; "The accumulated delta value is only for the mouse events that a process receives while it has input focus"; per-mouse readings. https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/advanced/input-keyboard-mouse
- History: "The input stream maintains the last half second of historical readings" (GetNextReading walk) → must poll ≥2 Hz or use callbacks. https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-readings
- Focus: `GameInputFocusPolicy` removed in v4 → `IGameInput::EnableBackgroundInput`; default "will not provide background input". https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/deprecated/enums/gameinputfocuspolicy
- Raw HID bytes: `GetRawReport` — "only Game Input Protocol (GIP) devices support raw reports" (v0 intro, v1 removed, v3 re-introduced) ⇒ **no raw HID mouse reports via GameInput**. https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/interfaces/igameinputreading/methods/igameinputreading_getrawreport
- SDL3 uses GameInput for mouse only if `SDL_HINT_WINDOWS_GAMEINPUT="1"` (default "0") — SDL `include/SDL3/SDL_hints.h` L4727–4740.
- Value for KineForge: only Windows API giving **per-reading µs timestamps + per-device integer deltas**; whether one reading == one HID report (vs coalesced) is **UNVERIFIED** (exp. W3/W4). Rust: no maintained binding found in this pass (not searched in depth) — COM via `windows` crate would need hand-written vtables.

### 1.8 Cursor confinement (for completeness)
Raw input is independent of cursor position; lock = hide cursor + `ClipCursor` to client rect (and optionally recentre with SetCursorPos, which only affects the cursor stream). RDP caveats above.

### 1.9 Runtime contamination / provenance checks (Windows)
1. `usFlags & MOUSE_MOVE_ABSOLUTE` or `SM_REMOTESESSION` → not device counts.
2. `ulExtraInformation & 0xFFFFFF00 == 0xFF515700` → pen/touch-synthesized.
3. `hDevice == 0` → injected/touchpad (treat as non-device).
4. Read (read-only registry) `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e96f-e325-11ce-bfc1-08002be10318}\UpperFilters` (Mouse setup class GUID per https://learn.microsoft.com/en-us/windows-hardware/drivers/install/system-defined-device-setup-classes-available-to-vendors) and the device's own `UpperFilters`; anything besides `mouclass` (e.g. `rawaccel`) ⇒ flag `possible_filter_driver`. Exact default value list **UNVERIFIED**.
5. Known-motion injection test: a hardware motion rig/known pattern vs captured counts (§9).
6. `GetCurrentInputMessageSource` → `IMO_HARDWARE / IMO_INJECTED / IMO_SYSTEM` (https://learn.microsoft.com/en-us/windows/win32/api/winuser/ne-winuser-input_message_origin_id) — applicability to WM_INPUT / buffered drains **UNVERIFIED**; LL hook `MSLLHOOKSTRUCT.flags & LLMHF_INJECTED` is documented for the cursor path.

### 5-Win. HID-level access on Windows
- "Top-level collections opened by Windows for system use": Mouse 0x01/0x01–0x02 **Exclusive**, Keyboard 0x01/0x06–0x07 Exclusive, pens/touch/PTP Exclusive; consumer controls 0x0C/0x01 Shared. https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/top-level-collections-opened-by-windows-for-system-use (ms.date 01/11/2024). ⇒ a user-mode app (hidapi, `CreateFile`+`ReadFile`, WebHID-on-Windows underneath) **cannot read mouse-TLC input reports**; it can open the path without read/write access to query attributes/strings (behaviour long relied on by hidapi; current doc no longer states it → **UNVERIFIED**).
- Raw Input never hands mouse-TLC report bytes to apps: mouse TLC arrives as parsed `RIM_TYPEMOUSE`; `RAWHID.bRawData` only for non-mouse/keyboard TLCs (RAWINPUTHEADER dwType table). Vendor-defined TLCs (UsagePage ≥0xFF00, e.g. config/telemetry channels) are openable via hidapi or Raw Input `RIM_TYPEHID`.
- GameInput raw reports: GIP only (above). WinUSB: replacing the HID driver on a mouse interface would remove it from the OS input stack (and requires driver install) — out of scope.
- Therefore on Windows "device counts" = **RAWMOUSE.lLastX/lLastY from relative packets with no filter driver**, assumed 1 packet per HID report (W1/W4 to verify); firmware processing (CPI, angle snapping, smoothing, LOD, motion sync) is always included.

### 7-Win. Taxonomy rows

| Layer | Windows source | Units | Quantisation | Aggregation | Timestamp domain/res | Latency contributors |
|---|---|---|---|---|---|---|
| (1) device counts | not directly readable (mouse TLC exclusive); closest = (2) | sensor counts at configured CPI | integer | per HID report (firmware may sum sensor frames) | none from device | sensor frame, MCU, USB poll interval (bInterval)/wireless link |
| (2) OS raw events | Raw Input RAWMOUSE (relative); GameInput readings | counts ("mickeys"), wheel 1/120 notch | int32 (RawInput), int64 cumulative (GameInput) | expected 1 per report (W1); Win11 coalesces for background listeners; app batch-drain | RawInput: none (app QPC at dequeue, <1 µs res but measures dequeue); GameInput: µs "when the input was made" (base UNVERIFIED) | HIDCLASS/MOUHID/MOUCLASS, filter drivers, RIT, thread scheduling, queue drain cadence, background throttle |
| (3) OS cursor events | WM_MOUSEMOVE, WM_POINTER, GetCursorPos, GetMouseMovePointsEx, WinRT PointerPoint | screen px (DIP for WinRT) | integer px; sub-pixel carried internally | coalesced to latest when queue lags | GetMessageTime ms (tick); POINTER_INFO.PerformanceCount QPC at receipt; PointerPoint µs since boot | + ballistics, desktop/DPI scaling, message coalescing |
| (2/3?) WinRT MouseMoved | MouseDelta | "screen location" delta per docs; raw-ness UNVERIFIED | int | per event | none in args (UNVERIFIED) | CoreWindow dispatch |
| (4) browser | (other section) Chromium on Windows uses raw input for unadjustedMovement — not verified here | | | | | |

### Experiments (Windows)
- **W1 — pass-through:** capture USB HID reports (USBPcap/Wireshark on a separate test box, or a hardware USB analyzer) simultaneously with RawInput; assert Σ and per-packet equality of X/Y and 1:1 packet count at 1/2/4/8 kHz.
- **W2 — background coalescing:** same mouse, two processes: foreground RIM_INPUT reader and background RIDEV_INPUTSINK reader; compare event rates, Σcounts over 10 s, `MOUSE_MOVE_NOCOALESCE` flag distribution, on Win10 vs Win11 24H2/25H2.
- **W3 — API equivalence:** record RawInput, GameInput (`positionX` deltas per reading), WinRT MouseMoved, WM_MOUSEMOVE concurrently with EPP on/off and speed 1/10/20; raw-class APIs must be invariant to settings.
- **W4 — rate/timestamp:** inter-arrival histograms of QPC-at-receipt (per-message read on TIME_CRITICAL thread) vs GameInput µs timestamps; check `dwSampleRate` value; check whether GameInput gives one reading per report.
- **W5 — filter detection:** install RawAccel on a test VM/box with known curve; verify UpperFilters check flags it and Σcounts diverge from (W1) USB capture.
- **W6 — RDP/VM:** confirm absolute packets and flag 0x40 under RDP, Hyper-V, VMware, VirtualBox.

### Open / UNVERIFIED summary
125 Hz figure + exact background definition; coalesced-sum losslessness; GetMessageTime granularity; GameInput timestamp base & per-report granularity & raw-ness; WinRT MouseDelta raw-ness; MOUHID scaling; injected hDevice=0; EPP↔SPI_GETMOUSE mapping and whether modern EPP still assumes 125 Hz/400 dpi; UpperFilters default list.

---

## 2. Linux: evdev, libinput, X11, Wayland (accessed 2026-11-14)

### 2.0 Data path

```
sensor+MCU firmware ─► USB/BT HID input report (@ bInterval / BT interval)
  ─► usbhid URB completion ─► hid_input_report()
        ├─► HID-BPF hid_device_event programs (may rewrite/drop)        K drivers/hid/hid-core.c L2150
        ├─► hidraw_report_event()  → /dev/hidrawN  (raw bytes)          L2102-2106
        └─► hid-input: usage→EV_REL value, input_sync per report        L2115-2116
              ─► input core (drops zero REL, timestamps at flush) ─► evdev /dev/input/eventN
                    ├─► libinput (compositor / Xorg driver): accel profile, rotation, [Lua plugins ≥1.30]
                    │     ├─► Xorg: XI_RawMotion(raw=unaccel), XI_Motion/core (accel, cursor)
                    │     └─► Wayland compositor: wl_pointer (accel, logical px), zwp_relative_pointer_v1 (dx, dx_unaccel, utime)
                    │            └─► Xwayland: XI_RawMotion = dx_unaccel × surface scale
                    └─► any reader with permission (evdev crate, SDL/kmsdrm, games)
```

### 2.1 Kernel: hid-input → evdev semantics (primary source: kernel source at K)

| Property | Fact | Source |
|---|---|---|
| Value mapping | `hidinput_hid_event` ends with `input_event(input, usage->type, usage->code, value)`. A Generic-Desktop X/Y relative field becomes REL_X/REL_Y with the HID **logical value unchanged** (no unit or physical scaling). Exceptions: quirk paths such as `HID_QUIRK_X_INVERT` (ABS only), and device-specific HID drivers (`hid-logitech-hidpp` etc.), which may remap. | K `drivers/hid/hid-input.c` L1789 |
| Packetisation | `hid_report_raw_event` → `hidinput_report_event` → `input_sync()` **once per HID input report** (unless `HID_QUIRK_NO_INPUT_SYNC`). So an evdev packet ≈ one report. | hid-input.c L1798-1808; hid-core.c L2043, L2115-2116 |
| Zero suppression | Input core passes EV_REL only `if (… && value)`, so zero-valued axes are not emitted. A flush with fewer than 2 values (i.e. only SYN_REPORT) is not delivered (`if (dev->num_vals >= 2)`). **A report with dx = dy = 0 and no button change produces no evdev event.** Report rate therefore cannot be read off idle reports. | K `drivers/input/input.c` L267-269, L340-351 |
| Hi-res wheel | `hi_res = value * 120 / resolution_multiplier`, accumulated into `REL_WHEEL` notches. Both `REL_WHEEL` and `REL_WHEEL_HI_RES` are emitted. The doc says 120 = one detent and "may be a fraction of 120" when the device is hi-res. | hid-input.c L1530-1556, L947; K `Documentation/input/event-codes.rst` L204-209 |
| SYN semantics | SYN_REPORT separates "packets of input data changes occurring at the same moment in time". SYN_DROPPED means the client buffer overran: discard up to the next SYN_REPORT and resync. | event-codes.rst L97-118 |
| Acceleration | **None in the kernel.** evdev passes driver values; acceleration lives in libinput, the Xorg drivers or the compositor. Firmware processing upstream is untouched. | (structure of the above; no accel code in `input.c`/`evdev.c`) |
| Timestamp: when | `input_get_timestamp()` lazily sets `ktime_get()` the first time it is requested in a packet unless the driver called `input_set_timestamp()`. evdev requests it in `evdev_events()` at flush. The timestamp is reset after each flush. **The HID core never calls `input_set_timestamp`** (grep of `hid-core.c`, `hid-input.c`, `usbhid/hid-core.c`, `hidraw.c`). So the evdev time = **host time when the input core flushes the report's SYN_REPORT** (USB completion → HID parse → flush). It is not device time, and it is shared by all events of the packet. | input.c L2092-2131, L340-351; K `drivers/input/evdev.c` L296-313 |
| Timestamp: clock | Per-client `clk_type`, default `INPUT_CLK_REAL` (= 0, zero-initialised), i.e. **CLOCK_REALTIME by default**. `EVIOCSCLOCKID` accepts `CLOCK_REALTIME`, `CLOCK_MONOTONIC`, `CLOCK_BOOTTIME`. Changing it flushes the queue and inserts SYN_DROPPED. | evdev.c L177-205, L1125-1129; K `include/linux/input.h` L38-43; K `include/uapi/linux/input.h` L243 |
| Timestamp: format | `struct input_event { timeval time; u16 type, code; s32 value }`. On 32-bit with time64 it uses `__sec/__usec` fields. **µs resolution** (ktime ns truncated). Doc: "time at which the event happened". | K `Documentation/input/input.rst` L261-270; uapi `input.h` L28-40 |
| Buffering | Per-client ring = `roundup_pow_of_two(max(hint_events_per_packet × 8, 64))`. `hint` = 1 + #REL axes + 7. Local mouse: REL = {X, Y, HWHEEL, WHEEL, WHEEL_HI_RES, HWHEEL_HI_RES} gives hint 14, ring **128 events** (≈ 42 three-event motion packets ≈ 42 ms at 1 kHz, **≈ 5 ms at 8 kHz**). Overrun gives SYN_DROPPED. A capture thread must drain promptly and log drops. | evdev.c L12-13, L457-463; input.c L2199-2229; local `/sys/class/input/event7/device/capabilities/rel` = `1943` |
| Grab | `EVIOCGRAB` gives the grabbing client the exclusive stream. **Other handlers (X server/compositor) stop receiving**, so passive measurement must not grab. | evdev.c L337-363, L1089-1093 |
| USB poll | `usbhid.mousepoll` module param overrides the endpoint interval. The full-speed-interval-on-high-speed fixup is in the same function. The local receiver is full-speed with bInterval 1 (1 ms). | K `drivers/hid/usbhid/hid-core.c` L51-53, L1123-1146; local sysfs `ep_81/interval` = `1ms`, `speed` = `12` |

### 2.2 Permissions (no change made)

- **evdev nodes.** `/dev/input/event*` and `/dev/input/mice` are `root:input 0660` (local; `50-udev-default.rules` L49 `SUBSYSTEM=="input", GROUP="input"`).
- **No uaccess for mice.** systemd's `70-uaccess.rules` tags input devices with `uaccess` **only for `ID_INPUT_JOYSTICK`** (local systemd 255: `/usr/lib/udev/rules.d/70-uaccess.rules` L63). Mice and keyboards are excluded, which is the anti-keylogger policy. The upstream main `rules.d/70-uaccess.rules.in` has no input-mouse rule either (fetched 2026-11-14; https://github.com/systemd/systemd/blob/main/rules.d/70-uaccess.rules.in).
- **logind `TakeDevice()`** "allows a session controller to get a file descriptor for a specific device". Only the session controller (the compositor) gets evdev fds this way, so an ordinary app cannot. https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html
- **Local consequences.** The user is not in `input`, so the evdev and libinput backends are unavailable unless the user changes group membership or ACLs (a user decision, not made here). hidraw for the Glorious receiver is open via the user's own rule (see §5).

### 2.3 libinput (primary: LI source and docs; local version 1.25.0)

| Topic | Fact | Source |
|---|---|---|
| Unaccelerated deltas | `libinput_event_pointer_get_dx_unaccelerated`: "raw device coordinates… subject to the device's native resolution… Any rotation applied to the device also applies to unaccelerated motion". | LI `src/libinput.h` ~L1380-1402 |
| DPI normalisation | "For devices with a resolution of 1000dpi and higher, motion events are normalized to a default of 1000dpi before pointer acceleration". **"This normalization only applies to accelerated coordinates, unaccelerated coordinates are left in device-units."** Custom-profile deltas are not normalised. DPI comes from the udev `MOUSE_DPI` property (hwdb). Locally it is not set, so the 1000 dpi default applies. | LI `doc/user/normalization-of-relative-motion.rst`; local `udevadm info` |
| Profiles | **adaptive** (default), **flat** ("constant factor applied to all device deltas"), **custom** (since 1.23). `NONE = 0` is a placeholder for devices without accel. | LI `doc/user/pointer-acceleration.rst` L18-28, L185-194; `libinput.h` L5734-5757, L5806 |
| Flat details | `accelerated = factor × unaccelerated`, with `factor = max(0.005, 1 + speed)`, applied to device-unit coordinates. **Flat profile at speed 0 gives accelerated == unaccelerated == device counts.** | LI `src/filter-flat.c` L44-61, L118 |
| Dropping | `if (normalized_is_zero(accel)) return;` means a frame whose *accelerated* result is zero is not emitted, so its raw deltas are lost too. Relevant only to custom curves mapping to 0. | LI `src/evdev-fallback.c` L179-209 |
| Timestamps | `libinput_event_pointer_get_time_usec`: µs; "may not always increase". libinput sets the evdev clock to CLOCK_MONOTONIC (`libevdev_set_clock_id`), so the value is the kernel flush time, on monotonic. | `libinput.h` L1325-1336; LI `src/evdev.c` L2369, L2990 |
| Lua plugins | Since **1.30**. A plugin may "modify the event stream seen by this device before it is passed to libinput", from `/etc/libinput/plugins` and `/usr/lib*/libinput/plugins`. "Not loaded by default"; the compositor must call `libinput_plugin_system_load_plugins()`. **Contamination path for "unaccelerated" values on ≥ 1.30 systems.** Not present locally (1.25). | LI `doc/user/lua-plugins.rst` L7-36; `libinput.h` `@since 1.30` ~L3853 |
| Hi-res wheel | `libinput_event_pointer_get_scroll_value_v120` (since 1.19). | `libinput.h` ~L1770 |

### 2.4 X11 (Xorg and Xwayland)

**Xorg + xf86-input-libinput (the local stack)**
- The driver posts both values: `x = get_dx()` (accelerated) and `ux = get_dx_unaccelerated()` via `valuator_mask_set_unaccelerated(mask, 0, x, ux)` (xf86-input-libinput `src/xf86libinput.c` L1645-1671).
- It disables the X server's own acceleration (`AccelerationScheme` = `none`, L4135), so libinput does all ballistics.

**Raw event content**
- X server `GetPointerEvents` → `fill_pointer_events`. The raw event gets `set_raw_valuators(raw, &mask, TRUE, data_raw)`, which **uses the unaccelerated values when present** (XS `dix/getevents.c` L211-232, L1392).
- `axisvalues` (non-raw) holds the post-transform/accel values (L1409-1417).
- XI2 spec: `axisvalues_raw` = "Untransformed valuator data in device-native resolution". RawEvent provides "the raw data as supplied by the driver and transformed data… Transformations include… axis clipping and acceleration" (xorgproto `specs/XI2proto.txt` L2555-2606).
- **⇒ On Xorg+libinput, `XI_RawMotion` raw values = libinput `dx_unaccelerated` = evdev counts (plus rotation, plus plugins on ≥ 1.30).** With xf86-input-evdev instead, raw = evdev values (driver not installed locally).

**Global and permission-free**
- "Clients supporting XI 2.1 or later receive raw events at all times, even when the device is grabbed by another client". RawEvents go to all root windows (XI2proto L2575-2583).
- ⇒ any X client can capture every mouse's raw motion **without input-group permission and regardless of focus**. That is useful for KineForge and is also a privacy property of X11.
- `sourceid` (XI ≥ 2.1) identifies the physical (slave) device. The master device id merges all mice (see the winit note in §4).

**Timestamps**
- `GetPointerEvents` stamps `ms = GetTimeInMillis()` **when the server processes the driver's post** (XS `getevents.c` L1664-1667). It does not use the libinput/evdev time; xf86-input-libinput never reads `get_time*` (grep).
- `GetTimeInMillis` uses `CLOCK_MONOTONIC_COARSE` if its resolution is ≤ 1 ms, else `CLOCK_MONOTONIC`; it returns 32-bit ms (wraps about every 49.7 days) (XS `os/utils.c` L460-482). Locally `CONFIG_HZ=1000`, so the coarse clock has 1 ms ticks.
- ⇒ X11 raw timestamps: **ms, server-dispatch time, monotonic-domain**.

**Core and XI2 pointer motion**
- Core MotionNotify: accelerated, integer root coordinates.
- XI2 `XI_Motion`: FP16.16 sub-pixel positions, accelerated.
- Both are clipped at screen edges.

**Xwayland**
- Relative motion from `zwp_relative_pointer_v1` is posted `POINTER_RAWONLY` with `dx_unaccel × global_surface_scale × viewport_scale` (XS `hw/xwayland/xwayland-input.c` L706-731). Raw values are therefore **scaled** whenever Xwayland applies a surface scale or viewport.
- Relative pointer events are only delivered while an Xwayland surface has pointer focus (protocol, §2.5). So **XI2 raw is no longer global under Wayland.**

### 2.5 Wayland

**`zwp_relative_pointer_v1.relative_motion`** (wayland-protocols `unstable/relative-pointer/relative-pointer-unstable-v1.xml` L76-128)
- Args: `utime_hi`, `utime_lo` ("64 bit timestamp with microsecond granularity"; **base not specified**), `dx`, `dy`, `dx_unaccel`, `dy_unaccel` (all `wl_fixed` 24.8, so 1/256 resolution).
- "Same dimension as regular wl_pointer motion events" (surface-local logical coordinates). Not clipped at screen edges.
- dx_unaccel is the delta "before having applied motion acceleration and other transformations such as normalization".
- **Explicitly: "the non-accelerated delta does not represent 'raw' events as they were read from some device."**
- Shares wl_pointer focus and "will only emit events when it has focus". So there is **no background capture** and no global input access on Wayland.

**`zwp_pointer_constraints_v1`** (`pointer-constraints-unstable-v1.xml`)
- `lock_pointer` / `confine_pointer` with lifetime `oneshot` or `persistent` (L75-95). While locked, "wl_pointer objects… will not emit any wl_pointer.motion events" (L186-200).
- Relative motion continues. `set_cursor_position_hint` is only a hint.
- `already_constrained` is a protocol error (L63-71).

**`zwp_input_timestamps_v1`**
- ns timestamps "in the same clock domain" as the wl_pointer/keyboard/touch ms timestamp (`input-timestamps-unstable-v1.xml` L120-141).
- Per wayland.app only **Weston** implements it, so wl_pointer events elsewhere are **ms**. https://wayland.app/protocols/input-timestamps-unstable-v1

**`wp_pointer_warp_v1`** (staging): warp request. Supported by Mutter, KWin, Hyprland, Jay and Muffin; not for locking. https://wayland.app/protocols/pointer-warp-v1

**Compositor implementations (source-verified)**

| Compositor | dx_unaccel source | utime source | Notes |
|---|---|---|---|
| **Mutter** (GNOME) | `clutter_event_get_relative_motion` → native backend libinput `get_dx_unaccelerated` (stored as float) | `clutter_event_get_time_us` (libinput µs); falls back to `ms × 1000` | Only the focused client gets it. `src/wayland/meta-wayland-pointer.c` L378-420; `src/backends/native/meta-seat-impl.c` (unaccel at ~L894-945) |
| **KWin** (KDE) | libinput `get_dx_unaccelerated`, sent **unscaled**. Accelerated `dx` is scaled `toSurfaceLocal()` | libinput `time_usec` (µs) | `src/wayland/relativepointer_v1.cpp` L74-92; `src/backends/libinput/events.cpp` L155-170 |
| **wlroots / sway** | libinput `get_dx_unaccelerated` | **`usec_to_msec(time_usec)` then sway sends `time_msec × 1000`, so µs field with ms resolution** | wlroots `backend/libinput/pointer.c` L32-36; sway `sway/input/cursor.c` L293-296. Other wlroots compositors: UNVERIFIED, likely the same since the event struct only carries ms |

**Protocol support** (wayland.app, 2026-11-14)
- relative-pointer: Cage, COSMIC, GameScope, Hyprland, Jay, KWin 6.7, Labwc, Louvre, Mango, Mir, Muffin, Mutter 51, niri, phoc, Qtile, river, Sway 1.11, Wayfire, Weston. Not Treeland. https://wayland.app/protocols/relative-pointer-unstable-v1
- Pointer-constraints support was not fetched separately: UNVERIFIED, but universal among the same set in practice.

**Fractional values**
- `wl_fixed` allows 1/256 steps. For a mouse, libinput unaccel deltas are integers, so dx_unaccel is integral **unless** the compositor or Xwayland scales it (Xwayland does; Mutter and KWin do not; Firefox multiplies accelerated dx by the fractional scale, see §6).
- Accelerated `dx` is fractional: adaptive profile and logical-px scaling.

**InputCapture portal + libei**
- The portal lets a client capture input after the pointer crosses a barrier, with the user's consent (`org.freedesktop.portal.InputCapture.xml`).
- libei relative motion is "in logical pixels" with µs frame timestamps (libei `proto/protocol.xml` L930, L978-991). Whether the compositor feeds accelerated or unaccelerated deltas: UNVERIFIED.
- It captures (consumes) input, so it is **not** a passive background tap.

### 2.6 Linux summary per source

| Source | Needs | Delta semantics | Per-device | Timestamp | Focus |
|---|---|---|---|---|---|
| hidraw | node permission (udev rule) | HID report bytes, i.e. device counts (post HID-BPF) | yes (per interface) | none; app `clock_gettime` at `read()` | global |
| evdev | `input` group / ACL | HID logical values, one packet per report, zeros dropped | yes | kernel flush, µs, clock per `EVIOCSCLOCKID` (default REALTIME) | global |
| libinput (own context) | same as evdev | unaccel = device units (+rotation, +plugins); accel = profile | yes | µs CLOCK_MONOTONIC (kernel flush) | global |
| Xorg XI_RawMotion | X connection only | libinput unaccel (xf86-input-libinput) | `sourceid` | X server dispatch, ms, MONOTONIC(_COARSE) | **global** |
| Xwayland XI_RawMotion | X connection | `dx_unaccel × scale` | per seat (one relative device) | ms server time | focused only |
| Wayland relative pointer | wl_display of the window | compositor's libinput unaccel (Mutter/KWin/wlroots) | per seat, no device id | µs (ms on wlroots/sway), base unspecified (in practice CLOCK_MONOTONIC: UNVERIFIED) | focused only |
| wl_pointer / XI_Motion / core | – | accelerated, logical/screen px, clipped | no | ms | focused |

---

## 3. macOS (accessed 2026-11-14)

Apple doc pages were read through the documentation JSON endpoint `https://developer.apple.com/tutorials/data/documentation/<path>.json`. They render at `https://developer.apple.com/documentation/<path>`.

### 3.0 Data path

```
HID report ─► IOHIDFamily (kernel IOHIDDevice/IOHIDEventService → IOHIDEvent pointer X/Y, AbsoluteTime stamp)
  ├─► user-space IOHIDDevice/IOHIDManager clients (input-report callbacks)  [TCC Input Monitoring for Mouse/Keyboard/TouchPad]
  └─► IOHIDEventSystem filters: IOHIDPointerScrollFilter keeps the original event and APPENDS an accelerated child (kIOHIDAccelerated)
        └─► WindowServer → CGEvent (kCGMouseEventDeltaX = accelerated int; kCGEventUnacceleratedPointerMovementX = unaccelerated)
              └─► NSEvent (deltaX; coalesced by default) ─► app
  └─► GameController GCMouse (mouseMovedHandler: "raw mouse movement delta values")
```

### 3.1 Sources

| API | What it is | Evidence | Raw? |
|---|---|---|---|
| `NSEvent.deltaX/deltaY` | "x-coordinate change for scroll wheel, mouse-move, mouse-drag, and swipe events" (CGFloat points). The doc is silent on acceleration. | `appkit/nsevent/deltax` | **Accelerated.** Chromium and WebKit use it as the *adjusted* path and switch to the unaccelerated CGEvent field for `unadjustedMovement` (§6.3/§6.4 citations). Fractional. |
| `kCGMouseEventDeltaX/Y` (field 4/5) | "integer field that contains the horizontal mouse delta since the last mouse movement event" | SDK `CGEventTypes.h` L160-166; `coregraphics/cgeventfield/mouseeventdeltax` | Accelerated, integer (inferred from browsers' usage; Apple text does not say). UNVERIFIED |
| `kCGEventUnacceleratedPointerMovementX/Y` (field **170/171**) | Public SDK field, comment "Unaccelerated pointer movement". Read by Chrome (`CGEventGetIntegerValueField`) and WebKit (`CGEventGetDoubleValueField`). | MacOSX11.3.sdk `CGEventTypes.h` L380-382; Chromium `components/input/web_input_event_builders_mac.mm` L152-162; WebKit `PlatformEventFactoryMac.mm` L712-718 | **Pre-acceleration.** IOHIDFamily's `IOHIDPointerScrollFilter::accelerateEvent` leaves the original pointer event untouched and appends an accelerated copy flagged `kIOHIDAccelerated` (`IOHIDEventSystemPlugIns/IOHIDPointerScrollFilter.cpp` L537-565). Whether field 170 equals HID counts 1:1 (versus resolution-scaled) is **UNVERIFIED** (experiment M1). Fractional parts are possible (WebKit reads a double). |
| `GCMouse` / `GCMouseInput.mouseMovedHandler` (macOS 11+) | "This profile provides **only raw mouse movement delta values**". The handler signature is `(GCMouseInput, Float deltaX, Float deltaY)` with no timestamp. `GCPhysicalInputProfile.lastEventTimestamp` = "time of the most recent change". | `gamecontroller/gcmouseinput`, `gcmousemoved`, `gcphysicalinputprofile/lasteventtimestamp` | Apple says "raw". Acceleration-independence, units and per-report granularity are **UNVERIFIED** (M1/M3). SDL `main` adopts it as its raw path from macOS Sonoma (§4.2). |
| `GCController.shouldMonitorBackgroundEvents` | Since macOS 11.3, default `false`: no input forwarded "until the app becomes the frontmost". | `gamecontroller/gccontroller/shouldmonitorbackgroundevents` | Foreground-only by default |
| IOHIDManager / IOHIDDevice input-report callbacks | Raw HID report bytes per report. `IOHIDDeviceRegisterInputReportWithTimeStampCallback` provides a per-report timestamp. `IOHIDValue` timestamps "should represent OS AbsoluteTime" (mach_absolute_time units). | SDK `IOHIDDevice.h` L412-428; `IOHIDValue.h` L63-77 | **Device counts** (tier 1). See TCC below |

### 3.2 TCC (Input Monitoring) and exclusive access

- **Which devices.** The kernel `IOHIDDevice::start` sets `kIOHIDRequiresTCCAuthorizationKey` for any device conforming to **GD Keyboard, GD Mouse, or Digitizer TouchPad** (`IOHIDFamily/IOHIDDevice.cpp` L526-531, tag 2238.120.5).
- **The prompt.** User-space `IOHIDDeviceClass initConnect` then calls `IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)` and logs "TCC deny IOHIDDeviceOpen" on refusal (`IOHIDLib/IOHIDDeviceClass.m` L495-531).
- **Header text.** `kIOHIDRequestTypeListenEvent` is a "Request to listen to event through IOHIDManager/IOHIDDevice API. Access must be granted by the user". It is requested implicitly in `IOHIDManagerOpen/IOHIDDeviceOpen` (SDK `IOHIDLib.h` L150-165, L195-213; macOS 10.15+ per `iokit/iohidrequesttype/kiohidrequesttypelistenevent`).
- **Inference: the vendor channel shares the gate.** Because the key is per IOHIDDevice (USB interface), a vendor-defined collection on the *same interface* as the mouse TLC is also TCC-gated (UNVERIFIED; M2).
- **Seize vs non-seize.** `kIOHIDOptionsTypeSeizeDevice` "will prevent the system and other clients from receiving events" (`IOHIDFamily/IOHIDKeys.h` L234-243). **hidapi on macOS calls `hid_darwin_set_open_exclusive(1)` in `hid_init()` "for backward compatibility"** (`mac/hid.c` L462-470, L1560-1562). So a naive hidapi open of the mouse **steals it from the OS**. Call `hid_darwin_set_open_exclusive(0)` before opening.
- **Event taps.** `CGPreflightListenEventAccess` / `CGRequestListenEventAccess` exist since 10.15 for event taps. Whether a listen-only **mouse-only** tap requires it is UNVERIFIED.

### 3.3 Locking and cursor

- **Lock.** `CGAssociateMouseAndMouseCursorPosition(false)`: "events your application receives… have a constant absolute location but contain delta updates" (`coregraphics/cgassociatemouseandmousecursorposition(_:)`). winit and SDL both use it (§4).
- **Warp.** `CGWarpMouseCursorPosition` moves the cursor "without generating or posting an event" (`coregraphics/cgwarpmousecursorposition(_:)`). The post-warp local-event suppression interval often cited for games is UNVERIFIED here.

### 3.4 Timestamps and delivery

| Item | Fact | Source |
|---|---|---|
| `NSEvent.timestamp` | "seconds since system startup" (double) | `appkit/nsevent/timestamp` |
| `CGEventTimestamp` | "roughly, nanoseconds since startup" (uint64) | SDK `CGEventTypes.h` L134-135; `coregraphics/cgeventtimestamp` |
| `mach_absolute_time` | "increments monotonically in tick units…does not increment while the system is asleep". Apple prefers `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`. Required-reason API (fingerprinting declaration in `PrivacyInfo.xcprivacy`). | `kernel/1462446-mach_absolute_time` |
| IOHID value/report time | AbsoluteTime, set at kernel event creation (IOHIDEvent timestamp) | `IOHIDValue.h` L66; `IOHIDPointerScrollFilter.cpp` passes `IOHIDEventGetTimeStamp(event)` into the accelerator (L551) |
| Coalescing | `NSEvent.isMouseCoalescingEnabled`: "whether the system coalesces mouse movement events", **default true** (10.5+). NSEvent deltas are therefore sums over coalesced events unless disabled. | `appkit/nsevent/ismousecoalescingenabled` |
| ProMotion / high refresh | Not researched in depth. Relevant only to (7) photon latency and to how NSEvent coalescing aligns with frames: UNVERIFIED. | – |

**macOS verdict**
- Tier 1 (device counts): IOHID reports with Input Monitoring and a non-seize open.
- Tier 2 (OS raw): the CGEvent unaccelerated field or GCMouse, both with units UNVERIFIED.
- `NSEvent.deltaX` is tier 3 (OS pointer, accelerated, coalesced). winit and SDL 3.4 use it (§4).

---

## 4. Cross-platform Rust: winit, SDL3, other crates (researched 2026-11-13)

Access date for every URL below: **2026-11-13**. Method: shallow clones of tagged sources read locally (no code executed); crates.io API for metadata; GitHub API for issues/PRs.
Source snapshots used:
- winit **v0.30.13** (latest stable, released 2026-03-02) and winit **master @ f0abec90a3f9916818b70cde12fea40b797309ae** (2026-11-13; ≈ v0.31.0-beta.4, released 2026-10-24). https://github.com/rust-windowing/winit/releases
- SDL **release-3.4.20** (released 2026-11-03) plus SDL `main` where noted. https://github.com/libsdl-org/SDL/releases
- Line numbers below refer to those exact refs. `W30` = `https://github.com/rust-windowing/winit/blob/v0.30.13/`, `WM` = `https://github.com/rust-windowing/winit/blob/f0abec90a3f9916818b70cde12fea40b797309ae/`, `S34` = `https://github.com/libsdl-org/SDL/blob/release-3.4.20/`.

---

### 4.1 winit

#### API shape
- 0.30: `DeviceEvent::MouseMotion { delta: (f64, f64) }`, documented as "raw, unfiltered physical motion", units "unspecified… different devices may use different units", and "delivered regardless of input focus". `W30 src/event.rs#L463-L484`.
- 0.31 (master/betas): renamed to `DeviceEvent::PointerMotion { delta: (f64, f64) }` (changelog: "rename `DeviceEvent::MouseMotion` to `PointerMotion`", `WM winit/src/changelog/v0.31.md#L291`). Doc adds: "**Web:** Only returns raw data, not OS accelerated, if `CursorGrabMode::Locked` is used and browser support is available." `WM winit-core/src/event.rs#L750-L767`. The 0.30 extra `DeviceEvent::Motion{axis,value}` is gone in 0.31.
- **No timestamps on any input event (verified).** `ApplicationHandler::device_event(&mut self, event_loop, device_id: Option<DeviceId>, event: DeviceEvent)` — `WM winit-core/src/application/mod.rs#L205-L212`; `Instant` appears in `winit-core/src/event.rs` only in `StartCause::{ResumeTimeReached,WaitCancelled}` (L30-L34). Same in 0.30 (`W30 src/event.rs#L134-L138`).
  - Open tracking: issue #1194 "Input event timestamps" (open since 2019) https://github.com/rust-windowing/winit/issues/1194 ; **draft** PR #4562 "core: Add timestamps to window and device events" (draft, unmerged, last updated 2026-07-04) https://github.com/rust-windowing/winit/pull/4562 — proposes a `timestamp: Instant` arg: AppKit `NSEvent.timestamp` calibrated via `CACurrentMediaTime`; X11 `xev.time` anchored; Wayland `wl_pointer` ms + relative-pointer `utime_hi/lo` µs; **Win32 = `Instant::now()` in WindowProc** (no device/OS time); Web/Android/iOS `Instant::now()`. Earlier PR #4529 (AppKit only, open) https://github.com/rust-windowing/winit/pull/4529.
- `DeviceEvents` filter: `Always | WhenFocused (default) | Never` — `WM winit-core/src/event_loop/mod.rs#L585-L597`.

#### Per-backend data source of `MouseMotion`/`PointerMotion` (both refs unless stated)

| Backend | Source feeding the event | Units / type | Device id | Accel? | Notes / citation |
|---|---|---|---|---|---|
| Windows | `WM_INPUT` → `GetRawInputData` (one packet per message) → `RAWMOUSE.lLastX/lLastY` cast to f64 | integer counts as reported by Raw Input | `hDevice` (per physical device) | No (Raw Input bypasses pointer speed/EPP per MS docs) | `WM winit-win32/src/event_loop.rs#L2623-L2629` (WM_INPUT on hidden event-target window on the **event-loop thread**), `#L2657-L2675` (handler); `W30 src/platform_impl/windows/event_loop.rs#L2559-L2592`. Registration `raw_input.rs#L133-L160`: usage 1/2 mouse + 1/6 keyboard, flags `RIDEV_DEVNOTIFY` (+`RIDEV_INPUTSINK` only for `DeviceEvents::Always`); **no `RIDEV_NOLEGACY`, no `GetRawInputBuffer`**. **BUG: absolute packets treated as relative** — code tests `has_flag(usFlags, MOUSE_MOVE_RELATIVE)` where `has_flag(b,f)= b&f==f` (`winit-win32/src/util.rs#L46-L51`) and `MOUSE_MOVE_RELATIVE = 0x00` (https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse) ⇒ always true ⇒ `MOUSE_MOVE_ABSOLUTE` (RDP/VM/tablet, 0..65535 normalised coords) are emitted as huge "deltas". Matches open issue #2884 "Mouse Motion incorrect when using Remote Desktop" https://github.com/rust-windowing/winit/issues/2884. |
| X11 | XI2 `XI_RawMotion` selected on root for `XIAllMasterDevices` (`WM winit-x11/src/event_loop.rs#L731-L747`) → reads **`xev.raw_values`** (= protocol `axisvalues_raw`, "untransformed valuator data") for valuators 0/1 | f64 device-native units | `xev.deviceid` (for master-selected raw events this is the master pointer; physical device is `sourceid`, used only for a type filter) ⇒ multiple mice likely share one `DeviceId` — **UNVERIFIED empirically**; SDL uses `sourceid` | No (raw_values) — but see xf86-input-libinput note | `WM winit-x11/src/event_processor.rs#L1666-L1706`; `W30 src/platform_impl/linux/x11/event_processor.rs#L1429-L1473`. XI2 spec RawEvent: https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/raw/master/specs/XI2proto.txt (§RawEvent: "axisvalues_raw: Untransformed valuator data in device-native resolution"; XI≥2.1 clients get raw events "at all times, even when the device is grabbed by another client"). `xev.time` is only stored via `set_timestamp`, not exposed. |
| X11 (driver detail) | With **xf86-input-libinput**, raw valuators = `libinput_event_pointer_get_dx_unaccelerated()` via `valuator_mask_set_unaccelerated(mask, 0, x, ux)` | libinput "raw device coordinates… subject to the device's native resolution" (+device rotation if configured) | – | No | https://gitlab.freedesktop.org/xorg/driver/xf86-input-libinput/-/raw/master/src/xf86libinput.c (`xf86libinput_handle_motion`, ~L1646-L1671); https://gitlab.freedesktop.org/libinput/libinput/-/raw/main/src/libinput.h (~L1380-L1402) |
| Wayland | `zwp_relative_pointer_v1.relative_motion` → **`dx_unaccel, dy_unaccel`**; `utime_hi/lo` and accelerated `dx/dy` **discarded** (`..` pattern) | f64 from `wl_fixed` (24.8 fixed point; fractional possible) | **none** (`Event::DeviceEvent { event }` with no id; `push_device_event` `WM winit-wayland/src/event_loop/sink.rs#L30-L32`) | No | `WM winit-wayland/src/seat/pointer/relative_pointer.rs#L55-L73`; `W30 …/wayland/seat/pointer/relative_pointer.rs#L76`. Relative pointer object created per seat when pointer capability appears (`WM winit-wayland/src/seat/mod.rs#L175`). Only delivered while the surface has pointer focus (protocol semantics). Changelog 0.27: "On Wayland, report unaccelerated mouse deltas". |
| macOS | `NSApplication sendEvent:` swizzle → for `MouseMoved`/`*MouseDragged` uses **`NSEvent.deltaX/deltaY`** | f64 points (OS-processed; see macOS section for accel semantics — **OS-accelerated, UNVERIFIED here**) | `None` | Yes (OS pointer pipeline) | `WM winit-appkit/src/app.rs#L103-L121`; `W30 src/platform_impl/macos/app.rs#L103-L123`. Only sees events routed to this app ⇒ "regardless of focus" doc claim does not hold on macOS (inferred from mechanism, **UNVERIFIED** empirically). No IOHID/GCMouse path. |
| Web (master/0.31) | `pointermove` listener on window; iterates `getCoalescedEvents()` if supported; **Chromium**: `movementX/Y` treated as **physical px**; **Gecko** (with coalesced support): delta reconstructed from `offsetX/Y` differences + parent `movementX` (Firefox bug 1753724: wrong movement in coalesced events); others: movementX as **logical (CSS) px**; all converted `.to_physical(scale_factor)` | f64 "physical px" (engine-dependent assumption) | `pointerId` | Raw only if lock got `unadjustedMovement` | `WM winit-web/src/event_loop/runner.rs#L308-L348`, `winit-web/src/web_sys/event.rs#L176-L221` (cites w3c/pointerlock#42 on movementX coordinate-space disagreement https://github.com/w3c/pointerlock/issues/42 and https://bugzilla.mozilla.org/show_bug.cgi?id=1753724), `event.rs#L327-L338` (coalesced; Safari lacks it, WebKit bug 210454). Under pointer lock on Gecko, offsets don't move ⇒ whole parent movement attributed to first coalesced sample (sum preserved, sub-frame distribution lost — inferred). |
| Web lock (master) | `CursorGrabMode::Locked` → `requestPointerLock({unadjustedMovement:true})` **iff** `requestPointerLock()` returns a Promise **and not Chrome-on-Linux** (hard-coded exclusion citing https://issues.chromium.org/issues/40833850); otherwise plain `requestPointerLock()`. **No fallback** if the unadjusted request rejects (just logs). Exposed as `ActiveEventLoopExtWeb::is_cursor_lock_raw()` (heuristic, not a measurement) | | | | `WM winit-web/src/lock.rs#L10-L62`; changelog `v0.31.md#L165-L168`, `#L242-L243`. Open issue #4831 (2026-11-10): lock fails on **Firefox/Wayland** with winit 0.31.0-beta.4, suspected `unadjustedMovement` https://github.com/rust-windowing/winit/issues/4831 |
| Web (0.30.13) | Same delta logic, but lock is **plain `requestPointerLock()`** (no `unadjustedMovement`) ⇒ OS-accelerated browser movement | | | Yes | `W30 src/platform_impl/web/web_sys/canvas.rs#L177`, `web_sys/event.rs#L104-L140` |
| Orbital | `dx,dy` from Orbital mouse-relative event | | `None` | ? | `WM winit-orbital/src/event_loop.rs#L451` |
| iOS / Android | **Never emitted** (no `PointerMotion` producer in `winit-uikit`/`winit-android`) | | | | grep of `WM winit-uikit/src`, `winit-android/src` |

#### `CursorGrabMode` support matrix (docs `WM winit-core/src/window.rs#L1683-L1708`; `W30 src/window.rs` identical)

| | Confined | Locked | Mechanism |
|---|---|---|---|
| Windows | ✔ | ✔ since **0.30.10** (changelog `v0.30.md#L34`) | `ClipCursor` to client rect; Locked = `ClipCursor` to a **1×1 rect at current cursor pos** (`WM winit-win32/src/window_state.rs#L555-L567`) |
| X11 | ✔ (pointer grab confine) | **✘ NotSupported** ("locked cursor is not implemented on X11", `WM winit-x11/src/window.rs#L1864-L1868`) | app must warp itself; raw motion still flows |
| Wayland | ✔ | ✔ | `zwp_pointer_constraints_v1` confine/lock (`WM winit-wayland/src/seat/pointer/mod.rs#L274-L294`); `set_cursor_position` only valid while Locked (cursor hint) |
| macOS | **✘ NotSupported** | ✔ | `CGAssociateMouseAndMouseCursorPosition(false)` (`WM winit-appkit/src/window_delegate.rs#L1635-L1643`) |
| Web | ✘ | ✔ | Pointer Lock (see above) |
| iOS/Android | ✘ | ✘ | |
| Orbital | ✔ (0.29 changelog) | ? | |

#### Known performance issues with high polling rates
- Windows: every WM_INPUT is processed individually on the UI thread with `GetRawInputData`; legacy mouse messages are not suppressed (no `RIDEV_NOLEGACY`). PR #3609 "On Windows, improve the performance for high polling rate mouse" (open; `WM_NCHITTEST` flood, author notes the naive fix breaks capture) https://github.com/rust-windowing/winit/pull/3609 ; Bevy issue #12679 "Frame rate drop caused by high polling rate mouse" (≥4000 Hz, max frame time 10→80 ms, Win11) https://github.com/bevyengine/bevy/issues/12679 ; winit #3689 "Rapid cursor movement causes frame loss" (closed) https://github.com/rust-windowing/winit/issues/3689.
- macOS: #1418 "Continuous events like MouseMotion … seem laggy" (open since 2020) https://github.com/rust-windowing/winit/issues/1418.
- Process-wide Raw Input constraint (critical for any custom capture next to winit/SDL): "Only one window per raw input device class may be registered to receive raw input within a process (the window passed in the last call to RegisterRawInputDevices)… should not be used from a library" — https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerrawinputdevices . winit registers mice on its event-target window at `EventLoop::new` (`WM winit-win32/src/event_loop.rs#L239-L247`) and again whenever `listen_device_events` is called (`#L507`). A KineForge raw-input thread must either be the last registrant and tell winit `DeviceEvents::Never` *before* registering (winit's `Never` = `RIDEV_REMOVE` with NULL hwnd, which would also remove a later-registered KineForge target) — ordering matters.

#### Verdict for KineForge
winit `MouseMotion`/`PointerMotion` = **"OS input events, raw API" on Windows/X11/Wayland (no timestamps, Windows absolute bug, X11 device merge, Wayland no device id), "OS pointer events (accelerated)" on macOS, and "browser pointer events, locked, unadjusted-or-not by heuristic" on Web.** It must never be labelled device counts without per-platform qualification, and cannot supply timing. Use winit for windows/surfaces; capture input through dedicated backends.

---

### 4.2 SDL3 (release-3.4.20; `main` = future 3.6 where noted)

#### Event struct
`SDL_MouseMotionEvent { type, reserved, Uint64 timestamp /* ns, "populated using SDL_GetTicksNS()" */, windowID, SDL_MouseID which /* "mouse instance id in relative mode, SDL_TOUCH_MOUSEID…, or 0" */, state, float x, y, xrel, yrel }` — `S34 include/SDL3/SDL_events.h#L453-L465`. **No raw-motion event type exists** in 3.4.20 or `main` (`SDL_EVENT_MOUSE_MOTION … SDL_EVENT_MOUSE_REMOVED` only; checked `include/SDL3/SDL_events.h` on `main`, 2026-11-13). Multi-mouse: `SDL_GetMice()`, `SDL_EVENT_MOUSE_ADDED/REMOVED` (`S34 include/SDL3/SDL_mouse.h#L228-L245`), but `which` is only meaningful in relative mode.

#### Clock
`SDL_GetTicksNS()` = ns since SDL init from `SDL_GetPerformanceCounter()`: Linux/Unix `clock_gettime(CLOCK_MONOTONIC_RAW)` (not Android) (`S34 src/timer/unix/SDL_systimer.c#L54-L62,#L87-L107`); Windows `QueryPerformanceCounter` (`src/timer/windows/SDL_systimer.c`); Apple: `CLOCK_MONOTONIC_RAW` if `clock_gettime` available else `mach_absolute_time` (same file). Note kernel evdev/libinput/Wayland stamps are CLOCK_MONOTONIC (slewed), SDL base is MONOTONIC_RAW ⇒ ppm-level drift between domains (inference).

#### Relative mode per platform

| Platform | Source in relative mode | Delta passed on | Timestamp actually stored | Mouse id | Citations |
|---|---|---|---|---|---|
| Windows | **Dedicated thread** `SDLRawInput`: message-only `HWND_MESSAGE` window, `RegisterRawInputDevices` (mouse `dwFlags=0` ⇒ foreground only, no NOLEGACY), `SetThreadPriority(TIME_CRITICAL)`, `MsgWaitForMultipleObjects(QS_RAWINPUT)` + **`GetRawInputBuffer`** batches; only enabled while relative mode is on (`WIN_SetRelativeMouseMode → WIN_SetRawMouseEnabled`) | `lLastX/lLastY` → float; **absolute packets handled** (RDP: recenters with "wobble" warps, drops jumps > h/6; tablets: drops jumps >32; else converts) ; filters non-mouse sources via `ulExtraInformation` | **Synthesized**: per batch, mouse packets get times **linearly interpolated** between `poll_start` and `poll_finish` (`SDL_GetTicksNS`); `poll_start` = previous poll if idle < 125 µs ("usb_8khz_interval") — not device/OS time | `hDevice` | `S34 src/video/windows/SDL_windowsrawinput.c#L57-L139`; `SDL_windowsevents.c#L805-L866` (poll+interpolation), `#L556-L700` (`WIN_HandleRawMouseInput`, RDP logic); `SDL_windowsmouse.c#L693-L696` |
| Windows non-relative | `WM_MOUSEMOVE` (cursor, accelerated) | xrel = position diff | `GetMessageTime` ms mapped onto ticks with offset/wrap/clamp (`WIN_GetEventTimestamp`, `SDL_windowsevents.c#L157-L190`) | 0 | |
| X11 | XI2 `XI_RawMotion` (selected on `XIAllMasterDevices`) only when `relative_mode && keyboard focus`; uses **`raw_values`** unless `SDL_HINT_MOUSE_RELATIVE_SYSTEM_SCALE=1` (then transformed `valuators.values`); absolute valuators converted to deltas | float | **BUG (source-verified, not run): passes `rawev->time` (X server Time, 32-bit ms) directly as the ns timestamp** → stored nonzero, so `SDL_PushEvent` does not replace it (`SDL_events.c#L1808-L1809`). Other X11 events use `X11_GetEventTimestamp()` which just returns `SDL_GetTicksNS()` ("FIXME: Get the event time in the SDL tick time base", `SDL_x11events.c#L1070-L1074`). Same call present on `main` (`SDL_x11xinput2.c#L140`). | `rawev->sourceid` (per slave device) | `S34 src/video/x11/SDL_x11xinput2.c#L101-L141,#L586-L601,#L283-L315`; `X11_SetRelativeMouseMode` returns Unsupported without XI2 (`SDL_x11mouse.c#L448-L454`). Related fixed bugs: #16163 XWayland valuator-mode corruption https://github.com/libsdl-org/SDL/issues/16163 ; #14926 Xvnc absolute values https://github.com/libsdl-org/SDL/issues/14926 |
| Wayland | `zwp_relative_pointer_v1`: stores dx, dy, **dx_unaccel, dy_unaccel**; dispatch uses **unaccel** unless system-scale hint set (then accelerated × `pointer_scale`) | float (wl_fixed) | relative-pointer **`utime` µs** mapped by `Wayland_AdjustEventTimestampBase`: offset fixed at first event (`now - ts`), then clamped so ts ≤ now (offset only ever decreases) — preserves inter-event spacing, absolute anchor biased by first-event latency | per **seat** (`seat->pointer.sdl_id`), not per device | `S34 src/video/wayland/SDL_waylandevents.c#L199-L215,#L1184-L1201,#L1319-L1340`; other pointer events use `zwp_input_timestamps_v1` ns if available else `wl_pointer` ms (`#L220-L267`) |
| macOS (3.4.x) | `NSEvent.deltaX/deltaY` from `Cocoa_HandleMouseEvent` (+ warp compensation) with `CGAssociateMouseAndMouseCursorPosition(NO)` | float points, **OS-processed** (no `ApplySystemScale` on macOS; the "system scale" hint cannot un-accelerate) | `NSEvent.timestamp` (s) → ns, offset-mapped/clamped like Wayland (`SDL_cocoaevents.m#L584-L600`) | `SDL_DEFAULT_MOUSE_ID` (single) | `S34 src/video/cocoa/SDL_cocoamouse.m#L603-L614,#L338-L388`; `SDL_cocoavideo.m#L213` ("We could use GCMouse… as is done in SDL_uikitevents.m") |
| macOS (`main`, future **3.6**) | **GCMouse** `mouseMovedHandler` "for raw (unaccelerated) mouse input" (macOS Sonoma+), used unless system-scale hint set; `SDL_HINT_MAC_USE_GCMOUSE` (default 1, "since SDL 3.6.0") | float (Y negated) | **`SDL_GetTicksNS()` at callback** (receipt) | GCMouse-derived id | `main` `src/video/cocoa/SDL_cocoamouse.m#L299-L392` (commits ad91384704 2026-03-22 "Base GCMouse raw input implementation", 060b74a664 "Restrict GCMouse to macOS Sonoma and later", d66e2d0cbd hint); open bug #16241 (synthetic clicks dropped with GCMouse) https://github.com/libsdl-org/SDL/issues/16241 ; whether GCMouse deltas are truly unaccelerated: **UNVERIFIED** (macOS section) |
| Emscripten | `emscripten_request_pointerlock` → emscripten `requestPointerLock()` **without options** (no `unadjustedMovement`; https://github.com/emscripten-core/emscripten/blob/main/src/lib/libhtml5.js#L1563-L1566) ; `movementX/Y × (window->w / canvas CSS width)` | float CSS px (scaled to SDL window units) | **0 ⇒ `SDL_GetTicksNS()` at push**; `event.timeStamp` ignored; no coalesced events | `SDL_DEFAULT_MOUSE_ID` | `S34 src/video/emscripten/SDL_emscriptenevents.c#L711-L733,#L960-L990`; `SDL_emscriptenmouse.c#L188` |
| iOS | GCMouse (`SDL_uikitevents.m`) | | | | not examined in detail |

#### Hints / transforms (`S34 include/SDL3/SDL_hints.h`)
- `SDL_HINT_MOUSE_RELATIVE_MODE_WARP` **does not exist in SDL3**: "relative mode is always implemented at the hardware level or reported as unavailable" (`S34 docs/README-migration.md#L833`); `SDL_SetRelativeMouseMode` returns `SDL_Unsupported()` if backend can't (`S34 src/events/SDL_mouse.c#L1358-L1380`). Warp **emulation** exists the other way round: `SDL_HINT_MOUSE_EMULATE_WARP_WITH_RELATIVE` (default on) turns center-warp loops into relative mode.
- `SDL_HINT_MOUSE_RELATIVE_SYSTEM_SCALE` (L2934): default "0" = "Relative mouse motion will be unscaled"; "1" = system acceleration curve (Windows: `WIN_ApplySystemScale` emulation, `SDL_windowsmouse.c#L840`; X11: transformed valuators; Wayland: accelerated dx; macOS: no effect in 3.4).
- `SDL_HINT_MOUSE_RELATIVE_SPEED_SCALE` (L2915): float multiplier after system scale. `SDL_HINT_MOUSE_NORMAL_SPEED_SCALE` for non-relative.
- `SDL_HINT_MOUSE_RELATIVE_MODE_CENTER` (L2905, default 1), `SDL_HINT_MOUSE_RELATIVE_WARP_MOTION` (L2953, default 0), `SDL_HINT_MOUSE_RELATIVE_CURSOR_VISIBLE` (L2972, default 0; "for systems without raw hardware inputs, relative mode is implemented using warping").
- Undocumented `SDL_MOUSE_INTEGER_MODE` hint (bit 0 ⇒ fractional residual accumulation, integer xrel) (`SDL_mouse.c#L218-L226,#L749-L753`) — sdl2-compat use.
- **`SDL_SetRelativeMouseTransform(cb, userdata)` (since 3.4.0)** overrides both scale hints; callback `(userdata, Uint64 timestamp, SDL_Window*, SDL_MouseID, float *x, float *y)` "may be [called on] a thread separate from the main event loop that is run at realtime priority" (`S34 include/SDL3/SDL_mouse.h#L175-L211,#L432-L448`; applied in `SDL_mouse.c#L729-L745`). ⇒ Usable as a **low-latency tap** that sees the pre-scale deltas on the Windows raw thread (x,y = lLastX/lLastY as float) — but the timestamp it receives is still SDL's synthesized/derived one (above).
- SDL drops motion events with xrel==yrel==0 (`SDL_mouse.c#L773`).

#### Verdict
SDL3 relative mode gives the best *off-the-shelf* Windows path (dedicated TIME_CRITICAL thread, GetRawInputBuffer, per-device ids, RDP handling) but its **timestamps are not device or OS input times on Windows (interpolated) and are in the wrong unit/domain for X11 relative motion (bug)**; macOS 3.4 is OS-accelerated NSEvent deltas; Emscripten never requests unadjusted lock and ignores `event.timeStamp`. Provenance per platform must be recorded; SDL cannot be treated as a uniform "raw" source.

---

### 4.3 Other crates (crates.io API https://crates.io/api/v1/crates/NAME, 2026-11-13)

| Crate | Latest (date) | Repo / activity | What it reads | Raw vs processed | Timestamps | Fit for KineForge |
|---|---|---|---|---|---|---|
| `evdev` | 0.13.2 (2025-09-15); 0.96 M recent dl | https://github.com/emberian/evdev (pushed 2026-05-28) | `/dev/input/event*` `input_event` (EV_REL REL_X/Y, REL_WHEEL_HI_RES, SYN_REPORT), `grab()` (EVIOCGRAB), uinput, async `EventStream`, `AsRawFd` | **Kernel evdev = device report deltas** (needs read perms on node) | `InputEvent::timestamp() -> SystemTime` blindly converts `timeval` as UNIX-epoch (`src/lib.rs#L372-L377,#L510-L516`); `EVIOCSCLOCKID` ioctl exists only in private `mod sys` (`src/sys.rs#L76`, `lib.rs#L177`) ⇒ set clock yourself via raw fd and read raw `timeval` via `AsRef<libc::input_event>` | ✔ primary Linux backend (when perms allow) |
| `evdev-rs` | 0.6.3 (2025-09-14) | libevdev bindings | same + libevdev SYN_DROPPED resync | raw | timeval | alternative |
| `input` (libinput) | 0.10.0 (2026-04-05); crates.io repo field says Drakulix, live repo https://github.com/Smithay/input.rs (pushed 2026-11-12) | libinput context over udev/path (needs same device perms) | `PointerMotionEvent::dx_unaccelerated()` / `dx()`; `time_usec()` (`src/event/pointer.rs#L14,#L176`) | unaccel = libinput "raw device coordinates" (+rotation) | µs, libinput clock (CLOCK_MONOTONIC) | ✔ if libinput semantics wanted (also gives accel for comparison) |
| `hidapi` | 2.6.7 (2026-08-27) | https://github.com/ruabmbua/hidapi-rs | HID input reports (hidraw / IOHIDManager / Windows HID) | true report bytes where OS allows; mouse TLCs generally not readable on Windows (OS exclusive) and need TCC on macOS (see §5) | none in API (host receipt only) | vendor/config collections; mouse reports mostly ✘ |
| `rusb` | 0.9.4 (2024-04-27) | libusb wrapper | USB transfers | raw USB; claiming a mouse interface requires detaching usbhid ⇒ OS loses the mouse | none | ✘ for live capture |
| `nusb` | 0.2.9 (2026-10-24) | https://github.com/kevinmehall/nusb (active) | pure-Rust USB | same caveat as rusb (**UNVERIFIED** detail of Windows HID handling) | none | ✘ for live capture |
| `rawinput` | **does not exist** on crates.io | – | – | – | – | – |
| `multiinput` | 0.1.0 (2020-05-11); repo pushed 2025-04-17 | https://github.com/Jonesey13/multiinput-rust | Windows Raw Input incl. `GetRawInputBuffer` (`src/rawinput.rs`), `RIDEV_INPUTSINK` (`src/registrar.rs`) | raw | none exposed (**UNVERIFIED**) | ✘ stale; write own with `windows`/`windows-sys` (0.62.2 / 0.61.2) |
| `gilrs` | 0.11.2 (2026-05-30) | https://gitlab.com/gilrs-project/gilrs | gamepads only | – | – | irrelevant for mice |
| `device_query` | 4.0.1 (2025-07-21) | https://github.com/ostrosco/device_query | **polls** cursor position: `GetCursorPos` (Win, `src/device_state/windows/mod.rs#L6,#L30`), `XQueryPointer` (Linux X11, `linux/mod.rs#L72`) | **accelerated cursor pixels**, sampled at poll rate | none | ✘ (at most a "cursor" provenance sanity source) |
| `rdev` | 0.5.3 (2023-06-26); git master pushed 2026-05-12, 61 open issues | https://github.com/Narsil/rdev | global hooks: Windows `WH_MOUSE_LL` (`src/windows/common.rs#L135`), X11 XRecord (`src/linux/x11/listen.rs#L36-L47`), macOS CGEventTap, Wayland (master) via libinput | `MouseMove{x,y}` = **absolute pixels** (accelerated cursor) (`src/rdev.rs#L259-L264`) | `time: SystemTime::now()` at hook (wall clock) | ✘ for measurement |
| `sdl3` (sdl3-rs) | 0.22.1 (2026-11-09) | https://github.com/vhspace/sdl3-rs | `Event::MouseMotion{timestamp:u64, which, xrel:f32,…}` (`src/sdl3/event.rs#L755-L764`); `set_relative_mouse_mode`; no safe wrapper for `SDL_SetRelativeMouseTransform`/`SDL_GetMice` (grep) | as SDL | as SDL (see bugs) | use `sdl3-sys` 0.7.3+SDL-3.4.20 for full API |
| `winit` | 0.30.13 stable / 0.31.0-beta.4 | see §4.1 | | | none | windowing only |

---

### 4.4 Implications / recommended layering (Rust)
1. **Windowing**: winit (or SDL) only for surfaces/focus/cursor grab. Do not use winit `MouseMotion/PointerMotion` as the research stream.
2. **Windows capture**: own dedicated thread + message-only window + `RegisterRawInputDevices` + `GetRawInputBuffer` drain (SDL's design is the reference), stamp `QueryPerformanceCounter` at each drain, keep per-packet order, honour `MOUSE_MOVE_ABSOLUTE` (flag & mark provenance `absolute/RDP`), keep `hDevice`. Coordinate with winit: call `listen_device_events(DeviceEvents::Never)` **before** registering (process-wide last-registration-wins).
3. **Linux**: evdev (crate `evdev`, set `EVIOCSCLOCKID=CLOCK_MONOTONIC` yourself, read raw `timeval`) when the user has access; else X11 XI2 `XI_RawMotion` raw_values (X server ms time only; with xf86-input-libinput = libinput unaccel) or Wayland `zwp_relative_pointer_v1` `dx_unaccel` + `utime` µs (needs own binding — winit drops `utime`; feasibility of attaching a second event queue to winit's `wl_display` via raw-window-handle is **UNVERIFIED**).
4. **macOS**: winit/SDL-3.4 give NSEvent deltas (OS-processed) → provenance "OS pointer events". GCMouse (SDL main/3.6 approach) or IOHID (TCC Input Monitoring) needed for anything closer to device counts.
5. **Web**: winit 0.31 requests `unadjustedMovement` heuristically (not on Chrome Linux; fails on Firefox/Wayland per #4831); SDL/Emscripten never does. KineForge-web should call `requestPointerLock` itself, record whether the unadjusted promise resolved, and log `event.timeStamp` + coalesced events.

### 4.5 Open questions / experiments (this section)
- X11: confirm empirically (`xinput test-xi2 --root` while moving two mice) whether winit's `DeviceId` from `xev.deviceid` collapses all mice into the master id.
- Windows: reproduce winit #2884 (RDP/Hyper-V enhanced session) to quantify absolute-as-relative garbage; measure winit vs SDL raw-thread CPU and dropped/merged packets at 4 kHz/8 kHz.
- SDL X11 relative timestamp bug: run `testrelative` / log `event.motion.timestamp` vs `SDL_GetTicksNS()` to confirm X-server-ms leak (then report upstream — only after live proof).
- SDL Windows: quantify error of interpolated timestamps vs per-packet QPC-at-drain under 8 kHz.
- Wayland: test SDL/winit dx_unaccel vs evdev counts on GNOME/KDE/wlroots (fractional values? scaling by output scale?).
- macOS: GCMouse delta units vs IOHID report counts; whether pointer-speed setting affects GCMouse.
- Web: winit `is_cursor_lock_raw()` true-positive rate per browser×OS vs injected counts.

---

## 5. HID-level access and the meaning of "true device counts" (accessed 2026-11-14 unless noted)

### 5.1 Definition

**Device-reported counts** are the integer X/Y fields of a HID *input report*, as sent by the mouse firmware at its report rate. They are interpreted with the report descriptor: logical min/max, report size, and the Relative flag.

**One report = the motion the firmware accumulated since its previous report.** This is the sum of one or more sensor frames.

What the counts already include, and no host API can remove:
- **CPI scaling.** The sensor resolution setting, and any firmware rescaling such as the ODD directional-DPI features.
- **Angle snapping / angle tune, and lift-off cut-off.** These are sensor-register features. The PixArt PMW3360 datasheet lists "Angle snapping", "Angle tunability" and selectable lift detection. Registers `Angle_Tune` 0x11, `Angle_Snap` 0x42 and `Lift_Config` 0x63 are per the QMK driver. **Secondary**: the datasheet PDF mirror (https://wmsc.lcsc.com/wmsc/upload/file/pdf/v2/lcsc/2404021016_PixArt-PMW3360DM-T2QU_C20612443.pdf) did not render for verification; https://hacktivis.me/git/qmk_firmware/file/drivers/sensors/pmw3360.h.html.
- **Smoothing / filtering, "motion sync"** (aligning sensor frames to USB polling), and **report-rate binning**. These are vendor-specific and UNVERIFIED per device.
- **Wireless link batching and retransmission.** The local receiver relays 2.4 GHz packets as USB reports. How the link aggregates them is UNVERIFIED.

KineForge should record firmware-relevant state as provenance metadata: CPI, angle snapping, LOD, report rate, and the firmware version (readable on this mouse through its vendor channel).

### 5.2 Access by platform

| Platform | Can a user-mode app read the mouse TLC's input reports? | Details |
|---|---|---|
| **Linux** | **Yes, via `/dev/hidrawN`, if the node is readable.** | hidraw delivers reports "unmodified" (K `Documentation/hid/hidraw.rst` L5-8). `read()` gives "the reports sent from the device on the INTERRUPT IN endpoint" (L42-47). `hid_report_raw_event` calls `hidraw_report_event` **and** `hidinput_report_event` for the same report (K `hid-core.c` L2102-2116), so **reading hidraw does not take the mouse from the OS**. Default nodes are `root 0600` (local hidraw0/1/5/6). **HID-BPF** `hid_device_event` programs run first and "can act on the data". Later stages ("hidraw, input, LEDs") see the modified data, or nothing if the program discards it (K `Documentation/hid/hid-bpf.rst` L157-165, L261-276; dispatch at `hid-core.c` L2150). Fixups ship via `udev-hid-bpf`. **Contamination path:** check `/sys/fs/bpf` or `udev-hid-bpf list` (UNVERIFIED tool usage). No timestamp: stamp at `read()` with `clock_gettime(CLOCK_MONOTONIC)`. |
| **Windows** | **No.** | "The system opens all keyboard and mouse collections for its exclusive use" (MS Learn, §1 5-Win). hidapi falls back to an access-less handle: "System devices, such as keyboards and mice, cannot be opened in read-write mode… feature reports can still be sent and received" (hidapi `windows/hid.c` L1024-1037). Vendor-defined TLCs are readable. GameInput raw reports are GIP-only. Device counts on Windows = Raw Input relative packets (tier 2), or USB capture (USBPcap) offline. |
| **macOS** | **Yes, with Input Monitoring and a non-seize open.** | §3.2. Report callback with timestamp. hidapi seizes by default, so call `hid_darwin_set_open_exclusive(0)`. |
| **Browser** | **No.** | WebHID always protects GD Mouse/Pointer input reports. WebUSB protects class 0x03 (§6.9). Vendor TLCs are readable via WebHID after the chooser. |
| **Any (offline)** | USB bus capture | Linux `usbmon` (root), Windows USBPcap, or a hardware analyser: device counts *plus* bus-level timestamps. This is the best ground truth for validating every other layer (experiments W1, H2). |

### 5.3 How OS raw APIs relate to reports

| OS raw API | Per-report? | Value transform | Evidence |
|---|---|---|---|
| Linux evdev | Yes: one SYN_REPORT per report; all-zero reports vanish | none (logical value); HID-BPF upstream | §2.1 |
| libinput unaccel / Xorg XI2 raw | per evdev frame | rotation; Lua plugins (≥ 1.30) | §2.3–2.4 |
| Wayland dx_unaccel | per libinput event | compositor-defined (Mutter/KWin/wlroots pass libinput unaccel); Xwayland scales | §2.5 |
| Windows Raw Input | assumed one `MOUSE_INPUT_DATA` per report: **UNVERIFIED** (W1). Background listeners are coalesced on Win11. | filter drivers can rewrite (RawAccel); absolute for RDP/VM | §1 |
| macOS CGEvent field 170 / GCMouse | UNVERIFIED (coalescing applies to NSEvent) | UNVERIFIED scaling | §3 |

### 5.4 Local machine (read-only, 2026-11-14)

- **Receiver.** `258a:2031` "Glorious Series One PRO Wireless", USB **full-speed (12 Mb/s)**. Three HID interfaces:
  - `1-4:1.0`: class 03/01/**02** (boot mouse), EP 0x81, bInterval 1 ms, hidraw2, evdev event7.
  - `1-4:1.1` and `1-4:1.2`: 03/01/**01** (boot *keyboard* protocol), hidraw3 and hidraw4.
  - Source: `/sys/bus/usb/devices/1-4/1-4:1.*`.
- **What the other two interfaces carry** (report descriptors decoded by hand):
  - **hidraw3** (`.0006`, interface 1.1):

    | Report ID | Collection |
    |---|---|
    | 1 | Consumer Control |
    | 2 | System Control |
    | **3** | **Keyboard** (modifiers + key array) |
    | 4 | Vendor page 0xFFA0, 8-byte input |
    | 5 | Vendor page 0xFFFF, 8-byte input |

  - **hidraw4** (`.0007`, interface 1.2): vendor page 0xFFFF, a single **64-byte feature report**. This is presumably the configuration channel used by ODD/OpenMouse (inference).
- **Mouse report descriptor** (`/sys/bus/hid/devices/0003:258A:2031.0005/report_descriptor`, decoded by hand). No Report ID, 8-byte report:

  | Field | Usage | Size | Logical range | Relative |
  |---|---|---|---|---|
  | Buttons 1–5 | Button page | 5 bits + 3 padding | – | – |
  | Wheel | 0x38 | 8 bits | −127..127 | yes |
  | X, Y | 0x30, 0x31 | 16 bits each | −32767..32767 | yes |
  | AC Pan | 0x0C/0x0238 | 8 bits | −127..127 | yes |
  | (one byte) | 0x00 | 8 bits | 0..1 | – |

  Implications:
  - At ≤ 1 kHz, X/Y never saturate in practice.
  - No Resolution Multiplier, so the wheel is notch-only (v120 = 120 × notches).
- **Permissions.** hidraw2–4 are `0666 root:plugdev` + `uaccess` ACL via the user's `/etc/udev/rules.d/99-glorious-series-one-pro.rules` (written for the ODD/OpenMouse tooling). Consequences:
  - KineForge can read **ground-truth mouse reports on this machine without any permission change**.
  - **Security note (no action taken):** the same rule makes hidraw3 world-readable. hidraw3 contains a Keyboard TLC (report ID 3), so any key events the mouse emits (e.g. macro bindings) can be read by any local process.
    - Only hidraw2 (KineForge ground truth) and hidraw4 (config) appear necessary.
    - Narrowing the rule is the user's decision.

---

## 6. Browser (researched 2026-11-13; plus browser rows of §7)

All claims accessed **2026-11-13**. Source line numbers refer to `main` as fetched that day
(chromium/chromium HEAD ≈ `a0facb83b514`, mozilla-firefox/firefox ≈ `6beeccd5eef3`,
WebKit/WebKit ≈ `cebeae1c387e`, mdn/browser-compat-data ≈ `69fcf3608174`). Local browsers are
older (Chromium 156.0.8078.79, Firefox 159.0), so every source-derived claim gets a runtime check
in the experiments section. "UNVERIFIED" means I could not confirm it from a primary source.

Abbreviations: **BCD** = MDN browser-compat-data; **DIP** = device-independent pixel
(physical px ÷ OS scale factor); **COI** = cross-origin isolated (COOP+COEP);
**TLC** = HID top-level collection.

---

### 6.0 Bottom line: what a browser can and cannot measure

**Can measure**
1. **OS-raw relative deltas behind Pointer Lock with `unadjustedMovement: true`.** These are the
   OS's pre-acceleration values, not proven device counts. Support:
   - Chrome: Windows (`WM_INPUT` `lLastX/lLastY`), macOS (`kCGEventUnacceleratedPointerMovementX/Y`), ChromeOS (Ozone evdev).
   - Firefox ≥ 152: Windows and macOS.
   - Safari ≥ 18.4: macOS.
   - Integers in Chrome and Firefox; `double` in WebKit.
   - Provenance: `BrowserPointerLockUnadjusted(os_raw)`.
2. **Accelerated ("adjusted") pointer deltas everywhere else.** This includes **every browser on
   Linux (X11 and Wayland)**, where `unadjustedMovement` is rejected with `NotSupportedError`. This
   data is OS pointer motion: Chrome reports it in DIP integers, Firefox in CSS px integers or
   platform units (see §6.4).
3. **Sub-frame event history.**
   - `getCoalescedEvents()`: Chrome, Firefox, and Safari ≥ 18.2. WebKit on macOS only puts the event itself in the list (§6.5).
   - `pointerrawupdate`: Chrome and Firefox ≥ 148. It is secure-context only.
   - Both sum or preserve movement, so per-OS-event deltas can be rebuilt (Chrome on Windows: one per `WM_INPUT`).
4. **Event timestamps on the page's monotonic clock** (`performance.timeOrigin`-relative), coarsened:
   - Chrome: 100 µs (5 µs if COI), with jitter.
   - Firefox: 1 ms with jitter (20 µs if COI).
   - Safari: 1 ms.
   - The *source* instant differs by browser×OS (§6.6). On Chrome Windows and Chrome Wayland it is receipt time in the browser process, not OS event time.

**Cannot measure**
- **Raw HID mouse reports.** WebHID always protects input reports of the Generic-Desktop Mouse
  and Pointer TLCs (and keyboards). WebUSB protects HID class 0x03. The Gamepad API never lists
  mice. Only *vendor-defined* TLCs (for example a 0xFF00 config channel) can be read, through
  WebHID.
- **Which physical mouse moved.** All mice merge into one pointer; Pointer Events say so explicitly.
- **Device CPI/DPI or true report rate.** These can only be inferred, and badly (see §6.6 for timestamp domains).
- **Anything on Linux without OS acceleration**, unless the user configures the libinput flat
  profile. The page cannot confirm that it has.
- **Device-side processing** (angle snapping, smoothing, lift-off) and HID-report timestamps.

**Rule for KineForge:** never label browser data as `DeviceCounts`. The best case is
`BrowserPointerLock{unadjusted:true}`, which on Windows means "OS raw API delta, per RAWMOUSE
report, relayed by the browser".

---

### 6.1 Pointer Lock spec state

- **Spec status.** Pointer Lock 2.0 Editor's Draft at <https://w3c.github.io/pointerlock/>. Latest commits:
  - `0e99fcf3` (2024-06-17): "Introduce PointerLockOptions and return promise for requestPointerLock".
  - `d59584c2` (2025-09-29): include `pointermove` in movementX/Y.
  - `c2692a6b` (2026-02-25): "MouseEvent moved to Pointer Events".
  - Source: `gh api repos/w3c/pointerlock/commits`.
- **`unadjustedMovement`.** When true, "the event coordinates will not be affected by the underlying platform behaviors such
  as mouse acceleration". If the platform cannot support it, the UA must fire `pointerlockerror`
  and reject with **`NotSupportedError`**.
- **Activation.** If the global "does not have transient activation and the Document has not previously released a
  successful pointer lock with `exitPointerLock()`", reject with `NotAllowedError`. Fullscreen consumes
  activation, so **request pointer lock before fullscreen** in the same gesture.
- **IDL.** In the ED, `movementX/movementY` are **`double`**. They are defined as `eNow.screenX − ePrevious.screenX`.
  The sum of movementX across events must equal the screenX delta. They are zero except for `mousemove` and `pointermove`.
- **Exit and abuse rules.** Esc is the recommended default unlock gesture. Losing focus (tab or window change) exits.
  "Repeated escapes … can signal [UA] to not re-lock without more specific user action."
- **Open spec issues:**
  - <https://github.com/w3c/pointerlock/issues/42>: movementX coordinate space differs per browser (2019 table, stale for Chrome; see §6.4).
  - <https://github.com/w3c/pointerlock/issues/98>: units of unadjusted values undefined; no DPI exposure.
  - <https://github.com/w3c/pointerlock/issues/74>: movement in coalesced events.
  - <https://github.com/w3c/pointerevents/issues/535>: per-pointerId movement.
- **MDN.** The movementX page warns that units "may be a physical pixel, a logical pixel, or a CSS pixel"
  (mdn/content `files/en-us/web/api/mouseevent/movementx/index.md`).

### 6.2 `requestPointerLock()` returns a Promise

| Browser | Pointer Lock | Promise return | Source |
|---|---|---|---|
| Chrome/Edge | 37 | **92** | BCD `api/Element.json` `requestPointerLock` note |
| Firefox | 50 | Yes. Current IDL is `Promise<undefined> requestPointerLock(optional PointerLockOptions options = {})`. BCD gives no version note (UNVERIFIED first version). | `dom/webidl/Element.webidl` L349-358 |
| Safari | 10.1 | **18.4** | BCD note; BCD commit `45d96dad` (2025-07-24) |
| Chrome Android | 144 | 144 | BCD |
| Safari iOS | not supported per BCD. WebKit has IOS_FAMILY code paths for unadjusted movement, so the iPadOS status is UNVERIFIED. | | BCD; `WebPageIOS.mm` L4051 |

Feature detection: call `requestPointerLock({unadjustedMovement:true})` and catch `NotSupportedError`.
You cannot detect support without a request; this is an open question in the W3C thread.

### 6.3 `unadjustedMovement` support by browser and OS (source-verified)

| | Windows | macOS | Linux X11 | Linux Wayland | ChromeOS | Android |
|---|---|---|---|---|---|---|
| **Chrome ≥ 88** | ✔ WM_INPUT raw | ✔ CGEvent unaccelerated field | **✘ NotSupportedError** | **✘ NotSupportedError** | ✔ Ozone evdev | ✔ (144; mechanism UNVERIFIED) |
| **Firefox ≥ 152** | ✔ WM_INPUT raw | ✔ CGEvent unaccelerated field | **✘** | **✘** | n/a | ✘ (throws NotSupportedError, per bug comment) |
| **Safari ≥ 18.4** | n/a | ✔ (double) | WebKitGTK/WPE **✘** | WebKitGTK/WPE **✘** | n/a | iPadOS: UNVERIFIED |

#### Chrome (Chromium main)

**Gate.** `RenderWidgetHostViewEventHandler::LockPointer()` calls
`window_->GetHost()->RequestUnadjustedMovement()`. A null result returns `kUnsupportedOptions`
(`content/browser/renderer_host/render_widget_host_view_event_handler.cc` L147-161, L191-219).
Blink maps `kUnsupportedOptions` to **`NotSupportedError`**: "The options asked for in this request are not supported on this platform"
(`third_party/blink/renderer/core/page/pointer_lock_controller.cc` L248-251).

**Linux.** The base `aura::WindowTreeHost::RequestUnadjustedMovement()` is `NOTIMPLEMENTED(); return nullptr;`
(`ui/aura/window_tree_host.cc` L433-436). GitHub code search for `RequestUnadjustedMovement` in the Chromium mirror finds
overrides only in:
- `ui/views/widget/desktop_aura/desktop_window_tree_host_win.cc` L961 (Windows)
- `ash/host/ash_window_tree_host_platform.cc` L179 (ChromeOS, `ScopedEnableUnadjustedMouseEventsOzone`)

There is no override for Linux X11 or Wayland, so **Linux desktop rejects**. This matches the BCD note "Not yet supported on Linux".

**Windows registration.** `ScopedEnableUnadjustedMouseEventsWin::StartMonitor` calls `RegisterRawInputDevices`
with usage page 1, usage 2, **`RIDEV_INPUTSINK`**, hwnd target. It does not use `RIDEV_NOLEGACY`. On unlock it uses `RIDEV_REMOVE`
(`ui/views/win/scoped_enable_unadjusted_mouse_events_win.cc` L35-50).

**Windows `WM_INPUT` handling** (`ui/views/win/hwnd_message_handler.cc` L2544-2594):
- Each `WM_INPUT` is read with `GetRawInputData` (no `GetRawInputBuffer`).
- It builds one `ui::MouseEvent` with `ui::EventTimeForNow()`.
- If **not** `MOUSE_MOVE_ABSOLUTE`, it calls `set_movement(lLastX, lLastY)`.
- `WM_MOUSEMOVE`-derived moves are suppressed while `WM_INPUT` is in use (L3637-3642).

**Raw flag.** `set_movement` sets `EF_UNADJUSTED_MOUSE` (`ui/events/event.h` L525-528). The header comment reads: "Raw mouse movement value reported from mouse hardware. The value of this is platform dependent" (L611-615).
`MakeWebMouseEvent` copies it and sets `is_raw_movement_event=true` (`ui/events/blink/web_input_event.cc` L249-253).

**Absolute devices (RDP, VMs, tablets).** The flag is not set, so Blink falls back to a
positional delta from `GetCursorPos`. In unadjusted mode Windows does not recenter the cursor
(`ShouldMoveToCenter` returns false, `…event_handler.cc` L976-985), so movement likely stalls at the clip edge. Runtime behaviour is UNVERIFIED.

**macOS** (`components/input/web_input_event_builders_mac.mm` L152-162):
- `movement_x = CGEventGetIntegerValueField(cg, kCGEventUnacceleratedPointerMovementX)` and `is_raw_movement_event = true`.
- Otherwise movement is `event.deltaX` (accelerated).
- `RenderWidgetHostViewMac::LockPointer` always returns success (`render_widget_host_view_mac.mm` L1571-1586). BCD records "macOS Catalina 10.15.1+".

**Changing options while locked.** Supported: Blink `RequestMouseLockChange` (`pointer_lock_controller.cc` ~L150-170) and
`ChangePointerLock` (`…event_handler.cc` L191-219).

#### Firefox (152+, shipped by Bugzilla 2037802, pref `dom.pointer-lock.unadjusted-movement.enabled`)

- **Default and gate.**
  - `nsIWidget::SupportsUnadjustedMovement()` defaults to `false` (`widget/nsIWidget.h` L1928-1949).
  - Windows overrides it to `true` (`widget/windows/nsWindow.h` L441).
  - Cocoa returns `dom.pointer-lock.native-lock.enabled`, default `true` (`widget/cocoa/nsCocoaWindow.mm` L7955; `StaticPrefList.yaml` L4337-4341).
  - GTK has **no override**, so X11 and Wayland reject.
  - `PointerLockManager` maps that to "PointerLockDeniedFailedToLock", then **`NotSupportedError`** (`dom/base/PointerLockManager.cpp` L594-601, L93-96).
- **Windows** (`widget/windows/nsWindow.cpp` L8833-8853):
  - `SetNativePointerLockMode` registers 0x01/0x02 with **`RIDEV_INPUTSINK`**.
  - The `WM_INPUT` handler (L5198-5246) uses `lLastX/lLastY` and **drops `MOUSE_MOVE_ABSOLUTE` entirely**, with the comment "would cause analog-stick like motion for tablet devices". `WM_MOUSEMOVE` is ignored in raw mode (L5250-5253), so under RDP or VM absolute pointers you get **no movement**. Runtime check needed.
- **macOS** (`nsCocoaWindow.mm` L3060-3086): uses `int32_t(CGEventGetIntegerValueField(cg, kCGEventUnacceleratedPointerMovementX/Y))`. Otherwise it falls back to `int32_t(aMouseEvent.deltaX)`, which **truncates** fractional accelerated deltas.
- **Android**: per bug 2037802 comment 6 (2026-05-18), it "currently throws `NotSupportedError`".

#### Safari / WebKit

- `PointerLockController::requestPointerLock` rejects `NotSupportedError` "Unadjusted movement is unavailable." when
  `!supportsUnadjustedMovement()` (`Source/WebCore/page/PointerLockController.cpp` L106-110).
- `supportsUnadjustedMovement()` is
  `HAVE(MOUSE_UNACCELERATED_MOVEMENT) || PLATFORM(IOS_FAMILY)` combined with `hasAccessoryMousePointingDevice()` (L329-336).
- `HAVE_MOUSE_UNACCELERATED_MOVEMENT` is defined only for `PLATFORM(MAC)` (`Source/WTF/wtf/PlatformHave.h` L1515-1517).
- Mac `WebPage::hasAccessoryMousePointingDevice()` returns `true` (`WebPage.cpp` L12093-12096).
- Values come from `CGEventGetDoubleValueField(…, kCGEventUnacceleratedPointerMovementX/Y)` (`Source/WebCore/platform/mac/PlatformEventFactoryMac.mm` L712-718, L749).
- WebKitGTK/WPE (Epiphany on Linux) therefore reject.

### 6.4 `movementX/Y`: units, quantisation, provenance

| Browser / OS / mode | Native source | Units exposed | Type / quantisation | Page zoom / DPR effect |
|---|---|---|---|---|
| Chrome Win, adjusted | `WM_MOUSEMOVE` cursor positions (Enhanced Pointer Precision applied if on) | **DIP** screen delta | `long`. `trunc(screenNow) − trunc(screenPrev)` | Not zoom-scaled; DSF removed in Feb 2022 |
| Chrome Win, **unadjusted** | `RAWMOUSE.lLastX/Y` per WM_INPUT | **OS raw units (mickeys ≈ counts for relative HID mice)** | `long` integer, verbatim | Bypasses zoom/DSF (`mouse_event.cc` L226-235) |
| Chrome macOS, adjusted | `NSEvent.deltaX` (accelerated points) | DIP/points | `long` | |
| Chrome macOS, **unadjusted** | `kCGEventUnacceleratedPointerMovementX` integer field | macOS unaccelerated units (relation to HID counts UNVERIFIED) | `long` | |
| Chrome Linux X11 | XI2 core-pointer Motion (libinput profile applied by the X server). Chrome uses no `XI_RawMotion`; the only "RawMotion" match under `ui/` is the generated proto | DIP | `long`, truncated positions | |
| Chrome Linux Wayland | `zwp_relative_pointer_v1` **accelerated `dx`**. `dx_unaccel` is decoded then **discarded** (`ui/ozone/platform/wayland/host/wayland_zwp_relative_pointer_manager.cc` L94-101). Accumulated into a float virtual location (`wayland_event_source.cc` L992-999) | DIP | `long` | |
| Firefox, any OS, when the widget supplies `mMovement` | Value returned **verbatim** (`dom/events/MouseEvent.cpp` L449-454): Windows-unadjusted `lLastX`; macOS native lock `int32(unaccel)` or `int32(deltaX)`; Wayland `int32(dx_accel × fractionalScale)` (`widget/gtk/nsWindowWayland.cpp` L197-220) | Platform units, **not CSS px** | `long`. Truncation per event, **no remainder carry** (macOS adjusted, Wayland) | Not zoom-scaled |
| Firefox X11, Windows-adjusted | `DevPixelsToCSSPixels(refPoint) − DevPixelsToCSSPixels(lastRefPoint)` (L456-459). X11 lock uses XFixes pointer barriers plus recentering (`nsWindowX11.cpp` L299-313); Windows uses `ClipCursor` (5 px inset) | **CSS px** (DPR and page zoom applied) | `long`; each position rounded before differencing | Scaled by DPR **and** zoom |
| Safari macOS, adjusted | `NSEvent.deltaX` | points | **`double`** (`MouseEvent.idl` L43-44) | |
| Safari macOS, **unadjusted** | `CGEventGetDoubleValueField(kCGEventUnacceleratedPointerMovementX)` | macOS unaccelerated units | `double` | |

**Chrome changed units in Feb 2022.** Commit `66ad18c093` (Feb 2022), "ZoomForDSF: Don't adjustment movementX/Y by the device
scale factor" (Bug 907309), switched adjusted movement from **physical px to DIP**. The 2019 table in pointerlock#42
("Chrome = physical pixel") is stale for current Chrome.
- Current code: `pointer_event_factory.cc` L88-105 (`device_scale_factor = 1`).
- Commit position r972962, roughly M101. The milestone is UNVERIFIED.

**Chrome recentering.** Chrome (Aura: Windows-adjusted and Linux) recenters the hidden cursor **only when within 15 % of the
view border** (`kMouseLockBorderPercentage = 15`, `…event_handler.cc` L58-63). Adjusted deltas are therefore cumulative screen-position differences.

**Spec vs implementations.** The spec IDL says `double`, but Chrome and Firefox expose `long`:
- Blink `mouse_event.idl` L65-66; `WebMouseEvent::movement_x` is `int` (`web_mouse_event.h` L105-110).
- Firefox `MouseEvent.webidl`.
- WebKit switched coordinates to fractional with commit `8dc5033bb0` (2025-08-08), "Enable fractional coordinates for Pointer Events…". The Safari release that shipped it is UNVERIFIED.

**Implication.** Under unadjusted lock, Chrome and Firefox expose **integer OS raw units independent of DPR and zoom**.
Under adjusted lock, Chrome gives DIP and Firefox gives CSS px. KineForge must record browser, OS,
DPR, page zoom and mode with every sample, and never mix them.

### 6.5 Delivery: rAF alignment, coalescing, `pointerrawupdate`, `getCoalescedEvents`

**Chrome**
- **rAF alignment.** `mousemove`, `mousewheel` and `touchmove` are **rAF-aligned**: queued and coalesced until the next main frame
  (`third_party/blink/renderer/platform/widget/input/main_thread_event_queue.cc` L890-909, `IsRafAlignedEvent`).
  The only exceptions are low-latency or unbuffered modes.
- **Movement is summed** when coalescing: `WebMouseEvent::Coalesce` does `movement_x += x` (`third_party/blink/common/input/web_mouse_event.cc` L56-65).
- **`pointerrawupdate`.** When a page has `pointerrawupdate` listeners, each incoming mouse move also queues one
  (`main_thread_event_queue.cc` L559-570). These are dispatched *ahead* of rAF-aligned events, not at frame cadence (L688-691; `ShouldFlushQueue` L883-888). They can still coalesce if the main thread is busy (spec allows).
- **Secure context.** `pointerrawupdate` is secure-context only since **M142** (BCD note; runtime flag `PointerRawUpdateOnlyInSecureContext` "stable", `runtime_enabled_features.json5` ~L5199).
- **`getCoalescedEvents()`** is `[SecureContext]` (`pointer_event.idl`).

**Firefox**
- **Coalescing.** `dom.events.coalesce.mousemove = true` (`StaticPrefList.yaml` L3227). `CoalescedMouseData::Coalesce`
  **accumulates `mMovement`** ("Without this, the dispatched event would report only the first motion"). It appends each raw widget event for `getCoalescedEvents()` (`dom/ipc/CoalescedMouseData.cpp` L46-75).
- **Exposure.** `getCoalescedEvents` is exposed for secure contexts or WebExtensions (`PointerEvent.webidl`).
- **`pointerrawupdate`** ships fully in **148**. Versions 140-147 had `movementX/Y` always 0, bug 1987671 (BCD). Pref `dom.event.pointer.rawupdate.movement.enabled = true` (L3130). `[SecureContext]` in `EventHandler.webidl`.

**Safari**
- `getCoalescedEvents` from 18.2. `pointerrawupdate` is **not supported** (BCD).
- On macOS, `PlatformMouseEventBuilder` sets `m_coalescedEvents` to a single copy of the event itself (`PlatformEventFactoryMac.mm` L751-752). The list therefore probably gives **no sub-frame history**. UNVERIFIED end to end.

**Spec.** Pointer Events ED (<https://w3c.github.io/pointerevents/>) says:
- UA "MUST fire … pointerrawupdate … only … within a secure context".
- It SHOULD dispatch "as soon as possible and as frequently as the JavaScript can handle".
- The concatenated coalesced lists of `pointerrawupdate` events equal the next `pointermove`'s list.
- Coalesced lists are chronologically sorted by `timeStamp`.
- Multiple mice "are generally treated as a single device".

### 6.6 Timestamps (`event.timeStamp`)

**Spec.**
- DOM: `timeStamp` is the "relative high resolution coarse time" of the occurrence (<https://dom.spec.whatwg.org/>).
- HR-Time "coarsen time": resolution 100 µs "or a higher implementation-defined value", 5 µs if cross-origin isolated, "coarsen and potentially jitter" (<https://w3c.github.io/hr-time/>).
- The origin is `performance.timeOrigin` on the monotonic clock, so it is comparable with `performance.now()` and rAF timestamps in the same document.

| Browser | Coarsening | Which instant the platform timestamp represents |
|---|---|---|
| Chrome | `TimeClamper` **100 µs**, **5 µs if COI**, with a pseudo-random per-interval threshold (jitter) (`time_clamper.h` L23-24; `performance.cc` L1330-1352; `Event::timeStamp` uses `platform_time_stamp_`, `event.cc` L377-395) | **Windows: `TimeTicks::Now()` (QPC) when the browser UI thread processes the message, including WM_INPUT.** `GetMessageTime` is deliberately ignored (`ui/events/win/events_win_utils.cc` L257-270; `hwnd_message_handler.cc` L2584). **X11: X server event time, ms** (`events_x_utils.cc` L334-357). **Wayland: receipt time**; `EventMillisecondsToTimeTicks` returns `EventTimeForNow()`, TODO crbug.com/40287874 (`wayland_util.cc` L323-328); the relative-pointer µs `utime` is unused. **macOS: `NSEvent.timestamp`** (`web_input_event_builders_mac.mm`). ChromeOS: UNVERIFIED. |
| Firefox | `privacy.reduceTimerPrecision=true` with `…reduceTimerPrecision.microseconds = 1000`: **1 ms with jitter** (random midpoint). **COI: 20 µs, no jitter.** RFP: ≥ 16.667 ms (`nsRFPService.cpp` L121, L553-560, L728-802, L893-912; `StaticPrefList.yaml` L18672, L18769; `Event::TimeStamp` `dom/events/Event.cpp` L838-864) | Windows: **`GetMessageTime()`** (ms, tick-count domain) via `TimeConverter` (`nsWindow.cpp` L4085-4086, L6285-6288). GTK X11: X server ms via `TimeConverter`. GTK Wayland: GDK ms (`widget/gtk/nsWindow.cpp` L3282-3315). **Wayland relative pointer (locked): passes `utime_lo` (low 32 bits of a µs clock) into the ms converter** (`nsWindowWayland.cpp` L217), apparently a unit bug. macOS: `NSEvent.timestamp` (`nsCocoaUtils.mm` L710). |
| Safari | **1 ms** (`static Seconds timePrecision { 1_ms }`, `Source/WebCore/page/Performance.cpp` L73). No COI path found. | Event creation time, taken from the platform event (`Event::timeStampForBindings` L186-199; `MouseRelatedEvent` ctor). On macOS this is the `NSEvent.timestamp` monotonic value (`WebEventFactory.mm`). |

**Implications**
- Only Chrome on macOS, and Chrome or Firefox on X11 (ms), carry an OS-side event time.
- Chrome on Windows and Wayland stamps **browser-process receipt time**, which includes UI-thread queueing.
- Firefox's 1 ms jitter (without COI) makes inter-event intervals at ≥1 kHz meaningless.
- To use timestamps at all, **serve KineForge-web COI** (COOP `same-origin` + COEP `require-corp`/`credentialless`). That gives Chrome 5 µs and Firefox 20 µs.

### 6.7 Lock lifecycle: gesture, Esc, cooldown, rate limit, focus

**User gesture**
- **Chrome:** the browser-side check rejects with `kRequiresUserGesture` (Blink `NotAllowedError`) unless the page itself did the last unlock (`last_unlocked_by_target`) (`chrome/browser/ui/exclusive_access/pointer_lock_controller.cc` L44-75).
- **Firefox:** requires user input **or document fullscreen**; otherwise "PointerLockDeniedNotInputDriven", `NotAllowedError` (`PointerLockManager.cpp` L642-643, L103-108). I found no `exitPointerLock` re-lock exemption in Firefox (UNVERIFIED).
- **WebKit:** user gesture unless the document previously released the lock via `exitPointerLock` (`PointerLockController.cpp` L89-92).

**Re-lock cooldown (Chrome only)**
- `kEffectiveUserEscapeDuration = 1250 ms` after a *user* Esc. A request inside that window gets `kUserEscapeCooldown`, which Blink turns into **`SecurityError`** "Pointer lock cannot be acquired immediately after the user has exited the lock" (`pointer_lock_controller.cc` L23-24, L62-68; Blink L276-280).
- Esc that exits fullscreen while locked also starts the cooldown (`exclusive_access_manager.cc` L254-268, L286-296).
- No equivalent was found in Firefox or WebKit code (UNVERIFIED).

**Rate limit (Chrome renderer)**
- More than **10 requests within a trailing 2 s** gives `NotAllowedError` "Too many pointer lock requests" (`pointer_lock_controller.h` L143-144; `.cc` L116-140).
- Kill switch `RateLimitPointerLockRequests`, status "stable".

**Focus and visibility**
- Spec: focus loss exits.
- Chrome: `RenderWidgetHostImpl::SetPageFocus(false)` unlocks if locked (`render_widget_host_impl.cc` L1505-1521). `WasHidden()` rejects or unlocks with `kWrongDocument` (L888-897).
- Firefox: rejects requests from hidden or unfocused documents with `WrongDocumentError` (`PointerLockManager.cpp` L195-249, L85-92). Firefox's mid-lock blur path was not traced (UNVERIFIED, but the spec mandates it).

**Events and errors**
- Every rejection also fires `pointerlockerror` on the document. Success fires `pointerlockchange`.
- Sandboxed iframes need `allow-pointer-lock`; without it the request fails with `SecurityError` in Chrome, Firefox and WebKit (cites above).

**Capture recommendation:** treat lock as fragile. Log `pointerlockchange`, `pointerlockerror`, `visibilitychange` and
`blur` into the session record, mark the gaps, and never interpolate across them.

### 6.8 Keyboard Lock (Esc for in-game use)

- `navigator.keyboard.lock(['Escape'])` exists only in **Chromium** (Chrome 68; BCD shows no Firefox or Safari). Spec: <https://wicg.github.io/keyboard-lock/>.
- Chrome applies the lock **only while the WebContents is fullscreen** (`keyboard_lock_controller.cc` L196-205).
- With Esc locked, the user must **press and hold Esc for 1500 ms** (`kHoldEscapeTime`, L28; `exclusive_access_manager.cc` L30) to exit. That exits fullscreen, the keyboard lock and pointer lock, and starts the pointer-lock cooldown.
- Without fullscreen plus keyboard lock, a single Esc always exits pointer lock.

### 6.9 WebHID, WebUSB, Gamepad

**WebHID**
- **Spec.** Now the **WHATWG HID Living Standard** (<https://hid.spec.whatwg.org/>, "Last Updated 4 November 2026"; the old WICG URL redirects). `[SecureContext]`.
- **`requestDevice()`** rejects `SecurityError` without **transient activation**, and the UA shows a chooser. Permissions-policy feature `hid`.
- **Support.** Chromium only: Chrome/Edge 89, not Android, no Firefox or Safari (BCD `api/HID.json`). Dedicated workers from Chrome 131.
- **Spec blocklist** (<https://github.com/whatwg/hid/blob/main/blocklist.txt>): FIDO and some vendor rules. The non-normative text says "access to keyboard-like devices can be denied…" and that mice are served by PointerEvent.
- **Chromium "always protected" usages** (`services/device/public/cpp/hid/hid_report_utils.cc` L36-67), enforced in `HidConnection::IsReportProtected` (`services/device/hid/hid_connection.cc` L166-228):
  - Keyboard page 0x07: all report types.
  - Generic Desktop **Pointer 0x01, Mouse 0x02**, Keyboard 0x06, Keypad 0x07: **input and output reports protected, feature reports allowed**.
  - System Control 0x80 through Warm Restart, and System Dock through Display Swap: all report types.
- **Static blocklist** (`services/device/public/cpp/hid/hid_blocklist.cc` L38-114): FIDO page; specific security-key vendor/product IDs; a Google Titan rule.
- **Consequence.** The mouse TLC's input reports, which carry X/Y motion, are **never delivered to a page**. A vendor-defined TLC (usage page ≥ 0xFF00, for example the ODD config channel) is readable and writable after user consent, provided the OS exposes it.
- **Linux.** Chromium WebHID goes through `/dev/hidraw*`, so node permissions apply. Locally, the Glorious receiver 258a:2031 is on hidraw2-4 with mode `0666`, group `plugdev`, `uaccess`, via `/etc/udev/rules.d/99-glorious-series-one-pro.rules`. Other hidraw nodes are root-only. Read-only `ls -l` and sysfs check.
- **Windows.** The OS opens mouse and keyboard TLCs exclusively regardless (native-section topic).

**WebUSB**
- HID class **0x03** is a *protected interface class*, along with Audio, Mass Storage, Hub, Smart Card, Video, A/V and Wireless Controller (<https://wicg.github.io/webusb/>, now redirecting to usb.spec.whatwg.org; "Draft Community Group Report, 5 June 2026").
- The mouse interface cannot be claimed.

**Gamepad API**
- Irrelevant for mice. Chromium on Linux only enumerates udev devices with `ID_INPUT_JOYSTICK` (`device/gamepad/udev_gamepad_linux.cc` L99-100).

### 6.10 Local machine (read-only observations, 2026-11-13)

- Linux Mint, kernel 7.0.0-42, **X11** session (`XDG_SESSION_TYPE=x11`), Chromium 156.0.8078.79, Firefox 159.0.
- `xinput list-props 9` (Glorious Series One PRO Wireless):
  - `libinput Accel Profile Enabled: 1, 0` means **adaptive** (not flat).
  - `Accel Speed 0.0`.
  - Device node `/dev/input/event7`.
- Consequence: on this machine, every browser's movementX is libinput-adaptive-accelerated. `unadjustedMovement` is rejected in both installed browsers (expected; verify with E1).
- `libinput` CLI is not installed.

---

### 6.11 Browser rows for the §7 taxonomy

| Layer | Units | Quantisation | Aggregation | Timestamp domain / resolution | Latency contributors |
|---|---|---|---|---|---|
| **B-unlocked**: `pointermove` `clientX/screenX` | CSS px (client) / DIP (screen; Chrome and Firefox `screenX` are DIP per pointerlock#42) | Chrome PointerEvent coordinates double; MouseEvent long | rAF-aligned: one dispatched event per frame, originals in `getCoalescedEvents` | Page monotonic (`timeOrigin`), coarsened per §6.6; source instant per browser×OS | OS cursor path (accel), browser IPC (browser to renderer), main-thread queue to next rAF |
| **B-lock-adjusted**: `movementX` | Chrome: DIP. Firefox: CSS px (X11, Windows) or platform units (macOS `deltaX`, Wayland `dx×scale`). Safari: points | Chrome/Firefox int, with truncation or rounding of positions or deltas (sub-pixel loss in Firefox macOS and Wayland); Safari double | Summed across coalesced events (Chrome, Firefox) | Same as above | OS acceleration plus everything above. Linux: libinput profile in X server or compositor |
| **B-lock-unadjusted**: `movementX` | OS raw units: Windows `lLastX` (≈ device counts per report for relative HID mice); macOS unaccelerated CGEvent delta | Chrome/Firefox int, Safari double; not DPR or zoom scaled | Windows: 1 DOM (coalesced) event per `WM_INPUT`/RAWMOUSE; dispatched event = sum | Chrome Windows: receipt `Now()` (QPC→TimeTicks) on the browser UI thread, then 100/5 µs. Chrome macOS: `NSEvent.timestamp`. Firefox Windows: `GetMessageTime` ms + 1 ms jitter / 20 µs COI. Safari: 1 ms | OS raw-input delivery to the browser UI thread (`GetRawInputData` per message), IPC, renderer queue. No acceleration |
| **B-rawupdate**: `pointerrawupdate` + `getCoalescedEvents()` | Same as the lock mode in effect | Same | Dispatched ASAP, not rAF-aligned (Chrome); may self-coalesce under load | Same | Removes rAF wait; still IPC plus main-thread contention. Secure context only |
| **B-HID (WebHID)**: vendor TLC only | Device-defined bytes | n/a | Per HID report | `HIDInputReportEvent.timeStamp` (page clock, browser receipt; UNVERIFIED) | hidraw/HID API to browser to renderer. **Mouse TLC input: not available** |

Provenance enum values a web capture backend may emit:
- `BrowserPointerUnlocked`
- `BrowserPointerLockAdjusted{units: Dip|CssPx|PlatformPoints}`
- `BrowserPointerLockUnadjusted{os: Windows|MacOS|ChromeOS}`
- `Derived`

**Never** `DeviceCounts`.

---

### Anomalies found in browser source (each needs a runtime check)

1. **Chrome on Wayland:** decodes `dx_unaccel/dy_unaccel` and throws them away, uses accelerated `dx`, and stamps receipt time.
   Unadjusted support on Wayland looks feasible but is not implemented.
2. **Chrome on X11:** no XI2 raw path. Adjusted only, ms X-server time.
3. **Firefox on Wayland:** `int32_t(dx × scale)` per event with no remainder accumulation, so slow motion is lost (bias toward 0).
   It also treats the relative-pointer `utime_lo` (µs) as ms in `GetWidgetEventTime`. Timestamps are probably wrong.
4. **Firefox on macOS, adjusted native lock:** `int32_t(NSEvent.deltaX)` truncation, the same sub-point loss.
5. **Firefox on Windows, unadjusted:** ignores `MOUSE_MOVE_ABSOLUTE` raw input and suppresses `WM_MOUSEMOVE`, so zero motion under RDP, VMs or tablets.
   Chrome instead falls back to a positional delta, probably clamped at the clip edge.
6. **Chrome on Windows:** one `GetRawInputData` per `WM_INPUT` on the UI thread, no `GetRawInputBuffer`. CPU cost and drops at 4-8 kHz are UNVERIFIED (no primary source found).
7. **WebKit on macOS:** the coalesced list holds only the event itself.
8. **Spec vs implementations:** spec says `double`, Chrome and Firefox `long`, WebKit `double`.

### Experiments (browser)

| # | Question | Method | Expected (from source) |
|---|---|---|---|
| E1 | Linux `unadjustedMovement` | Chromium 156 and Firefox 159 on this X11 box, plus a Wayland session (GNOME/KDE). `el.requestPointerLock({unadjustedMovement:true})` in a click handler | Both reject `NotSupportedError`, `pointerlockerror` fires |
| E2 | Ground truth vs browser on Linux | Read the Glorious mouse-interface hidraw node (local nodes are 0666; read-only, non-grabbing; hidraw2-4, pick the one whose descriptor has the 01:02 TLC) at the same time as browser `pointerrawupdate` and coalesced events. Do slow and fast strokes with the same physical travel | With libinput adaptive, browser Σ movementX / HID Σ counts rises with speed (acceleration contamination). Flat profile speed 0 should give ≈ 1 × (Chrome DIP at scale 1) |
| E3 | DPR 1.25 | X11 Cinnamon fractional scaling, or Chrome `--force-device-scale-factor=1.25` / Firefox `layout.css.devPixelsPerPx=1.25` in a throwaway profile. Measure Σ movementX for a fixed hidraw count total | Chrome adjusted ≈ counts/1.25 (DIP, truncated positions). Firefox adjusted ≈ counts/1.25, rounded per event (CSS px) |
| E4 | Page zoom 200 % | Same as E3 | Chrome unchanged; Firefox (CSS px path) halved; unadjusted unchanged in both (Windows/macOS) |
| E5 | Timestamp resolution and domain | Collect `e.timeStamp` and `getCoalescedEvents()` stamps with and without COI (local server sending COOP/COEP). Plot diff histograms against `performance.now()` at handler entry | Chrome 100 µs grid (5 µs COI); X11 source ms-quantised. Firefox 1 ms + jitter, 20 µs COI. Safari 1 ms |
| E6 | Report-rate visibility (Windows) | 1 / 4 / 8 kHz mouse. Count coalesced events per second (Chrome, Firefox unadjusted) against a native Raw Input logger running at the same time | Chrome/Firefox ≈ one coalesced entry per `WM_INPUT`; check Σ conservation and drops at 8 kHz |
| E7 | Re-lock cooldown | User Esc, then relock on click at 0.5 / 1.0 / 1.5 s; also 11 programmatic relocks in 2 s | Chrome `SecurityError` < 1.25 s and `NotAllowedError` on the 11th; Firefox and Safari no cooldown (UNVERIFIED) |
| E8 | Windows absolute pointers | RDP session or VM without relative-mouse integration, unadjusted lock | Firefox: no movement. Chrome: positional deltas that stall at the clip edge |
| E9 | Firefox Wayland bugs | Slow sub-pixel strokes; compare `e.timeStamp` with `performance.now()` | Σ movement lower than the counts imply; timestamps implausible if the µs-as-ms bug is real |
| E10 | macOS semantics | Compare Chrome/Firefox (int) with Safari (double) unadjusted values against IOHID reports (Input Monitoring) | Equal to HID counts or scaled? UNVERIFIED; Safari fractions present? |
| E11 | rAF vs rawupdate latency | Chrome: `pointerrawupdate` handler timestamps against `pointermove`, both relative to rAF time | `pointerrawupdate` arrives before the frame; `pointermove` is batched at rAF |

### Open questions

- Exact units of `kCGEventUnacceleratedPointerMovementX` relative to HID counts (resolution scaling in IOHIDPointing?). Native macOS section.
- Chrome Android 144 unadjusted mechanism (presumably `View.requestPointerCapture` relative axes). Not traced.
- Firefox mid-lock blur handling, and whether Firefox or Safari implement any re-lock throttling.
- Whether Chrome's `pointerrawupdate` and coalesced lists stay complete at 8 kHz (IPC and main-thread pressure).
- Which Safari release first shipped fractional `movementX` (WebKit `8dc5033bb0`, Aug 2025).
- No browser exposes device identity, CPI, or report timestamps (pointerlock#98 is open), so per-device calibration must happen natively or through WebHID vendor channels.

### Sources (all accessed 2026-11-13)

**Specs and docs**
- Specs:
  - Pointer Lock 2.0 ED: <https://w3c.github.io/pointerlock/>; commits via `gh api repos/w3c/pointerlock/commits`.
  - Pointer Events ED: <https://w3c.github.io/pointerevents/>
  - HR-Time: <https://w3c.github.io/hr-time/>
  - DOM: <https://dom.spec.whatwg.org/>
  - WebHID: <https://hid.spec.whatwg.org/>; blocklist <https://github.com/whatwg/hid/blob/main/blocklist.txt>
  - WebUSB: <https://wicg.github.io/webusb/index.html>
  - Keyboard Lock: <https://wicg.github.io/keyboard-lock/> (via BCD spec_url)
- W3C issues: <https://github.com/w3c/pointerlock/issues/42>, <https://github.com/w3c/pointerlock/issues/98>, <https://github.com/w3c/pointerlock/issues/74>, <https://github.com/w3c/pointerevents/issues/535>
- BCD: <https://github.com/mdn/browser-compat-data> (`api/Element.json`, `api/PointerEvent.json`, `api/MouseEvent.json`, `api/Document.json`, `api/Keyboard.json`, `api/HID.json`, `api/Performance.json`, `api/Event.json`); commits `ec7562df` (Firefox 152 unadjustedMovement), `45d96dad` (Safari 18.4).
- MDN: <https://github.com/mdn/content/blob/main/files/en-us/web/api/mouseevent/movementx/index.md>, <https://developer.mozilla.org/docs/Web/API/Element/requestPointerLock>
- Bugzilla: <https://bugzilla.mozilla.org/show_bug.cgi?id=2037802> (ship unadjustedMovement, Fx152), <https://bugzil.la/1987671> (rawupdate movement 0).
- Chromium blink-dev: "Intent to Ship: Interoperable pointerrawupdate events exposed only in secure contexts" <https://groups.google.com/a/chromium.org/g/blink-dev/c/KLXAD2tozQY>. Only the search-result summary was seen; the version comes from BCD.

**Chromium** (<https://github.com/chromium/chromium/blob/main/…>):
- `ui/views/win/hwnd_message_handler.cc`
- `ui/views/win/scoped_enable_unadjusted_mouse_events_win.cc`
- `ui/views/widget/desktop_aura/desktop_window_tree_host_win.cc`
- `ui/aura/window_tree_host.cc`
- `ash/host/ash_window_tree_host_platform.cc`
- `content/browser/renderer_host/render_widget_host_view_event_handler.cc`
- `content/browser/renderer_host/render_widget_host_view_mac.mm`
- `content/browser/renderer_host/render_widget_host_impl.cc`
- `components/input/web_input_event_builders_mac.mm`
- `ui/events/event.h`
- `ui/events/blink/web_input_event.cc`
- `ui/events/win/events_win_utils.cc`
- `ui/events/x/events_x_utils.cc`
- `ui/ozone/platform/wayland/host/wayland_zwp_relative_pointer_manager.cc`
- `ui/ozone/platform/wayland/host/wayland_event_source.cc`
- `ui/ozone/platform/wayland/common/wayland_util.cc`
- `third_party/blink/renderer/core/page/pointer_lock_controller.{cc,h}`
- `third_party/blink/renderer/core/events/{mouse_event.cc,mouse_event.idl,pointer_event_factory.cc,pointer_event.idl}`
- `third_party/blink/common/input/web_mouse_event.cc`
- `third_party/blink/public/common/input/web_mouse_event.h`
- `third_party/blink/renderer/platform/widget/input/main_thread_event_queue.cc`
- `third_party/blink/renderer/core/timing/{time_clamper.h,performance.cc}`
- `third_party/blink/renderer/core/dom/events/event.cc`
- `third_party/blink/renderer/platform/runtime_enabled_features.json5`
- `chrome/browser/ui/exclusive_access/{pointer_lock_controller.cc,keyboard_lock_controller.cc,exclusive_access_manager.cc}`
- `services/device/public/cpp/hid/{hid_report_utils.cc,hid_blocklist.cc}`
- `services/device/hid/hid_connection.cc`
- `device/gamepad/udev_gamepad_linux.cc`
- Commit `66ad18c093` <https://github.com/chromium/chromium/commit/66ad18c093>

**Firefox** (<https://github.com/mozilla-firefox/firefox/blob/main/…>):
- `widget/nsIWidget.h`
- `widget/windows/nsWindow.{h,cpp}`
- `widget/cocoa/nsCocoaWindow.mm`
- `widget/cocoa/nsCocoaUtils.mm`
- `widget/gtk/{nsWindow.cpp,nsWindowWayland.cpp,nsWindowX11.cpp}`
- `dom/base/PointerLockManager.cpp`
- `dom/events/{MouseEvent.cpp,Event.cpp}`
- `dom/ipc/CoalescedMouseData.cpp`
- `dom/webidl/{Element,PointerEvent,MouseEvent,EventHandler}.webidl`
- `modules/libpref/init/StaticPrefList.yaml`
- `toolkit/components/resistfingerprinting/nsRFPService.cpp`

**WebKit** (<https://github.com/WebKit/WebKit/blob/main/…>):
- `Source/WebCore/page/PointerLockController.cpp`
- `Source/WebCore/platform/mac/PlatformEventFactoryMac.mm`
- `Source/WebKit/Shared/mac/WebEventFactory.mm`
- `Source/WebCore/dom/{MouseEvent.idl,MouseEvent.h,MouseRelatedEvent.cpp,Event.cpp}`
- `Source/WebCore/page/Performance.cpp`
- `Source/WTF/wtf/PlatformHave.h`
- `Source/WebKit/WebProcess/WebPage/WebPage.cpp`
- `Source/WebKit/WebProcess/WebPage/ios/WebPageIOS.mm`
- Commit `8dc5033bb0`

**Local (read-only):** `xinput list`, `xinput list-props 9`, `ls -l /dev/hidraw*`, `/sys/class/hidraw/*/device/uevent`, `/etc/udev/rules.d/99-glorious-series-one-pro.rules`, `chromium --version`, `firefox --version`.

---

## 7. Pipeline taxonomy (consolidated)

### 7.1 Layer definitions

Each layer is defined by **what transform separates it from the layer above**. Labels are exclusive: a sample belongs to exactly one layer, its *lowest* applicable one.

| # | Layer | Definition | Separated from the layer above by |
|---|---|---|---|
| **L1** | **Device-reported counts** | Integer X/Y of a HID input report, parsed with the device's report descriptor, one value per report. | – (firmware processing is *inside* L1, §5.1) |
| **L2** | **OS raw input events** | Per-device relative deltas exposed by an OS API **before pointer ballistics**: evdev, Raw Input, GameInput, libinput unaccel, XI2 raw, Wayland `dx_unaccel`, macOS unaccelerated field / GCMouse. | Possible: aggregation (coalescing, batching), zero-suppression, rotation or scale, filter drivers or plugins, absolute-mode substitution (RDP/VM), loss of the report boundary. **No acceleration.** |
| **L3** | **OS pointer / cursor events** | Post-ballistics pointer motion in display space: `WM_MOUSEMOVE`/`WM_POINTER`, X core/`XI_Motion`, `wl_pointer`, `NSEvent.deltaX`, `kCGMouseEventDeltaX`. | Acceleration curve (EPP / libinput adaptive / macOS curve), DPI or desktop scaling, pixel quantisation with remainder carry, edge clipping (absolute positions), coalescing to latest. |
| **L4** | **Browser pointer events** | `movementX/Y` or coordinates delivered to a page. `L4u` = locked + `unadjustedMovement` (sourced from L2). `L4a` = locked, adjusted (from L3). `L4p` = unlocked positions (from L3). | Browser IPC and renderer queueing; unit conversion (DIP / CSS px / zoom); integer truncation; rAF-aligned coalescing; coarsened page timestamps. |
| **L5** | **Application-processed movement** | KineForge's transform from an input delta to an angular command: `Δθ = counts × deg_per_count` (or cm/360 calibration), remainder policy, any smoothing (should be none), and binning into simulation ticks. | App sensitivity/gain, float rounding, tick or frame binning, input-thread → sim-thread handoff. |
| **L6** | **Camera rotation** | Integrated yaw/pitch state at a simulation tick, then used for rendering. | Pitch clamp, angle wrap, float precision, interpolation between ticks, frame pacing. |
| **L7** | **Rendered crosshair / photon** | The time and position when the view change reaches emitted light. | Render and GPU queue, present mode (vsync/VRR/tearing), compositor (Wayland/DWM/WindowServer), scanout, pixel response. Measured externally (photodiode, high-speed camera). |

### 7.2 Per-source characteristics

| Layer | Platform source | Units | Quantisation | Aggregation | Timestamp domain / resolution / meaning | Latency contributors |
|---|---|---|---|---|---|---|
| L1 | Linux hidraw | counts @ CPI | int (descriptor size, here int16) | 1 per report | none; app CLOCK_MONOTONIC at `read()` (ns API, µs-scale jitter) | sensor frame, MCU, wireless link, USB poll (bInterval), URB completion, HID-BPF, read wakeup |
| L1 | macOS IOHID report callback | counts | int | 1 per report | AbsoluteTime (mach ticks) at kernel event creation | as above + IOHIDFamily, user-client queue, run loop |
| L1 | USB capture (usbmon/USBPcap/analyzer) | counts | int | 1 per transfer | bus capture clock (µs or better) | none beyond the bus: reference |
| L2 | Linux evdev | counts (HID logical) | int32 | 1 packet per report; zero axes dropped; all-zero reports invisible | kernel `ktime_get` at SYN flush; µs; REALTIME by default (set MONOTONIC) | USB + HID parse + input core; reader wakeup; ring overflow → SYN_DROPPED |
| L2 | libinput unaccel (own context) | device units (+rotation) | double (integral for mice) | per evdev frame | µs CLOCK_MONOTONIC (kernel flush) | + libinput dispatch |
| L2 | Xorg `XI_RawMotion` raw | libinput unaccel units | FP32.32 (integral for mice) | per driver post | X server `GetTimeInMillis`, ms, MONOTONIC(_COARSE), **dispatch time** | + Xorg input thread, X protocol, client socket |
| L2* | Xwayland `XI_RawMotion` | `dx_unaccel × surface scale` | FP32.32 | per relative_motion | X server ms | + compositor + Wayland IPC + Xwayland. Focused only |
| L2 | Wayland `dx_unaccel` | compositor-defined; in practice libinput unaccel | wl_fixed 1/256 | per libinput event | `utime` µs (ms on wlroots/sway), base unspecified | + compositor frame/dispatch, Wayland IPC. Focused only |
| L2 | Windows Raw Input (relative) | counts ("mickeys") | int32 | expected 1 per report (UNVERIFIED); Win11 coalesces for background sinks; app batch drains | **none**; app QPC at dequeue (<1 µs res); `GetMessageTime` ms only via GetMessage | HID stack, filter drivers, RIT, message queue, thread scheduling, drain cadence |
| L2 | Windows GameInput | counts, cumulative int64 | int | per reading (1:1 with reports: UNVERIFIED) | µs "when the input was made" (base UNVERIFIED) | GameInput service, 0.5 s history window |
| L2? | macOS `kCGEventUnacceleratedPointerMovementX` | UNVERIFIED (≈ counts?) | int or double | per CGEvent (NSEvent coalescing upstream for AppKit consumers) | CGEventTimestamp ns-ish / NSEvent s since boot | HID event system, WindowServer, app event loop |
| L2? | macOS GCMouse | Apple: "raw" deltas | float | per callback (UNVERIFIED) | none in handler; `lastEventTimestamp` | GameController framework queue |
| L2† | Windows Raw Input `MOUSE_MOVE_ABSOLUTE` | normalised 0–65535 screen coordinates | int | per packet | as Raw Input | RDP/VM/tablet path. **Not counts.** |
| L3 | WM_MOUSEMOVE / WM_POINTER / GetCursorPos | screen px | int | coalesced to latest | `GetMessageTime` ms; `POINTER_INFO.PerformanceCount` QPC at receipt | + EPP ballistics, DPI virtualisation |
| L3 | X core / XI_Motion | screen px (XI2 FP16.16) | int / 1/65536 | per driver post | X ms | + libinput accel |
| L3 | wl_pointer.motion | surface-local logical px | wl_fixed | per compositor event | ms (`zwp_input_timestamps_v1` ns only on Weston) | + accel, output scale |
| L3 | NSEvent.deltaX / kCGMouseEventDeltaX | points | double / int | coalesced (default on) | NSEvent s since boot (mach, excludes sleep) | + macOS accel curve |
| L4u | Chrome/Firefox Win unadjusted | `lLastX` raw units | int | 1 coalesced entry per WM_INPUT; dispatched event = sum | Chrome: browser-UI-thread `Now()` at WM_INPUT; Firefox: `GetMessageTime` ms; then page coarsening | + browser UI thread, IPC, renderer main thread |
| L4u | Chrome/Firefox/Safari macOS unadjusted | CG unaccelerated units | int (Chrome, Firefox) / double (Safari) | per NSEvent | NSEvent.timestamp → page clock | as above |
| L4a/L4p | all others (incl. **all Linux**) | DIP (Chrome), CSS px or platform units (Firefox), points (Safari) | int (Chrome, Firefox), double (Safari) | rAF-aligned; `pointerrawupdate` ASAP | page clock coarsened (100/5 µs Chrome, 1 ms+jitter/20 µs Firefox, 1 ms Safari) | + OS accel + all of the above |
| L5 | KineForge transform | degrees | f64 (keep counts integer until the final multiply) | per input event, then per tick | host monotonic | input thread → sim queue |
| L6 | camera | radians/degrees | f32/f64 | per tick/frame | sim clock | tick and frame pacing |
| L7 | photon | px on panel | – | per frame / scanout | external sensor clock | GPU, present, compositor, display |

Rows for L4 come from §6.4–6.6. Windows rows come from §1.4 and §7-Win.

### 7.3 Labelling rules for KineForge

1. **Only L1 may be labelled "device counts".** L2 Linux evdev and Windows Raw Input relative packets may be labelled `os_raw_counts`, with the caveats listed in §5.3. Everything else must carry its own layer.
2. **Absolute packets are never deltas.** `MOUSE_MOVE_ABSOLUTE`, RDP, VMs and tablets are `os_absolute_pointer`.
3. **Every timestamp carries `(clock, resolution, meaning)`.** Meaning is one of: device-report-arrival-in-kernel, OS dispatch, compositor, browser receipt, app dequeue, interpolated. Interpolated or synthesized stamps (SDL Windows, batch spreading) are stored separately and flagged `derived`.
4. **Gaps are first-class events.** Examples: SYN_DROPPED, pointer-lock loss, focus or visibility loss, Win11 background mode, device removal. Never interpolate across a gap.

---

## 8. Proposed common Rust capture interface

This is a design proposal, not implemented. It is deliberately small: one trait and one event enum, with per-platform implementations behind `cfg`.

```rust
use std::sync::{atomic::AtomicBool, mpsc::SyncSender, Arc};

/// Exact API the sample came from. Never collapse variants: semantics differ (§2–§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    // L1 device counts
    LinuxHidraw, MacIoHidReport, UsbCapture,
    // L2 OS raw relative
    LinuxEvdev, LinuxLibinputUnaccel, X11RawMotion, XwaylandRawMotion,
    WaylandRelativeUnaccel(Compositor), WinRawInput { sink: bool }, WinGameInput,
    MacCgUnaccelerated, MacGcMouse,
    // L2† absolute pointer arriving through a raw API (RDP/VM/tablet)
    WinRawAbsolute,
    // L3 OS pointer (ballistics applied)
    WinPointer, X11Pointer, WaylandPointer, MacNsEvent,
    // L4 browser
    Browser { engine: Engine, os: Os, locked: bool, unadjusted: bool },
    // not measurements
    Injected, // KineForge's own uinput/SendInput/hardware-rig test stream (known truth)
    Derived,  // replay, resampling, interpolation
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit { DeviceCounts, OsRaw, Abs65535, ScreenPx, LogicalPx, Dip, CssPx, Points }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    LinuxMonotonic, LinuxRealtime, LinuxBoottime, // evdev clk / clock_gettime
    XServerMs,                                     // GetTimeInMillis (MONOTONIC_COARSE)
    WaylandUtime,                                  // base unspecified; µs or ms×1000
    WinQpc, GameInputUs, MachAbsolute,
    BrowserPerf { coi: bool },                     // performance.timeOrigin-relative
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meaning { KernelFlush, OsDispatch, Compositor, BrowserReceipt, AppDequeue, Interpolated }

#[derive(Clone, Copy, Debug)]
pub struct Stamp { pub ns: i64, pub clock: Clock, pub res_ns: u32, pub meaning: Meaning }

#[derive(Clone, Debug)]
pub struct Motion {
    pub seq: u64,             // per-backend, strictly +1; loss is reported via Gap, never silent
    pub device: u32,          // backend-scoped id → DeviceInfo (VID/PID/interface/path/OS handle)
    pub source: Source,
    pub unit: Unit,
    pub dx: f64, pub dy: f64, // f64 holds every i32 exactly; one type for all sources
    pub integral: bool,       // backend asserts integer-valued source (counts)
    pub t_src: Option<Stamp>, // the source's own stamp, unmodified
    pub t_rx: Stamp,          // our receipt on the host monotonic clock (always present)
    pub batch_ix: u16,        // index within one drain (GetRawInputBuffer / read() of N events)
    pub flags: u8,            // COALESCED | BACKGROUND | ABSOLUTE | SUSPECT_INJECTED
}

pub enum InputEvent {
    Motion(Motion),
    Button { seq: u64, device: u32, button: u8, down: bool, t_src: Option<Stamp>, t_rx: Stamp },
    Wheel  { seq: u64, device: u32, v120: i32, h120: i32, t_src: Option<Stamp>, t_rx: Stamp },
    Gap    { seq: u64, reason: GapReason, t_rx: Stamp },
    Device(DeviceInfo), // arrival/removal + static provenance (CPI, report rate, accel profile, filters)
}

pub enum GapReason { KernelDropped, LockLost, FocusLost, Hidden, BackgroundThrottled, DeviceRemoved, ClockChanged }

pub trait Capture: Send {
    /// Static description: source, clock, focus requirement, permission status (readable? why not?).
    fn info(&self) -> CaptureInfo;
    /// Blocks on the calling (dedicated) thread until `stop` is set; pushes events in arrival order.
    fn run(self: Box<Self>, tx: SyncSender<InputEvent>, stop: Arc<AtomicBool>) -> Result<(), CaptureError>;
}
```

A bounded `std::sync::mpsc::sync_channel` is enough to start. ponytail: swap it for an SPSC ring buffer only if profiling at 8 kHz shows channel cost. Multiple backends may run **at the same time**; that is how contamination checks work (§8.3).

`CaptureInfo`, `DeviceInfo`, `Compositor`, `Engine`, `Os` and `CaptureError` are omitted here; they are plain data.

### 8.1 Backends and threading

| Backend | Platform | Thread | Notes |
|---|---|---|---|
| `RawInputCapture` | Windows | **Dedicated thread** owning a message-only `HWND` (`HWND_MESSAGE`), `THREAD_PRIORITY_TIME_CRITICAL`, `MsgWaitForMultipleObjects(QS_RAWINPUT)` + `GetRawInputBuffer` drain, QPC per drain and per-packet index (SDL's design, §1.3/§4.2). | Must own the process-wide mouse registration. Set winit `DeviceEvents::Never` **before** registering (last registration wins, and winit's `Never` = `RIDEV_REMOVE`). `RIDEV_INPUTSINK` only when background capture is wanted; tag `RIM_INPUTSINK`. `RIDEV_NOLEGACY` only during locked trials (it kills `WM_MOUSEMOVE` process-wide). Flag absolute packets, `hDevice == 0`, and `MI_WP_SIGNATURE`. |
| `GameInputCapture` | Windows | Callback thread, or a poll thread at ≥ 2 Hz (0.5 s history) | Only Windows source with per-reading µs stamps. Needs COM bindings (none maintained found). |
| `EvdevCapture` | Linux | Dedicated thread, `poll()` over `/dev/input/event*` | Needs the `input` group or an ACL (absent locally). `EVIOCSCLOCKID(CLOCK_MONOTONIC)` on open. **Never `EVIOCGRAB`.** Handle SYN_DROPPED → `Gap`. Group by SYN_REPORT. The `evdev` crate's `timestamp()` assumes REALTIME, so read the raw `timeval` (§4.3). |
| `HidrawCapture` | Linux | Dedicated thread, `poll()`/`read()` | Ground truth. Parse the report descriptor from sysfs or `HIDIOCGRDESC`. Stamp CLOCK_MONOTONIC at read. Works locally now (hidraw2). |
| `X11RawCapture` | Linux X11 | **Own `Display` connection on its own thread** | Select `XI_RawMotion` on the root window for `XIAllDevices` (to get per-slave `sourceid`). Independent of winit, no permissions, global. Timestamps are X-server ms. Mark Xwayland (`XWAYLAND` extension or vendor string) as `XwaylandRawMotion`. |
| `WaylandRelativeCapture` | Linux Wayland | A separate `wl_event_queue` on the **window's** `wl_display`, dispatched on a capture thread | Must share the window's connection (focus-scoped protocol). Getting winit's `wl_display` via raw-window-handle and binding a second queue: feasibility UNVERIFIED (R-exp). Record `utime` and the compositor name (`XDG_CURRENT_DESKTOP`) since `utime` resolution differs. |
| `IoHidCapture` | macOS | Dedicated thread with its own CFRunLoop, or `IOHIDManagerSetDispatchQueue` | Input Monitoring prompt; **non-seize** open; `RegisterInputReportWithTimeStampCallback`. |
| `GcMouseCapture` | macOS | GameController `handlerQueue` | Foreground only. Stamp at receipt and read `lastEventTimestamp`. |
| `MacEventCapture` | macOS | **Main thread** (AppKit event loop): local `NSEvent` monitor reading `event.cgEvent` field 170/171 plus `deltaX`, with mouse coalescing disabled | Gives L2? and L3 side by side. winit exposes no NSEvent hook, so this needs a local monitor (UNVERIFIED interplay with winit). |
| `WebCapture` | wasm | **Main thread only** (DOM events) | Own `requestPointerLock({unadjustedMovement:true})` with a fallback that records `unadjusted=false`. `pointerrawupdate` + `getCoalescedEvents()`. Serve COOP/COEP (COI) for 5 µs/20 µs stamps. Log `pointerlockchange`/`error`, `visibilitychange` and `blur` as `Gap`. Hand off to a worker via `SharedArrayBuffer` only if needed. |

### 8.2 What must run where

- **Dedicated threads.**
  - Windows Raw Input: queue drain latency and 8 kHz floods; the Godot evidence is in §1.3.
  - evdev/hidraw: the 128-event ring overflows in about 5 ms at 8 kHz.
  - IOHID, the X11 raw connection, the Wayland capture queue.
- **The event loop.** Window and cursor-grab management (winit/SDL), the macOS NSEvent monitor, and all browser capture.
- **The simulation (L5/L6)** consumes `InputEvent`s by `t_rx`, binned to ticks. Integer counts stay integer until multiplied by `deg_per_count`. The remainder policy (none needed when applying in angle space) is part of provenance.

### 8.3 Runtime contamination and scaling detection

1. **Static configuration snapshot** stored in `DeviceInfo`:
   - Linux: libinput profile and speed (`xinput list-props` "libinput Accel Profile Enabled" on X11; gsettings or KWin config on Wayland), `MOUSE_DPI`, the libinput version (≥ 1.30 means plugins are possible), and loaded HID-BPF programs.
   - Windows: `UpperFilters` of the Mouse class and the device, `SM_REMOTESESSION`, and EPP state (only relevant for L3).
   - macOS: pointer speed setting, and whether Input Monitoring was granted.
   - Browser: engine, OS, DPR, zoom, and whether the unadjusted lock resolved.
2. **Dual-source ratio test (main tool).** Run an L1 source (hidraw/IOHID/USB capture) next to the stream under test. Over sliding windows compute `r = Σ|Δ_test| / Σ|Δ_L1|` and regress `r` on the L1 speed (counts/s):
   - `r ≡ 1`: clean.
   - `r` constant ≠ 1: linear scaling (DPR, surface scale, CPI normalisation, flat factor).
   - `r` rising with speed: **acceleration**.
   - per-event mismatches with equal sums: coalescing or resampling.
   - missing sums: drops.
3. **Speed-sweep invariance without L1.** For any source claimed L2, total displacement for the same physical path must not depend on stroke speed. Use a mechanical rig, or a hardware HID injector (an RP2040/Teensy USB mouse replaying a known count script at 1/4/8 kHz). The injector is the strongest test because it exercises the whole OS stack with known truth.
4. **Software injection** (Linux `uinput`, Windows `SendInput`, macOS `CGEventPost`) checks the processed paths only. Injected events are flagged by the OS (Windows `hDevice = 0` / `LLMHF_INJECTED`; Linux virtual device), so they must be labelled `Injected` and are not proof for real devices.
5. **Quantisation fingerprints.**
   - A non-integer delta from a source asserted `integral` means scaling.
   - Deltas that are all multiples of 1/s for some s (e.g. 0.8 at 1.25 scale) mean a scale factor.
   - Timestamps whose µs value is always ≡ 0 mod 1000 mean an ms source (sway `utime`, X11).
   - Inter-arrival histograms should peak at the report interval (1 ms locally). A peak at the frame interval means rAF or frame coalescing.
6. **Sequence and conservation.** `seq` gaps without a `Gap` event are a backend bug. Σcounts across a Windows background/foreground transition is a coalescing-losslessness check (W2).

---

## 9. Verification experiments (consolidated)

**Tags**
- **[local-now]**: runnable on the user's machine without permission changes.
- **[perm]**: needs a user decision (group, ACL, root).
- **[hw]**: needs other hardware or OS.

Full procedures for W*, R* and E* are in §1, §4.5 and §6.

### 9.1 Linux and local machine

| ID | Question | Method | Expected |
|---|---|---|---|
| **L1** [local-now] | Are Xorg `XI_RawMotion` raw values identical to HID counts, one event per report? | Read `/dev/hidraw2` (non-grabbing; parse the 8-byte report) and `XI_RawMotion` (`xinput test-xi2 --root`, or a tiny client) at the same time. Slow and fast strokes. | Σ equal. Per-event equal (libinput unaccel, rotation 0). Events with dx = dy = 0 absent on the X side. X timestamps ms. |
| **L2** [local-now] | Acceleration contamination of the cursor path | Same as L1, plus XI_Motion/core deltas and Chromium/Firefox `movementX` (E1/E2). | Ratio rises with speed (adaptive profile active locally). |
| **L3** [perm] | Is the evdev timestamp the kernel flush time? Latency of hidraw-read vs evdev | With read access to event7, compare evdev `timeval` (CLOCK_MONOTONIC via EVIOCSCLOCKID) with the hidraw `read()` CLOCK_MONOTONIC for matched reports. | evdev stamp ≤ hidraw read time, within tens of µs. Same packet = same stamp. |
| **L4** [perm] | evdev ring overflow at high rate | 8 kHz wired mouse; reader at normal priority under CPU load; count SYN_DROPPED. | Drops appear when the reader stalls > ~5 ms. |
| **L5** [hw] | Wayland `dx_unaccel` fidelity and `utime` | GNOME, KDE and sway sessions; client with relative pointer + lock vs hidraw; fractional scale 1.0/1.25/2.0; Xwayland client XI2 raw. | Mutter/KWin: unaccel = counts, µs stamps. Sway: ms×1000 stamps. Xwayland raw scaled by surface scale. |
| **L6** [perm] | Flat profile identity | Temporarily set libinput flat, speed 0 (a *user* action). Compare XI_Motion/accelerated vs raw. | accel == unaccel == counts (filter-flat factor 1). |
| **L7** [hw] | libinput ≥ 1.30 plugin / HID-BPF contamination | On a distro with libinput ≥ 1.30 and udev-hid-bpf: load a test plugin that scales REL_X; check detection via §8.3. | Unaccel changes; L1 (hidraw) unaffected by plugins but affected by HID-BPF. |

### 9.2 macOS

| ID | Question | Method |
|---|---|---|
| **M1** [hw] | Units of `kCGEventUnacceleratedPointerMovementX`, GCMouse deltas and `NSEvent.deltaX` vs HID counts | IOHID report callback (non-seize, Input Monitoring) recorded together with a local NSEvent monitor (coalescing off) and GCMouse. Pointer speed slider at min/default/max. Slow/fast sweeps. |
| **M2** [hw] | TCC scope | IOHIDDeviceOpen on (a) the mouse interface, (b) a vendor-only interface of the same device, without Input Monitoring granted. Observe the prompt and the "TCC deny" log. |
| **M3** [hw] | GCMouse rate and per-report granularity | 1/4/8 kHz mouse: count callbacks/s vs IOHID reports/s. Check foreground-only behaviour. |
| **M4** [hw] | Timestamp domains | IOHID AbsoluteTime vs `NSEvent.timestamp` vs `CGEventGetTimestamp` for the same motion; check sleep behaviour (mach vs continuous). |

### 9.3 HID and cross-platform

| ID | Question | Method |
|---|---|---|
| **H1** [hw] | Windows: mouse TLC unreadable | hidapi open of the mouse TLC path, expecting a no-access handle; vendor TLC readable. |
| **H2** [hw] | USB-bus truth vs L1/L2 on every OS | usbmon (Linux, root) / USBPcap (Windows) / hardware analyser alongside hidraw/evdev/RawInput/IOHID; 1/2/4/8 kHz. Includes W1. |
| **H3** [hw] | Firmware processing | Straight-line rig at fixed angles (0°, 3°, 45°) with angle snapping on/off and different LOD settings. Detect snapping in the L1 counts. |
| **H4** [hw] | Hardware injector | RP2040/Teensy HID mouse replaying known count scripts. The universal ground-truth stream for all layers incl. browsers. |

### 9.4 Rust layers (from §4.5)

| ID | Question | Method |
|---|---|---|
| **R1** [local-now] | Does winit X11 collapse all mice into one `DeviceId`? | `xinput test-xi2 --root` while moving two mice, plus a winit `device_event` logger. |
| **R2** | winit #2884 under RDP/Hyper-V | Reproduce on a Windows host. |
| **R3** | winit vs SDL raw-thread CPU and drops at 4/8 kHz | Windows, high-polling-rate mouse. |
| **R4** [local-now] | SDL X11 relative timestamp bug | Log `event.motion.timestamp` vs `SDL_GetTicksNS()` (the X server ms value leaks into the ns field). |
| **R5** | SDL Windows interpolation error | Compare interpolated stamps with per-packet QPC at 8 kHz. |
| **R6** | Wayland second event queue on winit's display | Feasibility check. |
| **R7** | `is_cursor_lock_raw()` accuracy | Per browser×OS, using H4. |

### 9.5 Browser and Windows

- **Browser E1–E11 (§6, "Experiments (browser)").**
  - [local-now]: E1 (Linux `unadjustedMovement` rejection), E2 (browser vs hidraw2 ratio), E3 (DPR 1.25 via `--force-device-scale-factor` / `layout.css.devPixelsPerPx` in a throwaway profile), E4 (zoom), E5 (timestamp grids, with and without COI), E11 (rAF vs rawupdate).
  - Others [hw].
- **Windows W1–W6 (§1, "Experiments (Windows)")**, all [hw]:
  - W1 USB-capture pass-through
  - W2 Win11 background coalescing and losslessness
  - W3 API equivalence under EPP/speed changes
  - W4 rate and timestamps, GameInput granularity
  - W5 filter-driver detection
  - W6 RDP/VM absolute packets and flag 0x40

**Suggested first pass (cheapest, highest value).** Run L1 + L2 + E1/E2/E3/E5 on this machine. One recording session with hidraw2 as truth would establish the following for X11:
- the XI2-raw = counts identity;
- the browser acceleration curve;
- browser timestamp grids.

---

## 10. Open questions (consolidated)

**Windows**
- Is Raw Input 1:1 per HID report? Is Win11 background coalescing lossless, and what exactly are its rate and definition?
- GameInput: timestamp base, per-report granularity, and whether it is raw. Is WinRT `MouseDelta` raw?
- Does MOUHID scale X/Y? Is `hDevice == 0` reliable for injected input?

**Linux**
- Do non-Mutter/KWin/wlroots compositors (Hyprland, niri, COSMIC, GameScope) pass libinput unaccel and µs `utime` unchanged?
- What is the base of `utime` in practice?
- Can a capture queue be attached to winit's `wl_display`?
- Does libei/InputCapture deliver accelerated or unaccelerated motion?

**macOS**
- Units and scaling of field 170 and of GCMouse.
- Does Input Monitoring cover vendor TLCs on the mouse interface?
- Do listen-only mouse event taps require it?
- How does NSEvent coalescing interact with ProMotion?

**Browser**
- Chrome Android unadjusted mechanism; Firefox and Safari re-lock throttling.
- Are coalesced lists complete at 8 kHz?
- Which Safari release first shipped fractional `movementX`?

**Firmware**
- What is the Series One PRO's sensor-to-report pipeline (smoothing, motion sync, angle snapping defaults)? Readable via the vendor channel (hidraw4)?

---

## 11. Sources for §0, §2, §3, §5, §7–§10 (accessed 2026-11-14)

§1, §4 and §6 carry their own source lists (accessed 2026-11-13).

**Linux kernel** (K = https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/)
- `drivers/input/input.c`
- `drivers/input/evdev.c`
- `include/linux/input.h`
- `include/uapi/linux/input.h`
- `drivers/hid/hid-input.c`
- `drivers/hid/hid-core.c`
- `drivers/hid/hidraw.c`
- `drivers/hid/usbhid/hid-core.c`
- `Documentation/input/event-codes.rst`, `Documentation/input/input.rst`
- `Documentation/hid/hidraw.rst`, `Documentation/hid/hid-bpf.rst`

**libinput** (main `0685c9787e59`): https://gitlab.freedesktop.org/libinput/libinput
- `src/libinput.h`
- `src/evdev.c`
- `src/evdev-fallback.c`
- `src/filter-flat.c`
- `doc/user/normalization-of-relative-motion.rst`
- `doc/user/pointer-acceleration.rst`
- `doc/user/lua-plugins.rst`
- Tags 1.30.0 / 1.32.0 (2026-09-17)

**X.Org**
- xserver main `f6839e1d9bef`: `dix/getevents.c`, `os/utils.c`, `hw/xwayland/xwayland-input.c` — https://gitlab.freedesktop.org/xorg/xserver
- xf86-input-libinput `src/xf86libinput.c`: https://gitlab.freedesktop.org/xorg/driver/xf86-input-libinput
- xorgproto `specs/XI2proto.txt`: https://gitlab.freedesktop.org/xorg/proto/xorgproto

**Wayland**
- wayland-protocols `unstable/relative-pointer/relative-pointer-unstable-v1.xml`, `unstable/pointer-constraints/pointer-constraints-unstable-v1.xml`, `unstable/input-timestamps/input-timestamps-unstable-v1.xml`: https://gitlab.freedesktop.org/wayland/wayland-protocols
- wayland.app pages:
  - https://wayland.app/protocols/relative-pointer-unstable-v1
  - https://wayland.app/protocols/input-timestamps-unstable-v1
  - https://wayland.app/protocols/pointer-warp-v1
- Compositors and portals:
  - Mutter `src/wayland/meta-wayland-pointer.c`, `src/backends/native/meta-seat-impl.c`: https://gitlab.gnome.org/GNOME/mutter
  - KWin `src/wayland/relativepointer_v1.cpp`, `src/backends/libinput/events.cpp`, `src/pointer_input.cpp`: https://github.com/KDE/kwin
  - wlroots `backend/libinput/pointer.c`: https://gitlab.freedesktop.org/wlroots/wlroots
  - sway `sway/input/cursor.c`: https://github.com/swaywm/sway
  - libei `proto/protocol.xml`: https://gitlab.freedesktop.org/libinput/libei
  - xdg-desktop-portal `data/org.freedesktop.portal.InputCapture.xml`: https://github.com/flatpak/xdg-desktop-portal

**systemd**
- https://github.com/systemd/systemd/blob/main/rules.d/70-uaccess.rules.in
- https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html (TakeDevice)

**Apple** (rendered at https://developer.apple.com/documentation/…; JSON at https://developer.apple.com/tutorials/data/documentation/…)
- AppKit: `appkit/nsevent/deltax`, `appkit/nsevent/ismousecoalescingenabled`, `appkit/nsevent/timestamp`
- Core Graphics: `coregraphics/cgeventfield/mouseeventdeltax`, `coregraphics/cgeventtimestamp`, `coregraphics/cgassociatemouseandmousecursorposition(_:)`, `coregraphics/cgwarpmousecursorposition(_:)`, `coregraphics/cgpreflightlisteneventaccess()`
- Game Controller: `gamecontroller/gcmouse`, `gamecontroller/gcmouseinput`, `gamecontroller/gcmousemoved`, `gamecontroller/gcphysicalinputprofile/lasteventtimestamp`, `gamecontroller/gccontroller/shouldmonitorbackgroundevents`
- IOKit / kernel: `iokit/iohidrequesttype/kiohidrequesttypelistenevent`, `kernel/1462446-mach_absolute_time`
- IOHIDFamily tag `IOHIDFamily-2238.120.5`: `IOHIDFamily/IOHIDDevice.cpp`, `IOHIDLib/IOHIDDeviceClass.m`, `IOHIDFamily/IOHIDKeys.h`, `IOHIDEventSystemPlugIns/IOHIDPointerScrollFilter.cpp` — https://github.com/apple-oss-distributions/IOHIDFamily
- SDK headers (MacOSX11.3.sdk): `IOKit.framework/Headers/hidsystem/IOHIDLib.h`, `hid/IOHIDValue.h`, `hid/IOHIDDevice.h`, `CoreGraphics.framework/Headers/CGEventTypes.h` — https://github.com/phracker/MacOSX-SDKs (third-party mirror of Apple SDK headers)

**HID / other**
- hidapi `mac/hid.c`, `windows/hid.c`: https://github.com/libusb/hidapi
- Chromium `components/input/web_input_event_builders_mac.mm`; WebKit `Source/WebCore/platform/mac/PlatformEventFactoryMac.mm`
- PixArt PMW3360 features, secondary: QMK `drivers/sensors/pmw3360.h` and the LCSC datasheet mirror (did not render; UNVERIFIED)

**Local (read-only)**
- `xinput list`, `xinput list-props 9`, `udevadm info -q property -n /dev/input/event7`, `ls -l /dev/input /dev/hidraw*`
- sysfs: `/sys/class/input/event7/device/capabilities/rel`, `/sys/bus/usb/devices/1-4/*` (speed, ep intervals, interface classes), `/sys/bus/hid/devices/0003:258A:2031.000{5,6,7}/report_descriptor`
- `/etc/udev/rules.d/99-glorious-series-one-pro.rules`, `/usr/lib/udev/rules.d/{50-udev-default,70-uaccess}.rules`
- `dpkg-query` package versions, `/boot/config-7.0.0-42-generic` (`CONFIG_HZ=1000`)
