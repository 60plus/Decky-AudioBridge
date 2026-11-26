# Third party notices

## Plugin

The plugin frontend is built against the Decky frontend libraries, which are
bundled into `dist/index.js`:

- @decky/api 1.1.2 and @decky/ui 4.10.4, LGPL-2.1, SteamDeckHomebrew.
  The license is included in `licenses/LGPL-2.1.txt`. The original sources
  ship in the pinned npm packages; this repository's source, lockfile and
  build scripts allow rebuilding with modified libraries.
  https://github.com/SteamDeckHomebrew/loader-api
  https://github.com/SteamDeckHomebrew/decky-frontend-lib
- React and React DOM 18.3.1, MIT, Meta Platforms, Inc. and affiliates.
  Provided by Steam at runtime, not bundled.

The plugin backend and sender use only the Python standard library. At run
time they call `pw-record` (PipeWire, MIT) or `parec` (PulseAudio utilities,
LGPL-2.1), which are part of the operating system and not distributed here.

## Receiver

The Windows receiver is written in Rust. Its main libraries are eframe and
egui (MIT OR Apache-2.0), cpal (Apache-2.0), serde (MIT OR Apache-2.0),
socket2 (MIT OR Apache-2.0), if-addrs (MIT OR BSD-3-Clause), winreg (MIT)
and windows-sys (MIT OR Apache-2.0). egui's built-in fonts are under the SIL
Open Font License 1.1 and the Ubuntu Font Licence 1.0.

The complete list of crates in the receiver, with the full text of every
license, is in `receiver/THIRD_PARTY_LICENSES.txt` and is shipped next to
`AudioBridgeReceiver.exe` in releases.

## Protocol

Audio Bridge implements the VBAN protocol as published by VB-Audio Software
in the "VBAN Protocol Specifications". VBAN, Voicemeeter and VBAN Receptor are
products of VB-Audio Software. Audio Bridge is an independent project and is
not affiliated with VB-Audio.
