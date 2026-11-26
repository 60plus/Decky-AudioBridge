# Changelog

## 0.1.2

- First public release, with store artwork and screenshots. The plugin's
  store entry and links now point to the public repository.

## 0.1.1

- The Quality list in the panel uses the full width, so the preset name is
  no longer cut off.

## 0.1.0

First version.

- Stream the gaming PC's sound to another computer over the local network,
  started and stopped from the Quick Access menu.
- Capture through PipeWire (`pw-record`), with `parec` as a fallback. No
  changes to audio devices or routing.
- Sender runs as a temporary user service, so closing the menu or reloading
  Decky does not stop the stream.
- Find receivers with one button, or enter an address by hand.
- Low latency, Balanced and High quality presets.
- Play a test sound on the receiver.
- Optional automatic start when the gaming PC starts.
- Diagnostics with hidden addresses, ready to copy.
- Windows receiver: status, output device, volume, mute, delay setting,
  start with Windows, firewall helper, diagnostics.
