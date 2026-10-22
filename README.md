# dtm

A terminal multiplexer written in Rust, with a vertical tab sidebar, a
client/server design (detach and reattach), and everything configurable.

Everything below the PTY is written from scratch: the VT/ANSI parser, the
screen model, the frame differ and the layout engine. There is no `vt100`,
`vte` or `portable-pty`.

## Usage

```
cargo build --release          # binary: target/release/dtm

dtm                            # attach to the first session (starting the server and a session if needed)
dtm new -s work                # new named session
dtm attach -t work             # attach to an existing session
dtm ls                         # list sessions
dtm reload                     # re-read the config file
dtm kill-server                # end the server and every session
```

The default prefix is **Ctrl-a**. The server exits when its last session ends.

## Default keys (after the prefix)

| Key | Action | Key | Action |
|---|---|---|---|
| `c` | new window | `\|` | split side by side |
| `n` / `p` | next / previous window | `-` | split stacked |
| `1`-`9` | select window | arrows, `h j k l` | move focus |
| `,` | rename window | `H J K L` | resize pane by 5 |
| `&` | kill window | `o` | next pane |
| `x` | kill pane | `z` | zoom pane |
| `S` | new session | `$` | rename session |
| `(` / `)` | previous / next session | `b` | show / hide sidebar |
| `s` | sidebar: windows ↔ sessions | `d` | detach |
| `w` | focus the sidebar | | |
| `[` or `PageUp` | copy mode | `]` | paste |
| `a` | send a literal Ctrl-a | `r` | reload config |

**Resizing**: after `<prefix> H J K L` (5 cells), plain `h j k l` keep
resizing by 1 cell for a second (`options.repeat_time`, `[keys.resize]`), so you
don't have to press the prefix again for every step. Any other key ends it.

**Copy mode** (`k j h l`, `PageUp/Down`, `Ctrl-u/d`, `g`/`G` for top/bottom,
`0`/`$`, `v` to select, `V` to select whole lines, `y` to copy and leave, `q`
to cancel). Copied text is
kept in dtm's paste buffer and also sent to the terminal clipboard (OSC 52).

**Mouse**: click a sidebar entry to switch window or session, click a pane to
focus it, scroll the wheel to enter copy mode (or scroll full-screen apps).
Applications that request mouse reporting receive the events. Hold Shift to
select text with the terminal itself, or set `mouse = false`.

## Sidebar

A column on the left or right (`[sidebar] position`) lists the current
session's windows (`1 vim`, with `●` for activity, `!` for a bell, `Z` when
zoomed). `<prefix> s` toggles it to a list of sessions. Window names come from
`rename-window`, else the pane's title (OSC 0/2), else the foreground process.
It hides itself when the terminal is too narrow.

**The sidebar can take keyboard focus** with `<prefix> w`, or by moving focus
towards it (`<prefix> h` from the leftmost pane, or a root binding such as
`"C-h" = "focus-left"`; the opposite direction leaves it). While focused:
`j`/`k` select, `Enter` opens, `x` kills (after a y/n confirmation), `r`
renames, `J`/`K` reorder, `n` creates a window or session, `Tab` toggles the
list, `q` leaves. All of it is configurable in `[keys.sidebar]`.

Prompts (rename, kill confirmation) use a bar across the whole bottom row.

## Configuration

`~/.config/dtm/config.toml` (or `$DTM_CONFIG`). See
[config.example.toml](config.example.toml) for every option and binding; it is
checked by the test suite to match the built-in defaults.

- `options.shell` is the program started in new panes (default `$SHELL`, then
  `/bin/sh`).
- `[keys.prefix]`, `[keys.root]`, `[keys.copy]`, `[keys.resize]` and
  `[keys.sidebar]` map keys to actions. Set a
  binding to `"none"` to remove a default.
- Key notation: `C-a`, `M-h`, `S-Left`, `C-M-x`, `F5`, `PageUp`, `Space`, or a
  single character.
- Actions: `new-window`, `kill-window`, `next-window`, `prev-window`,
  `select-window N`, `rename-window`, `split-vertical`, `split-horizontal`,
  `focus-left|right|up|down`, `focus-next`, `resize-left|right|up|down [N]`,
  `kill-pane`, `zoom-pane`, `new-session`, `next-session`, `prev-session`,
  `select-session N`, `rename-session`, `toggle-sidebar`,
  `toggle-sidebar-mode`, `copy-mode`, `paste-buffer`, `detach`, `send-prefix`,
  `reload-config`, and in copy mode `copy-up|down|left|right`,
  `copy-page-up|down`, `copy-half-page-up|down`, `copy-top|bottom`,
  `copy-line-start|end`, `copy-select`, `copy-select-line`, `copy-yank`,
  `copy-cancel`; for the sidebar `focus-sidebar` and `sidebar-down|up|open|kill|rename|move-down|move-up|new|leave`.
- A config error is reported by `dtm reload` / `<prefix> r` and the running
  configuration stays active.

## Other environment variables

`DTM_SOCKET` overrides the socket path (default `$XDG_RUNTIME_DIR/dtm/default.sock`,
falling back to `/tmp/dtm-$UID/`). The server logs to
`$XDG_STATE_HOME/dtm/server.log`.

## Design

```
dtm (client) ──unix socket, length-prefixed bincode──► dtm server
 raw mode, stdin → Input                               one actor task owns all state:
 SIGWINCH → Resize                                      sessions → windows → split tree → panes
 Output bytes → stdout                                 PTY reader/writer tasks, per-client connection tasks
```

- The client is a thin relay. The server decodes keys, handles the prefix,
  renders each client's frame and sends only the escape sequences that differ
  from what that client last saw.
- The window is sized to the most recently active client; other clients see
  it cropped or padded.
- Panes are spawned with `openpty` plus `setsid`/`TIOCSCTTY` in a `pre_exec`
  hook, never a raw `fork` in the multithreaded process.

```
src/term/     parser.rs (VT500 state machine), screen.rs (emulator), canvas.rs (frame + differ)
src/server/   layout.rs, render.rs, sidebar.rs, copy.rs, mouse.rs, input.rs, actions.rs
src/          keys.rs, action.rs, config.rs, protocol.rs, pty.rs, client.rs
```

## Tests

```
cargo test
```

Unit tests cover the parser, screen model, differ (diffs are replayed into a
second emulator and compared), key decoding, config, layout, sidebar and copy
mode. `tests/e2e.rs` runs the real binary on a pty and checks what a terminal
would show: attach/detach/reattach, splits, windows, the sidebar, config
reload, remapped keys, multiple clients, copy mode, mouse, and `less`/`vim`
running inside dtm.
