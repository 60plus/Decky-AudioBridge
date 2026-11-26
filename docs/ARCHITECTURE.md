# Architecture

Audio Bridge has three parts:

```
Quick Access panel (React)            SteamOS, inside Steam
        |  Decky callable "request"
Plugin backend (main.py, Python)      SteamOS, Decky's plugin process, desktop user
        |  systemd-run --user / systemctl --user / status file / SIGUSR1
Sender engine (audiobridge_sender.py) SteamOS, transient user unit "audiobridge-sender"
        |  pw-record captures the default output's monitor
        |  VBAN over UDP, port 6980
Receiver (AudioBridgeReceiver.exe)    Windows PC, plays through WASAPI
```

## Plugin backend

`main.py` is a thin Decky adapter. All logic lives in `py_modules/audiobridge`:

| Module | Job |
| --- | --- |
| `settings.py` | Defaults, validation, atomic save, presets |
| `service.py` | Starts and stops the engine, reads its status |
| `backend.py` | Every request the panel makes, and the words it shows |
| `discovery.py` | Finds receivers with a PING0 broadcast |
| `diagnostics.py` | The troubleshooting report |
| `vban.py` | Packet layout |
| `engine.py` | The sender itself |

The panel calls one backend method, `request(method, args)`, and gets
`{ok, data}` or `{ok: false, error}` back. Errors meant for the user are plain
sentences.

The backend runs without the `root` flag, so Decky runs it as the desktop user.
It has no session environment, so `service.py` sets `XDG_RUNTIME_DIR` and
`DBUS_SESSION_BUS_ADDRESS` itself from the user id.

Settings live in Decky's plugin settings folder
(`~/homebrew/settings/AudioBridge/settings.json`). They carry a schema
version; a file with some bad values keeps the good ones.

## Sender engine

The engine is started with the system Python 3 as a transient systemd user unit:

```
systemd-run --user --unit audiobridge-sender --collect \
  --property=Restart=on-failure --property=RestartSec=2 \
  -- python3 -s -E .../py_modules/audiobridge_sender.py --host ... --port ... --preset ...
```

Running it in the user's own service manager puts it next to PipeWire, keeps it
alive when Decky reloads or updates the plugin, and leaves nothing behind
(`--collect` removes the unit once it stops). If the user manager cannot be
reached, the backend starts the engine as a detached process and remembers its
PID instead.

The engine:

1. Starts `pw-record --raw -P '{ stream.capture.sink = true }'` with the
   preset's sample format and latency. The capture stream has no target, so the
   session manager links it to the monitor of the default output and moves it
   when the default changes. If `pw-record` is missing or too old for `--raw`,
   `parec --device=@DEFAULT_MONITOR@` is used.
2. Cuts the PCM into VBAN packets (64 or 128 frames) and sends them with UDP.
   The socket asks for DSCP EF so routers that care can prioritise it.
3. Sends a PING0 to the receiver once a second and measures the reply time.
4. Writes its state to `$XDG_RUNTIME_DIR/audiobridge/status.json` twice a
   second. The backend treats a status older than three seconds as a hung
   engine.
5. Restarts capture with backoff when it stops, for example when PipeWire
   restarts after sleep.
6. Replaces the outgoing audio with a short two-note sound on `SIGUSR1`.
   That is the **Play a test sound** button; it only plays on the receiver.

The capture is taken before the output's volume, so changing the volume on the
gaming PC does not change the stream (measured on SteamOS 3.9.5).

## Protocol

The audio is VBAN PCM: 28-byte header, then interleaved little-endian samples.
48 kHz stereo, 16-bit or 24-bit. Frame counters let the receiver count lost and
late packets.

Identification uses the VBAN SERVICE sub-protocol, PING0 function
(`format_SR = 0x60`, `format_nbs = 0x00` for a request and `0x80` for a reply,
`format_nbc = 0`), with the 676-byte PING0 payload. Audio Bridge adds hints to
the request's `UserComment_utf8` field:

```
audiobridge=1;preset=balanced;buffer=40
```

The receiver uses `buffer` as its target delay when its Delay setting is
Automatic. Other VBAN programs ignore the field.

## Receiver

The receiver is a single Rust program (`receiver/`) with four threads:

- **UI**: egui window, settings, status.
- **Network**: receives on UDP 6980, checks every packet, answers PING0, feeds
  the playback buffer.
- **Audio**: owns the WASAPI output stream through cpal, reopens it when the
  device disappears or the Windows default changes, and keeps the device list
  current.
- **Sound card callback**: pulls audio from the playback buffer.

`playout.rs` is the core. It keeps a cushion of audio equal to the target
delay, starts playing once the cushion is full, and resamples with cubic
interpolation. The resampling ratio is nudged by up to 0.3 percent to keep the
cushion at its target, which absorbs the drift between the two computers'
clocks and also handles 44.1 kHz devices. Lost packets are concealed with a
short fade so timing is kept without clicks. When the buffer runs dry it
refills before playing again, and on Automatic the target grows by 10 ms after
two underruns within ten seconds.

The receiver plays one sender at a time: the first one it hears, until that
sender has been silent for 1.5 seconds. Packets from addresses outside private
ranges are dropped. By default it listens on the address Windows uses for the
local network and moves with it if it changes.

Settings and a small log are kept in `%APPDATA%\AudioBridge`.

## Why VBAN and not SonoBus

This project started as a bridge to SonoBus. SonoBus has no supported way to
run without its window, connects through a group server (an internet one by
default), and would need JACK routing to capture system audio. VBAN is a
simple, documented LAN protocol that PipeWire already implements and that has
existing receivers on every desktop system, so the sender can stay a small
standard-library Python program and the receiver a single file.
