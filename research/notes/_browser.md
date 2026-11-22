# KineForge input research: §6 Browser (+ browser rows of §7)

All claims accessed **2026-11-13**. Source line numbers refer to `main` as fetched that day
(chromium/chromium HEAD ≈ `a0facb83b514`, mozilla-firefox/firefox ≈ `6beeccd5eef3`,
WebKit/WebKit ≈ `cebeae1c387e`, mdn/browser-compat-data ≈ `69fcf3608174`). Local browsers are
older (Chromium 156.0.8078.79, Firefox 159.0), so every source-derived claim gets a runtime check
in the experiments section. "UNVERIFIED" means I could not confirm it from a primary source.

Abbreviations: **BCD** = MDN browser-compat-data; **DIP** = device-independent pixel
(physical px ÷ OS scale factor); **COI** = cross-origin isolated (COOP+COEP);
**TLC** = HID top-level collection.

---

## 6.0 Bottom line: what a browser can and cannot measure

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

## 6.1 Pointer Lock spec state

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

## 6.2 `requestPointerLock()` returns a Promise

| Browser | Pointer Lock | Promise return | Source |
|---|---|---|---|
| Chrome/Edge | 37 | **92** | BCD `api/Element.json` `requestPointerLock` note |
| Firefox | 50 | Yes. Current IDL is `Promise<undefined> requestPointerLock(optional PointerLockOptions options = {})`. BCD gives no version note (UNVERIFIED first version). | `dom/webidl/Element.webidl` L349-358 |
| Safari | 10.1 | **18.4** | BCD note; BCD commit `45d96dad` (2025-07-24) |
| Chrome Android | 144 | 144 | BCD |
| Safari iOS | not supported per BCD. WebKit has IOS_FAMILY code paths for unadjusted movement, so the iPadOS status is UNVERIFIED. | | BCD; `WebPageIOS.mm` L4051 |

Feature detection: call `requestPointerLock({unadjustedMovement:true})` and catch `NotSupportedError`.
You cannot detect support without a request; this is an open question in the W3C thread.

## 6.3 `unadjustedMovement` support by browser and OS (source-verified)

| | Windows | macOS | Linux X11 | Linux Wayland | ChromeOS | Android |
|---|---|---|---|---|---|---|
| **Chrome ≥ 88** | ✔ WM_INPUT raw | ✔ CGEvent unaccelerated field | **✘ NotSupportedError** | **✘ NotSupportedError** | ✔ Ozone evdev | ✔ (144; mechanism UNVERIFIED) |
| **Firefox ≥ 152** | ✔ WM_INPUT raw | ✔ CGEvent unaccelerated field | **✘** | **✘** | n/a | ✘ (throws NotSupportedError, per bug comment) |
| **Safari ≥ 18.4** | n/a | ✔ (double) | WebKitGTK/WPE **✘** | WebKitGTK/WPE **✘** | n/a | iPadOS: UNVERIFIED |

### Chrome (Chromium main)

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

### Firefox (152+, shipped by Bugzilla 2037802, pref `dom.pointer-lock.unadjusted-movement.enabled`)

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

### Safari / WebKit

- `PointerLockController::requestPointerLock` rejects `NotSupportedError` "Unadjusted movement is unavailable." when
  `!supportsUnadjustedMovement()` (`Source/WebCore/page/PointerLockController.cpp` L106-110).
- `supportsUnadjustedMovement()` is
  `HAVE(MOUSE_UNACCELERATED_MOVEMENT) || PLATFORM(IOS_FAMILY)` combined with `hasAccessoryMousePointingDevice()` (L329-336).
- `HAVE_MOUSE_UNACCELERATED_MOVEMENT` is defined only for `PLATFORM(MAC)` (`Source/WTF/wtf/PlatformHave.h` L1515-1517).
- Mac `WebPage::hasAccessoryMousePointingDevice()` returns `true` (`WebPage.cpp` L12093-12096).
- Values come from `CGEventGetDoubleValueField(…, kCGEventUnacceleratedPointerMovementX/Y)` (`Source/WebCore/platform/mac/PlatformEventFactoryMac.mm` L712-718, L749).
- WebKitGTK/WPE (Epiphany on Linux) therefore reject.

## 6.4 `movementX/Y`: units, quantisation, provenance

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

## 6.5 Delivery: rAF alignment, coalescing, `pointerrawupdate`, `getCoalescedEvents`

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

## 6.6 Timestamps (`event.timeStamp`)

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

## 6.7 Lock lifecycle: gesture, Esc, cooldown, rate limit, focus

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

## 6.8 Keyboard Lock (Esc for in-game use)

- `navigator.keyboard.lock(['Escape'])` exists only in **Chromium** (Chrome 68; BCD shows no Firefox or Safari). Spec: <https://wicg.github.io/keyboard-lock/>.
- Chrome applies the lock **only while the WebContents is fullscreen** (`keyboard_lock_controller.cc` L196-205).
- With Esc locked, the user must **press and hold Esc for 1500 ms** (`kHoldEscapeTime`, L28; `exclusive_access_manager.cc` L30) to exit. That exits fullscreen, the keyboard lock and pointer lock, and starts the pointer-lock cooldown.
- Without fullscreen plus keyboard lock, a single Esc always exits pointer lock.

## 6.9 WebHID, WebUSB, Gamepad

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

## 6.10 Local machine (read-only observations, 2026-11-13)

- Linux Mint, kernel 7.0.0-42, **X11** session (`XDG_SESSION_TYPE=x11`), Chromium 156.0.8078.79, Firefox 159.0.
- `xinput list-props 9` (Glorious Series One PRO Wireless):
  - `libinput Accel Profile Enabled: 1, 0` means **adaptive** (not flat).
  - `Accel Speed 0.0`.
  - Device node `/dev/input/event7`.
- Consequence: on this machine, every browser's movementX is libinput-adaptive-accelerated. `unadjustedMovement` is rejected in both installed browsers (expected; verify with E1).
- `libinput` CLI is not installed.

---

## 7. Taxonomy: browser rows

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

## Anomalies found in browser source (each needs a runtime check)

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

## Experiments (browser)

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

## Open questions

- Exact units of `kCGEventUnacceleratedPointerMovementX` relative to HID counts (resolution scaling in IOHIDPointing?). Native macOS section.
- Chrome Android 144 unadjusted mechanism (presumably `View.requestPointerCapture` relative axes). Not traced.
- Firefox mid-lock blur handling, and whether Firefox or Safari implement any re-lock throttling.
- Whether Chrome's `pointerrawupdate` and coalesced lists stay complete at 8 kHz (IPC and main-thread pressure).
- Which Safari release first shipped fractional `movementX` (WebKit `8dc5033bb0`, Aug 2025).
- No browser exposes device identity, CPI, or report timestamps (pointerlock#98 is open), so per-device calibration must happen natively or through WebHID vendor channels.

## Sources (all accessed 2026-11-13)

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
