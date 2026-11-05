# clickshift

Per-application mouse button remapper for GNOME on Wayland, written in Rust.
Inspired by the idea of X-Mouse Button Control; it shares no code with it.

```
mouse -> clickshift (evdev grab) -> virtual mouse / keyboard (uinput) -> GNOME
                ^
                | D-Bus: focused app
      GNOME Shell extension
```

## Features

- Map buttons and wheel directions to keys, chords, media keys, other mouse
  buttons, scrolling or shell commands
- Per-application profiles that switch automatically with window focus
- Up to 9 extra layers, held or toggled
- Different actions for a quick click and a long press
- Two-button combos, and sticky buttons for hands-free dragging
- Timed macros that play on a click, while held, or on repeat while held, such
  as `macro-hold:e{WAITMS:330}{LMB}{WAITMS:160}{RMB}`
- Config changes apply while running, and a broken edit is ignored
- Reconnects by itself when the mouse or its receiver comes back
- A settings app, so the config never has to be edited by hand

See `config.example.toml` for the full syntax.

## Build

The settings app needs the GTK 4 and libadwaita development files.

```sh
cargo build --release --workspace
```

## Settings app

`clickshift-settings` edits the config file, and the daemon applies each save
within about a second. It keeps your comments, refuses invalid values, and
reloads instead of overwriting if the file was changed elsewhere.

```sh
target/release/clickshift-settings
```

To add it to the app menu:

```sh
install -D target/release/clickshift-settings ~/.local/bin/clickshift-settings
install -Dm 644 packaging/io.github.clickshift.Settings.desktop ~/.local/share/applications/io.github.clickshift.Settings.desktop
```

## Install

Run the `sudo` commands in a real terminal, because they ask for a password.

1. Give your user access to this one mouse. Edit the name in the rule first if
   you use a different mouse.

   ```sh
   sudo install -m 644 packaging/70-clickshift.rules /etc/udev/rules.d/
   sudo udevadm control --reload
   sudo udevadm trigger --subsystem-match=input --action=change
   ```

   If `clickshift list` still finds nothing, unplug and replug the receiver.

2. Create the config.

   ```sh
   mkdir -p ~/.config/clickshift
   cp config.example.toml ~/.config/clickshift/config.toml
   ```

3. Install the extension, log out and back in, then enable it.

   ```sh
   cp -r extension/clickshift@clickshift.github.io ~/.local/share/gnome-shell/extensions/
   gnome-extensions enable clickshift@clickshift.github.io
   ```

## Try it safely

```sh
target/release/clickshift list                     # see the mouse and its buttons
target/release/clickshift check                    # validate the config
target/release/clickshift run --dry-run --verbose  # print only, the mouse is not taken
target/release/clickshift run --grab-seconds 30    # really remap, then give the mouse back
```

Stop a real run with Ctrl+C from the keyboard. The kernel returns the mouse
whenever the process ends, including after a crash.

## Run on login

```sh
install -D target/release/clickshift ~/.local/bin/clickshift
install -Dm 644 packaging/clickshift.service ~/.config/systemd/user/clickshift.service
systemctl --user daemon-reload
systemctl --user enable --now clickshift.service
```

Do not run it alongside an input-remapper preset on the same mouse, since only
one program can grab it.

## Tests

```sh
cargo test    # logic only, needs no permissions
```

The end-to-end suite runs the real binary against a virtual mouse. It grabs the
output devices before sending anything, so no input reaches the desktop, and it
never touches the real mouse. It needs one extra udev rule:

```sh
sudo install -m 644 packaging/71-clickshift-test.rules /etc/udev/rules.d/
sudo udevadm control --reload
cargo test --test e2e -- --ignored
```

## Limits

- Profiles follow the focused window, not the window under the pointer.
- One mouse per config.
- Macro characters assume a US keyboard layout.
- A macro stops when focus changes, so one cannot switch windows by itself.
- The settings app cannot unlock the left button; set `allow_left_remap` by hand.
- `metadata.json` lists GNOME Shell 51 only; other versions need testing.

## License

MIT, see `LICENSE`.
