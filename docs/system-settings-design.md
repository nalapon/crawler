# Native system settings

Crawler uses a native category rail, a local English/Spanish search catalogue and a wide detail area. The ten sections are Sound, Keyboard, General, Wi-Fi, Bluetooth, Battery, Wallpaper, Appearance, Desktop & Dock, and AirPods. There is no WebView. Command-Shift-3 opens the workspace; Command-F searches, arrows choose a section, Tab traverses enabled controls, and Command-R refreshes.

## Reading and applying changes

SystemSettingsReader builds typed rows without blocking AppKit. Carbon input-source calls run on MainActor because the framework asserts its main-queue requirement. Core Audio, CoreWLAN, IOBluetooth and IOKit supply actual hardware values. Sound and AirPods subscribe to Core Audio property changes while the workspace is active. Hardware-list changes rebuild listener registrations; events during a read coalesce into one follow-up read. The five-second refresh remains a fallback for other settings; stale reads cannot replace the selected page. Mutations disable controls while pending, and failures identify the originating section even if the user has navigated elsewhere.

Each editable row invokes a real operation. Core Audio writes devices, output volume/mute, input gain, balance and effects output. Wi-Fi scans use Location authorization; connection sheets collect passwords in secure fields and do not save credentials. Bluetooth uses native pairing and real connect/disconnect operations. Power settings use pmset with administrator authorization. System Events reads and changes appearance and Dock properties after explicit Automation permission.

General configures computer name, time zone, automatic/manual clock, preferred language and locale. Storage browsing enters Crawler's existing file browser. Software Update explicitly checks, downloads or installs through Apple's softwareupdate. Downloads and installations require confirmation; Crawler does not request automatic restart. Command arguments and the enclosing AppleScript string are escaped independently. The command timeout accommodates long installations. The full update report opens in a scrollable native sheet.

Wallpaper shows the current desktop image and a gallery of locally installed image files, then applies a selected thumbnail through NSWorkspace. ImageIO produces bounded thumbnails off the main actor; cancellation and generation checks reject stale loads. Remote/download-only Apple collections are not included. Appearance includes keyboard-focusable Light and Dark window previews that apply the existing System Events operation. Wallpaper also edits scaling/cropping through NSWorkspace. Keyboard repeat uses IOHID nanoseconds, displayed as milliseconds, and persists through CFPreferences. The shortcut editor modifies existing symbolic hotkey definitions, detects conflicting combinations and invokes Apple's activateSettings helper. Some preference consumers need relaunch or a new login. CFPreferences updates preserve unrelated keys.

## Private API boundary and actual limits

The maintainer authorized private API integration to prioritize coverage. SystemSettingsBridge checks dynamic symbols and Objective-C method signatures, and catches framework exceptions. It does not patch framework code, impersonate Apple processes or forge entitlements.

Bluetooth global power uses IOBluetoothPreferenceSetControllerPowerState and polls the actual controller state. AirPods listening modes use the advertised Core Audio lstm/lsms properties. Only advertised modes are editable; an unadvertised current mode is shown without offering an unsupported write.

Conversation awareness and head-tracking operations exist behind capability checks for AVOutputContext endpoints. A nil sharedSystemAudioContext or a missing required entitlement leaves these controls unavailable. The UI reports these unavailable controls. Private API permission from the user does not supply Apple's entitlement.

This implementation is not complete parity with every option in Apple's settings. Dynamic wallpaper collections, full storage administration, all keyboard shortcut categories/defaults, and unavailable advanced headphone controls must not be claimed as implemented.

## Verification

Verify hardware controls through their actual state and readback. A build or successful script compilation does not prove a hardware control works. Never install an OS update as a verification step.

Implementation research: [airpods-control](https://github.com/raulgg/airpods-control) documents mode identifiers and support masks; [blueutil](https://github.com/toy/blueutil) uses the Bluetooth power symbol. Crawler implements its own compatibility bridge.
