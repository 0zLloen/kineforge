# Windows mouse input semantics (KineForge research: §1, Windows parts of §5 and §7)

All URLs accessed 2026-11-13. "MS-SDK" = github.com/MicrosoftDocs/sdk-api (source of learn.microsoft.com/windows/win32/api pages), "MS-Win32" = github.com/MicrosoftDocs/win32 (inputdev conceptual pages, ms.date 07/14/2025), "MS-Drv" = github.com/MicrosoftDocs/windows-driver-docs (branch `staging`). Source pins: SDL `fff6448f523f` (2026-11-12), winit `f0abec90a3f9` (2026-11-13), RawAccel `master` (pushed 2026-04-23). **UNVERIFIED** = no primary source found.

## 1.0 Data path (where counts can be altered)

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

## 1.1 Raw Input registration

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

## 1.2 RAWMOUSE semantics (https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse)

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

### Absolute mode (RDP, VMs, tablets)
- Remote Desktop and tablets deliver `MOUSE_MOVE_ABSOLUTE`; SDL documents RDP behaviour (SetCursorPos calls cached/coalesced, user motion coalesced with warps, so it synthesizes relative motion by differencing + recentre "wobble") — SDL `src/video/windows/SDL_windowsevents.c` L609–660; SDL also honours undocumented `usFlags & 0x40` as "raw coordinates" (L621) → **UNVERIFIED** flag.
- VM integration (Hyper-V/VMware/VirtualBox mouse integration) → absolute: **UNVERIFIED** (no primary doc found); detect at runtime instead.
- **winit bug (verified in source):** `if util::has_flag(mouse.usFlags, MOUSE_MOVE_RELATIVE)` with `has_flag(b,f) = b & f == f` and `MOUSE_MOVE_RELATIVE == 0` is always true, so absolute 0–65535 coordinates are emitted as `PointerMotion { delta }` — `winit-win32/src/event_loop.rs` L2667–2672, `util.rs` L46–51; open issue https://github.com/rust-windowing/winit/issues/2884 (2023-06-16, "Mouse Motion incorrect when using Remote Desktop").
- KineForge rule: any `MOUSE_MOVE_ABSOLUTE` packet ⇒ provenance `os_absolute_pointer` (never device counts); also check `GetSystemMetrics(SM_REMOTESESSION)`.

## 1.3 Reading: standard vs buffered; high polling rates

- Standard: GetMessage → `GetRawInputData(lParam, RID_INPUT, …)` one RAWINPUT at a time. Buffered: `GetRawInputBuffer` returns an array, walk with `NEXTRAWINPUTBLOCK`; "useful for high-frequency devices such as mice at 1000Hz". **Important:** "GetMessage removes the current WM_INPUT from the raw input queue before returning. As a result, GetRawInputBuffer will not see the current event" → pattern: read lParam event with GetRawInputData, then drain with GetRawInputBuffer; or never retrieve WM_INPUT (range-filtered PeekMessage `0..WM_INPUT-1`, `WM_INPUT+1..0xFFFF`) and drain on a timer; "Don't use RIDEV_DEVNOTIFY with this pattern" (WM_INPUT_DEVICE_CHANGE goes via raw input queue → thread spins). MS-Win32 `inputdev/about-raw-input.md`, `inputdev/using-raw-input.md` (ms.date 07/14/2025) https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input , https://learn.microsoft.com/en-us/windows/win32/inputdev/using-raw-input
- `GetRawInputBuffer`: returns count, `(UINT)-1` on error (grow buffer on ERROR_INSUFFICIENT_BUFFER); pointer DWORD-aligned; **WOW64 note:** "do not use *pcbSize, use *pcbSize * 8 instead… you must align the RAWINPUT structure by 8 bytes" (header is 24 bytes for 64-bit layout). https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getrawinputbuffer . `NEXTRAWINPUTBLOCK` aligns to `sizeof(QWORD)` on _WIN64, DWORD otherwise (mingw-w64 `winuser.h` L6399–6404, https://github.com/mingw-w64/mingw-w64/blob/master/mingw-w64-headers/include/winuser.h). SDL handles the WOW64 case by `rawinput_offset = sizeof(RAWINPUTHEADER) + 8` (SDL_windowsevents.c L823–830). Rust: build x86_64 only; avoid WOW64.
- Legacy mouse messages: "When mouse messages are posted faster than a thread can process them, the system discards all but the most recent mouse message" (WM_MOUSEMOVE coalescing) — https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input
- **CPU cost at 4–8 kHz (engineering source, not MS):** Godot 4.7.2/4.8-dev3 (2026-08-24, H. Locurcio) — on Win11 24H2 the per-app input throughput limit is "somewhere between 1 kHz and 2 kHz" on most CPUs; apps that register raw input still get both WM_MOUSEMOVE and WM_INPUT unless NOLEGACY; fix = keep WM_INPUT queued and drain buffered once per frame, dispatch WM_MOUSEMOVE at most once/frame. Measured: captured mouse @8 kHz "under 1 FPS → 1703 FPS"; visible @8 kHz 173 → 1103 FPS (9950X3D). https://godotengine.org/article/fixing-high-polling-rate-mice-on-windows/
- SDL3 pattern (verified source): dedicated thread `SDLRawInput`, message-only window (`HWND_MESSAGE`), `SetThreadPriority(THREAD_PRIORITY_TIME_CRITICAL)`, `MsgWaitForMultipleObjects(…, QS_RAWINPUT)`, drain with `GetRawInputBuffer` — `src/video/windows/SDL_windowsrawinput.c` L60–195.

## 1.4 Timestamps (none from the device)

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

## 1.5 Enhanced Pointer Precision (EPP) and what bypasses it

- XP+ ballistics (archived MS white paper "Pointer Ballistics for Windows XP", updated 2002-10-31): mickeys converted to physical units assuming **USB 125 Hz and 400 mickey/inch** defaults and screen refresh rate/resolution; gain looked up from a 5-point (6 incl. origin) transfer curve on the **vector magnitude** of per-packet X/Y, interpolated; 16.16 fixed point; "divided remainder mouse counts have to be preserved and added into the next counts" (sub-pixel carry); curve stored in registry; algorithm "resides between ring0 and ring3". https://web.archive.org/web/20110623221617/http://msdn.microsoft.com/en-us/windows/hardware/gg463319.aspx  ⇒ because velocity = counts/packet assuming 125 Hz, EPP output depends on report rate (UNVERIFIED whether modern Windows corrects for actual rate).
- Legacy (pre-XP) threshold doubling ×2/×4 still documented on MOUSEINPUT remarks for SendInput relative motion; settings via `SystemParametersInfo(SPI_GETMOUSE)` → 3 ints (threshold1, threshold2, acceleration) and `SPI_GETMOUSESPEED` 1..20 (default 10). https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput , …/nf-winuser-systemparametersinfoa . Mapping "EPP checkbox on ⇔ SPI_GETMOUSE[2]≠0" is folk knowledge → **UNVERIFIED** primary.
- Bypass matrix: **Raw Input (WM_INPUT): bypasses** (RAWMOUSE remarks; "WM_INPUT has no ballistics applied", "read directly from the HID stack") — https://learn.microsoft.com/en-us/windows/win32/dxtecharts/taking-advantage-of-high-dpi-mouse-movement . **WM_MOUSEMOVE / WM_POINTER / GetMouseMovePointsEx / GetCursorPos: ballistics applied + pixel-quantised** ("no WM_MOUSEMOVE message is generated" for sub-pixel motion, same article). **DirectInput: internally "creates a second thread to read WM_INPUT data"** → unaccelerated but more overhead, "discouraged" for mouse (same article). GameInput: deltas "reported natively as a series of delta values" — no explicit "no ballistics" statement → **UNVERIFIED but expected raw** (exp. W3). WinRT `MouseDevice.MouseMoved`/`MouseDelta`: docs only say "change in screen location of the mouse pointer" → raw vs accelerated **UNVERIFIED** (exp. W3).
- None of these bypass kernel filter drivers (RawAccel etc.) or firmware.

## 1.6 Windows 11 background raw-input throttling

- **Primary (MS Windows Developer Blog, Sharif Farag, 2023-05-26, "Reduced game stutter with high report rate mice"):** "throttling and coalescing background raw mouse listeners and capping their message rate"; test on Surface Laptop Studio with a 1000 Hz mouse. No number given. https://blogs.windows.com/windowsdeveloper/2023/05/26/delivering-delightful-performance-for-more-than-one-billion-users-worldwide/
- The figure "**125** messages/s for a 1000 Hz mouse, each containing the combined payload" comes from a Microsoft engineer quoted on Blur Busters forums (https://forums.blurbusters.com/viewtopic.php?p=94890, 403 to fetchers) → **UNVERIFIED (secondary)**. Shipped publicly with Win11 22H2 KB5028185 (2023-07-11) per press (techpowerup.com/311357) → **UNVERIFIED** vs MS release notes. Not in Windows 10 (secondary).
- Reports (2025–26) that some games are misclassified as background on 24H2 and get ~125 Hz (EA forums BF6 thread) → anecdotal.
- What "background" means operationally (RIM_INPUTSINK recipients vs non-foreground process) is undocumented; coalesced packets presumably sum lLastX/Y ("combined payload") — whether sums are lossless and whether `MOUSE_MOVE_NOCOALESCE` is cleared on coalesced packets: **UNVERIFIED** (exp. W2).
- KineForge consequence: capture in the background (e.g. alongside a third-party game) loses temporal resolution (~8 ms bins) though counts may be preserved; tag every packet with `RIM_INPUT`/`RIM_INPUTSINK` and a `foreground` flag; treat background segments as `aggregation=os_coalesced`.

## 1.7 GameInput (mouse)

- Availability: "PC via NuGet … back to Windows 10 19H1"; Microsoft.GameInput latest **4.0.306 (published 2026-10-28)** on nuget.org (registration API). "built directly on top of the Windows hardware interfaces", lock-free, "synchronized to a common time base". https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-overview (ms.date 2026-10-29)
- `GameInputMouseState {buttons, positions, positionX, positionY, absolutePositionX, absolutePositionY, wheelX, wheelY}` (int64); positionX/Y = "Cumulative … change"; "Calculate deltas by subtracting"; trackpads appear as one virtual mouse. https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/structs/gameinputmousestate
- "Individual deltas are accessible when iterating through all intermediate readings"; "The accumulated delta value is only for the mouse events that a process receives while it has input focus"; per-mouse readings. https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/advanced/input-keyboard-mouse
- History: "The input stream maintains the last half second of historical readings" (GetNextReading walk) → must poll ≥2 Hz or use callbacks. https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-readings
- Focus: `GameInputFocusPolicy` removed in v4 → `IGameInput::EnableBackgroundInput`; default "will not provide background input". https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/deprecated/enums/gameinputfocuspolicy
- Raw HID bytes: `GetRawReport` — "only Game Input Protocol (GIP) devices support raw reports" (v0 intro, v1 removed, v3 re-introduced) ⇒ **no raw HID mouse reports via GameInput**. https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/interfaces/igameinputreading/methods/igameinputreading_getrawreport
- SDL3 uses GameInput for mouse only if `SDL_HINT_WINDOWS_GAMEINPUT="1"` (default "0") — SDL `include/SDL3/SDL_hints.h` L4727–4740.
- Value for KineForge: only Windows API giving **per-reading µs timestamps + per-device integer deltas**; whether one reading == one HID report (vs coalesced) is **UNVERIFIED** (exp. W3/W4). Rust: no maintained binding found in this pass (not searched in depth) — COM via `windows` crate would need hand-written vtables.

## 1.8 Cursor confinement (for completeness)
Raw input is independent of cursor position; lock = hide cursor + `ClipCursor` to client rect (and optionally recentre with SetCursorPos, which only affects the cursor stream). RDP caveats above.

## 1.9 Runtime contamination / provenance checks (Windows)
1. `usFlags & MOUSE_MOVE_ABSOLUTE` or `SM_REMOTESESSION` → not device counts.
2. `ulExtraInformation & 0xFFFFFF00 == 0xFF515700` → pen/touch-synthesized.
3. `hDevice == 0` → injected/touchpad (treat as non-device).
4. Read (read-only registry) `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e96f-e325-11ce-bfc1-08002be10318}\UpperFilters` (Mouse setup class GUID per https://learn.microsoft.com/en-us/windows-hardware/drivers/install/system-defined-device-setup-classes-available-to-vendors) and the device's own `UpperFilters`; anything besides `mouclass` (e.g. `rawaccel`) ⇒ flag `possible_filter_driver`. Exact default value list **UNVERIFIED**.
5. Known-motion injection test: a hardware motion rig/known pattern vs captured counts (§9).
6. `GetCurrentInputMessageSource` → `IMO_HARDWARE / IMO_INJECTED / IMO_SYSTEM` (https://learn.microsoft.com/en-us/windows/win32/api/winuser/ne-winuser-input_message_origin_id) — applicability to WM_INPUT / buffered drains **UNVERIFIED**; LL hook `MSLLHOOKSTRUCT.flags & LLMHF_INJECTED` is documented for the cursor path.

## 5-Win. HID-level access on Windows
- "Top-level collections opened by Windows for system use": Mouse 0x01/0x01–0x02 **Exclusive**, Keyboard 0x01/0x06–0x07 Exclusive, pens/touch/PTP Exclusive; consumer controls 0x0C/0x01 Shared. https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/top-level-collections-opened-by-windows-for-system-use (ms.date 01/11/2024). ⇒ a user-mode app (hidapi, `CreateFile`+`ReadFile`, WebHID-on-Windows underneath) **cannot read mouse-TLC input reports**; it can open the path without read/write access to query attributes/strings (behaviour long relied on by hidapi; current doc no longer states it → **UNVERIFIED**).
- Raw Input never hands mouse-TLC report bytes to apps: mouse TLC arrives as parsed `RIM_TYPEMOUSE`; `RAWHID.bRawData` only for non-mouse/keyboard TLCs (RAWINPUTHEADER dwType table). Vendor-defined TLCs (UsagePage ≥0xFF00, e.g. config/telemetry channels) are openable via hidapi or Raw Input `RIM_TYPEHID`.
- GameInput raw reports: GIP only (above). WinUSB: replacing the HID driver on a mouse interface would remove it from the OS input stack (and requires driver install) — out of scope.
- Therefore on Windows "device counts" = **RAWMOUSE.lLastX/lLastY from relative packets with no filter driver**, assumed 1 packet per HID report (W1/W4 to verify); firmware processing (CPI, angle snapping, smoothing, LOD, motion sync) is always included.

## 7-Win. Taxonomy rows

| Layer | Windows source | Units | Quantisation | Aggregation | Timestamp domain/res | Latency contributors |
|---|---|---|---|---|---|---|
| (1) device counts | not directly readable (mouse TLC exclusive); closest = (2) | sensor counts at configured CPI | integer | per HID report (firmware may sum sensor frames) | none from device | sensor frame, MCU, USB poll interval (bInterval)/wireless link |
| (2) OS raw events | Raw Input RAWMOUSE (relative); GameInput readings | counts ("mickeys"), wheel 1/120 notch | int32 (RawInput), int64 cumulative (GameInput) | expected 1 per report (W1); Win11 coalesces for background listeners; app batch-drain | RawInput: none (app QPC at dequeue, <1 µs res but measures dequeue); GameInput: µs "when the input was made" (base UNVERIFIED) | HIDCLASS/MOUHID/MOUCLASS, filter drivers, RIT, thread scheduling, queue drain cadence, background throttle |
| (3) OS cursor events | WM_MOUSEMOVE, WM_POINTER, GetCursorPos, GetMouseMovePointsEx, WinRT PointerPoint | screen px (DIP for WinRT) | integer px; sub-pixel carried internally | coalesced to latest when queue lags | GetMessageTime ms (tick); POINTER_INFO.PerformanceCount QPC at receipt; PointerPoint µs since boot | + ballistics, desktop/DPI scaling, message coalescing |
| (2/3?) WinRT MouseMoved | MouseDelta | "screen location" delta per docs; raw-ness UNVERIFIED | int | per event | none in args (UNVERIFIED) | CoreWindow dispatch |
| (4) browser | (other section) Chromium on Windows uses raw input for unadjustedMovement — not verified here | | | | | |

## Experiments (Windows)
- **W1 — pass-through:** capture USB HID reports (USBPcap/Wireshark on a separate test box, or a hardware USB analyzer) simultaneously with RawInput; assert Σ and per-packet equality of X/Y and 1:1 packet count at 1/2/4/8 kHz.
- **W2 — background coalescing:** same mouse, two processes: foreground RIM_INPUT reader and background RIDEV_INPUTSINK reader; compare event rates, Σcounts over 10 s, `MOUSE_MOVE_NOCOALESCE` flag distribution, on Win10 vs Win11 24H2/25H2.
- **W3 — API equivalence:** record RawInput, GameInput (`positionX` deltas per reading), WinRT MouseMoved, WM_MOUSEMOVE concurrently with EPP on/off and speed 1/10/20; raw-class APIs must be invariant to settings.
- **W4 — rate/timestamp:** inter-arrival histograms of QPC-at-receipt (per-message read on TIME_CRITICAL thread) vs GameInput µs timestamps; check `dwSampleRate` value; check whether GameInput gives one reading per report.
- **W5 — filter detection:** install RawAccel on a test VM/box with known curve; verify UpperFilters check flags it and Σcounts diverge from (W1) USB capture.
- **W6 — RDP/VM:** confirm absolute packets and flag 0x40 under RDP, Hyper-V, VMware, VirtualBox.

## Open / UNVERIFIED summary
125 Hz figure + exact background definition; coalesced-sum losslessness; GetMessageTime granularity; GameInput timestamp base & per-report granularity & raw-ness; WinRT MouseDelta raw-ness; MOUHID scaling; injected hDevice=0; EPP↔SPI_GETMOUSE mapping and whether modern EPP still assumes 125 Hz/400 dpi; UpperFilters default list.
