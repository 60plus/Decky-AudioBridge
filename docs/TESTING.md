# Testing

## Automated

```sh
sh scripts/build-plugin.sh      # TypeScript check, frontend build, Python tests, package
sh scripts/build-receiver.sh    # rustfmt, Rust tests, clippy, Windows build
```

Python tests (`tests/test_*.py`) run on Python 3.9 to 3.14. They cover the VBAN
encoding, settings validation, the engine against a fake receiver (packet
format, sequence numbers, pings, the test sound, a receiver that does not
answer, an unknown host), discovery, and every state the panel can show.

Rust tests cover packet decoding, the PING0 reply layout, the playback buffer
(start, clock drift in both directions, 44.1 to 48 kHz, underrun recovery,
lost and late packets, bursts, channel layouts, volume) and the network thread
over loopback (pings, a second sender, junk packets, a port in use).

## On a SteamOS machine without Decky

`tests/smoke_steamos.py` drives the real plugin backend with a stand-in `decky`
module. The engine runs as the real user unit and captures the real output
monitor; a fake receiver listens on localhost. It plays a very quiet tone
(about -50 dBFS) so the capture has something to measure.

```sh
# copy main.py, py_modules/ and tests/ to the machine, then:
env -u XDG_RUNTIME_DIR -u DBUS_SESSION_BUS_ADDRESS python3 -I tests/smoke_steamos.py
```

Unsetting the session variables reproduces the environment Decky gives the
backend.

## Manual checks before a release

Functional

- Install the ZIP through Decky, open the panel, find the receiver, start.
- Audio plays on the receiver; the test sound plays only there.
- Stop, start again, change the preset while streaming.
- Restart the receiver while streaming; the plugin shows "not answering" and
  recovers on its own.
- Reboot the gaming PC with automatic start on; streaming starts by itself.
- Settings survive a reboot and a plugin update.

Gaming Mode

- Start before launching a game, and with a game already running.
- Open and close the Quick Access menu while streaming.
- Exit a game and launch another.
- Sleep and resume; capture comes back.
- Change the output device (HDMI, headphones, Bluetooth).

Network

- Wired, 5 GHz Wi-Fi, mixed.
- Unplug the cable for a few seconds.
- Change the receiver's IP address.
- Long sessions: 1, 3 and 8 hours. Watch lost packets and underruns in the
  receiver's diagnostics.

Receiver

- First start shows the Windows Firewall question; Allow works.
- Cancel on that question, then **Allow through Windows Firewall** works.
- Unplug the chosen output device and plug it back.
- Start with Windows and Start minimized.

## Results so far

| Check | Where | Result |
| --- | --- | --- |
| Capture with `pw-record` in Gaming Mode, no routing change | SteamOS 3.9.5, desktop PC | Works, level matches the played tone |
| Backend starts and stops the user unit without session variables | SteamOS 3.9.5 | Works, nothing left behind |
| Test sound reaches the receiver | SteamOS 3.9.5 to local receiver | Works |
| Receiver plays a stream, answers pings, is found by discovery | Windows 11, loopback | Works |
| Find receivers by broadcast, receiver bound to its LAN address | SteamOS 3.9.5 to Windows 11, wired | Works, found in 1.5 s |
| Gaming PC to Windows PC over the LAN | SteamOS 3.9.5 to Windows 11, wired | Works: captured beeps at -20 dBFS and the test sound heard clean on headphones, reply time under 1 ms, no lost packets |
| Panel inside Steam: install from ZIP, find, choose, start, test sound, stop | SPlug 0.1.0b50 on SteamOS 3.9.5 | Works, driven by programmatic clicks over CDP; test sound heard on the receiver |
| Stream keeps running with the Quick Access menu closed | SPlug 0.1.0b50 on SteamOS 3.9.5 | Works |
| Panel with a physical controller | | Not yet tested |
| Panel under Decky Loader | | Not yet tested |
| Steam Deck, Bazzite, ChimeraOS | | Not yet tested |
