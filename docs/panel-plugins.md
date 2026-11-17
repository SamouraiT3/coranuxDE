# Coranux panel plugins

The panel detects plugins at startup and rescans their directories every five
seconds. External plugins are JSON manifests placed in one of these locations:

- `$XDG_CONFIG_HOME/coranux/panel-plugins/` (or `~/.config/coranux/panel-plugins/`)
- `$XDG_DATA_HOME/coranux/panel-plugins/` (or `~/.local/share/coranux/panel-plugins/`)
- `coranux/panel-plugins/` under each directory in `$XDG_DATA_DIRS`

Each `.json` file describes a button that launches a command directly, without
using a shell:

```json
{
  "name": "calendar",
  "label": "Calendar",
  "command": ["my-calendar", "--toggle"]
}
```

A plugin is displayed only if its executable is available in `PATH` or given
as a path. Directories are searched in the order listed above, and only the
first plugin with a given name is added.

Built-in audio controls are detected separately. The panel uses `wpctl` when it
is installed, otherwise it falls back to `pactl`. It can mute or unmute audio
and adjust the volume in 5% steps.

## Application launchers

Pinned launchers are configured separately from plugins in
`$XDG_CONFIG_HOME/coranux/panel-launchers.json` (or
`~/.config/coranux/panel-launchers.json`). The file is reloaded every five
seconds. Buttons appear next to the Coranux name, before open windows. Each
command is run directly, without a shell, and must be available in `PATH` or
specified by a path.

For example, to add Firefox and the Foot terminal:

```json
{
  "launchers": [
    { "label": "Firefox", "command": ["firefox"] },
    { "label": "Terminal", "command": ["foot"] }
  ]
}
```

Replace the commands with executables installed on your system. To find an
application's command, check the `Exec` field in its `.desktop` file. Use the
executable and relevant arguments, omitting field codes such as `%U`. Add
arguments as separate array items, for example:
`["firefox", "--new-window"]`.

The clock is built into the panel and updates every second; it needs no plugin
or additional configuration.

## Keyboard layout

The layout button (`FR`, `US`, etc.) cycles through the keyboard layouts
configured for Coranux. By default, the keyboard starts with French (`fr`), and
the button switches between French and English. The compositor applies the
change to applications in the session.

To use a different list, start Coranux with
`CORANUX_KEYBOARD_LAYOUTS=fr,us,de` (comma-separated XKB layout codes). If this
variable is unset, Coranux uses `XKB_DEFAULT_LAYOUT` when it is defined;
otherwise it uses `fr,us`.

## Starting Coranux

After installing the Arch package, start Coranux from a TTY or a session set up
by your display manager:

```sh
coranuxDE
```

The package also installs a Wayland session entry at
`/usr/share/wayland-sessions/coranux.desktop`. It declares its Wayland type for
LightDM and is detected by display managers that search this directory,
including GDM, SDDM, and LightDM. If your LightDM configuration overrides
`sessions-directory`, add `/usr/share/wayland-sessions` to the list. Select
“Coranux” in the greeter; the package does not change the default session.

Coranux starts its own DRM compositor and XWayland server. The Arch package
installs `xorg-xwayland` to support most X11 applications. Components and
applications launched by the session receive its `DISPLAY` and
`XDG_SESSION_TYPE=wayland` values. The display manager must select Coranux's
Wayland entry to start the session on its VT without an existing X11 server.

Build the package from the project root with `cd packaging/arch && makepkg -si`.
The command starts `coranux_session`, which starts the DRM compositor
(`coranux_wm`), then the desktop (`coranux_desktop`) and panel
(`coranux_panel`). The WM manages windows and composites their Wayland and X11
surfaces; `coranux_desktop` draws the desktop interface. The session waits for
Wayland and XWayland to become ready, then waits for the desktop and panel to
present a frame before reporting a successful startup. If either fails to
appear, the session exits with an error instead of leaving a blank screen.

Do not start Coranux from another graphical compositor such as Sway: Coranux
takes direct control of the display through DRM/GBM. It refuses to start when
`WAYLAND_DISPLAY` indicates an active Wayland compositor, when
`XDG_SESSION_TYPE=x11`, or when `DISPLAY` is set outside a Wayland session. On
systems with multiple GPUs, Coranux tries to select one with a connected
display. The keyboard layout is set at startup, and the panel button shows the
current layout.

From a TTY, start Coranux on the active TTY. If you are logged in to an
inactive TTY, switch to it first with `Ctrl+Alt+F<n>`; the WM refuses to start
on an inactive session rather than leaving a blank screen.

To run development binaries, first rebuild all components with
`cargo build --bins --locked`, then run `cargo run --bin coranux_session`. The
second command only rebuilds the session binary and may otherwise use older
neighboring executables.
