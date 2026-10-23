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
dtm select-pane -R             # move pane focus (used by editor plugins, see below)
dtm kill-server                # end the server and every session
```

The default prefix is **Ctrl-a**. The server exits when its last session ends.

## Default keys (after the prefix)

| Key | Action | Key | Action |
|---|---|---|---|
| `c` | new window | `\|` | split side by side |
| `n` / `p` | next / previous window | `-` | split stacked |
| `1`-`9` | select window | arrows, `Ctrl-h/j/k/l` | move focus |
| `,` | rename window | `h j k l` (or `H J K L`) | resize pane by 5 |
| `&` | kill window | `o` | next pane |
| `x` | kill pane | `z` | zoom pane |
| `S` | new session | `$` | rename session |
| `(` / `)` | previous / next session | `b` | show / hide sidebar |
| `s` | sidebar: windows ↔ sessions | `d` | detach |
| `w` | focus the sidebar | `;` | previously focused pane |
| `Ctrl-l` | send a literal Ctrl-l | `[` or `PageUp` | copy mode |
| `]` | paste | `a` | send a literal Ctrl-a |
| `r` | reload config | `R` | respawn pane (asks) |
| `Alt-r` | respawn window (asks) | `f` or `/` | search windows |
| `Tab` | previously opened window (then `Tab` / `Shift-Tab` repeat) | `m` | maximize / restore |
| `P` | popup shell (toggle) | `D` | show / hide the dock |
| `{` / `}` | previous / next dock pane | `A` / `X` | add / close a dock pane |

Without the prefix: **`Ctrl-h` `Ctrl-j` `Ctrl-k` `Ctrl-l`** move between panes
(see below).

**Maximize**: `<prefix> m` makes the focused thing fill the whole terminal: a window pane, a dock pane, the popup, or the sidebar (when it has focus). The pane is resized to the full screen, so lazygit or vim redraw at full size. Press it again to restore. Moving focus to something else (or hiding the popup or dock) restores it too. Unlike `z`, which zooms inside the pane area, this hides the sidebar and dock as well.

**Recent windows**: `<prefix> Tab` toggles to the previously opened window; for
the repeat time after it (`options.repeat_time`, 1 second) plain `Tab` goes
further back, like Alt-Tab, and `Shift-Tab` goes the other way. Any other key
ends it, and after that `Tab` is the shell's again. A window counts as
opened when you choose it (`Enter`/`o` in the sidebar, a click, `<prefix> 1`-`9`,
`n`/`p`, search), not while the sidebar selection merely passes over it.
`Ctrl-Tab` / `Ctrl-Shift-Tab` do the same without the prefix, but terminals only
send them when asked to, and most use them for their own tabs: make yours send `ESC [ 9 ; 5 u` (and `ESC [ 9 ; 6 u` for `Ctrl-Shift-Tab`), e.g.
Ghostty `keybind = ctrl+tab=csi:9;5u`, kitty `map ctrl+tab send_text all \x1b[9;5u`,
WezTerm `{ key = "Tab", mods = "CTRL", action = act.SendString "\x1b[9;5u" }`, or
an iTerm2 key mapping "Send Escape Sequence" `[9;5u`. xterm's
`modifyOtherKeys` form (`ESC [ 27 ; 5 ; 9 ~`) works too.

**Switching windows** focuses the top-left pane of the window
(`options.window_focus = "last"` keeps the pane that was focused there).

**Resizing**: while you resize (and for the repeat window after it), the middle of the pane shows its size as `columns×rows`, and the sidebar or dock shows its width, so panes in different windows can be lined up.

**Resizing**: while the sidebar or the dock has focus, `<prefix> h` / `l` move its inner edge to resize it, and then plain `h` / `l` keep resizing by one cell for a second, as for panes. Both widths belong to the session (every client and later attach sees them); the dock's is also exported. Otherwise `<prefix> h j k l` resizes the focused pane by 5 cells, then plain `h j k l` keep
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

## Navigating with Ctrl-h/j/k/l

`Ctrl-h/j/k/l` move focus left/down/up/right, continuing into the sidebar at the
edge (and back out again). They are root bindings, so change them in
`[keys.root]` (`navigate-left|right|up|down`, `navigate-last`), or remove one with
`"C-h" = "none"`. Because they replace the shell's own `Ctrl-l` (clear screen) and
friends, `<prefix> Ctrl-l` sends a real `Ctrl-l`; `send-key` does that for any
binding. (A terminal that sends `Ctrl-h` for Backspace will lose Backspace; most
send `0x7f`.)

If an editor is in the foreground of the active pane, dtm hands the key to it
instead. That is the same idea as vim-tmux-navigator: the editor moves between its
own windows and, when it is already at the edge, asks dtm to move pane focus. The
editor is recognised by process name: `vim`, `nvim`, `view`, `vimdiff`, `fzf`
(including Nix's `.nvim-wrapped`), plus anything listed in
`options.navigator_apps`.

### Neovim plugin

This repository is also the Neovim plugin: `plugin/` and `lua/dtm_navigator/` at the
root provide the same mappings, commands and options as vim-tmux-navigator, talking
to dtm instead of tmux (vim-tmux-navigator itself only works with tmux). With
lazy.nvim:

```lua
{ "Firely-Pasha/dtm", name = "dtm-navigator" }
```

Outside dtm (no `$DTM_PANE`) it only moves between Neovim windows. If you also use
tmux, load it only inside dtm, and vim-tmux-navigator elsewhere, since both map the
same keys:

```lua
local in_dtm = (vim.env.DTM_PANE or "") ~= ""
return {
  { "christoomey/vim-tmux-navigator", cond = not in_dtm },
  { "Firely-Pasha/dtm", name = "dtm-navigator", cond = in_dtm },
}
```

Without a plugin manager, clone the repository and `set rtp+=/path/to/dtm`.
Neovim only (classic Vim is not supported). It uses APIs available since Neovim 0.7
and is tested with 0.12.

Commands: `:DtmNavigateLeft`, `:DtmNavigateDown`, `:DtmNavigateUp`,
`:DtmNavigateRight`, `:DtmNavigatePrevious` (mapped to `<C-h/j/k/l>` and `<C-\>`).
Options, set before or after loading (`no_mappings` must be set first):

| Variable | Default | |
|---|---|---|
| `g:dtm_navigator_no_mappings` | 0 | don't define the default mappings (use the commands) |
| `g:dtm_navigator_save_on_switch` | 0 | 1: `:update` the buffer, 2: `:wall`, before leaving for another pane |
| `g:dtm_navigator_disable_when_zoomed` | 0 | stay in Neovim when the dtm window is zoomed |
| `g:dtm_navigator_disable_netrw_workaround` | 0 | don't make `<C-l>` work in netrw |

`require("dtm_navigator").setup({ save_on_switch = 1 })` sets them from Lua.

### What dtm exports to panes

Every pane gets `DTM_PANE` (its id), `DTM_SOCKET` (so the `dtm` CLI reaches the
right server) and `DTM_BIN` (the dtm executable). Two commands use them:

```
dtm select-pane [-t PANE] -L|-R|-U|-D|-l    # move focus (-l: previously focused pane)
dtm query [-t PANE] '#{window_zoomed_flag}'  # also pane_at_left/right/top/bottom, pane_id, pane_active
```

## Projects

A project file describes a session: its windows, how each window is split,
where each pane starts and what it runs. See
[`examples/project.dtm.yaml`](examples/project.dtm.yaml).

```yaml
name: app                 # session name (default: the directory's name)
root: ~/code/app          # default: the directory of the file
lazy: true                # default: a window starts when first shown
windows:
  - name: edit
    focus: true           # shown first
    layout:
      split: columns      # columns = side by side, rows = stacked
      panes:
        - command: nvim
          size: 65%       # N% or cells; the rest is shared equally
          focus: true
        - split: rows
          panes:
            - command: cargo watch -x check
            - path: scripts
  - name: server          # one pane: path/command straight on the window
    command: cargo run
    lazy: false
```

- A `path` is relative to the nearest `path` above it, then to `root`; `~` and
  absolute paths work. A pane's `command` is typed into its shell, so you are
  left at a prompt when it ends. Unknown keys and mistakes (a split with one
  pane, a bad size) are reported with their position before anything starts.
- **`sidebar`** (top level) sets this project's sidebar: `width` (4-200) and
  `visible`. Exporting writes the width you resized the sidebar to; a
  `visible` you wrote is kept.
- **`pane_command`** (top level) is typed into every pane's shell before the
  pane's own command, e.g. `pane_command: source .venv/bin/activate`. It also
  applies to panes you create later in the session (splits, new windows, the
  popup, dock panes) and to respawned ones.
- **Lazy start.** Only the first window is created when the project opens; the
  others get their panes and run their commands the first time you show them
  (a `…` marks them in the sidebar). Set `lazy: false` on the file or on a
  window to start it at once.
- **Opening.** `dtm` in a directory with a `project.dtm.yaml` opens it.
  `dtm open [FILE|DIR|NAME]` takes a file, a directory holding one, or a saved
  project. If a session with the project's name is already running, dtm attaches
  to it and starts nothing again.
- **Saved projects** live in `~/.config/dtm/projects/NAME.yaml` (next to the
  config file): `dtm project ls`, `dtm project edit NAME` (opens `$VISUAL` /
  `$EDITOR`, creating the file from a template, and checks it afterwards) and
  `dtm project rm NAME`. A saved project has no directory of its own, so give
  it a `root:`.
- **Export.** `dtm export [FILE|DIR|NAME] [-t SESSION]` writes a running session
  as a project: the splits and their sizes, each pane's directory and, when a
  program other than the shell is running, its command line. With no argument
  it updates the file the session was opened from, or creates
  `project.dtm.yaml` in the session's directory. Paths are written relative to
  the file. A file without a `root` is written without one; a file that has
  one keeps it, and the paths are then written relative to it. A saved project
  is updated in place, never exported to its root. When updating, your `name`,
  `root`, `lazy`, `pane_command` and dock size are kept, but comments are not.
  `e` in the focused sidebar does the same after asking (it shows the file it
  will write); the unbound `export-project` action writes at once.
- **Respawn.** `<prefix> R` restarts the focused pane the way it started (same
  directory and command, a new process) and `<prefix> Alt-r` restarts the whole
  window from its project layout, closing panes you added since. Both ask first.
  In the sidebar, `R` respawns the selected window.

## The dock

A second side panel, opposite the sidebar, holds panes that belong to the
whole session instead of one window: lazygit, a claude session, a log tail. It
shows one pane at a time under a row of tabs.

```yaml
dock:
  width: 35%          # N% of the terminal or cells (default 35%)
  visible: true       # shown when the session opens (default: hidden)
  panes:
    - name: git       # default: the command's first word
      command: lazygit
    - command: claude
      path: ..        # like window panes: relative to root
```

Dock panes start the first time they are shown. `<prefix> D` shows or hides
the dock, `{` and `}` cycle its panes, `A` asks for a command and adds a pane
running it (empty: a shell), `X` closes the current one (asks first), and the
`dock-rename` / `focus-dock` actions are unbound. `Ctrl-l` from the rightmost
window pane (or any focus move towards the dock, including the Neovim plugin's
at the edge of Neovim) moves into it and the opposite
direction comes back; clicks on tabs and panes work too. `<prefix> x` closes the
focused dock pane without asking. The dock is exported with the project.

## Popup

`<prefix> P` opens a floating shell in the current directory (`[popup] width`
and `height` size it) and `<prefix> P` again hides it; it keeps running while
hidden. While it is shown it has the keyboard, and `<prefix> x` (or exiting the
shell) closes it.

## Searching windows

`<prefix> f`, or `/` in the focused sidebar, filters the windows by typing
(fuzzy: `srv` finds `2 server`). `Up`/`Down` (or `Ctrl-n`/`Ctrl-p`) move the
selection, which shows that window as you go; `Enter` opens it and `Esc` goes
back to where you were.

## Sidebar

A column on the left or right (`[sidebar] position`) lists the current
session's windows (`1 vim`, with `●` for activity, `!` for a bell, `Z` when
zoomed). `<prefix> s` toggles it to a list of sessions. Window names come from
`rename-window`, else the pane's title (OSC 0/2), else the foreground process.
It hides itself when the terminal is too narrow.

**The sidebar can take keyboard focus** with `<prefix> w`, or by moving focus
towards it (`<prefix> Left` from the leftmost pane, or a root binding such as
`"C-h" = "focus-left"`; the opposite direction leaves it). While focused:
`j`/`k` select (and show) the window or session as the highlight moves
(`sidebar.follow`, on by default), `Enter` (or `o`) opens it and leaves, `q` leaves, `x` kills (after a y/n confirmation), `r`
renames, `J`/`K` reorder, `n` creates a window or session, `Tab` toggles the
list, `q` leaves. All of it is configurable in `[keys.sidebar]`.

Prompts (rename, confirmations, the dock command) are a box in the middle of the screen: type and press Enter, or Esc to cancel; confirmations take `y` and cancel on any other key.

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
  `select-window N`, `toggle-maximize`, `recent-window`, `recent-window-reverse`, `rename-window`, `split-vertical`, `split-horizontal`,
  `focus-left|right|up|down`, `focus-next`, `resize-left|right|up|down [N]`,
  `kill-pane`, `zoom-pane`, `new-session`, `next-session`, `prev-session`,
  `select-session N`, `rename-session`, `toggle-sidebar`,
  `toggle-sidebar-mode`, `copy-mode`, `paste-buffer`, `detach`, `send-prefix`,
  `reload-config`, `respawn-pane`, `respawn-window`, `export-project`, `search-windows`, `toggle-popup`, `toggle-dock`, `focus-dock`, `dock-next|prev|add|close|rename`, `focus-last`, `navigate-left|right|up|down`, `navigate-last`,
  `send-key` (sends the key that triggered the binding), and in copy mode `copy-up|down|left|right`,
  `copy-page-up|down`, `copy-half-page-up|down`, `copy-top|bottom`,
  `copy-line-start|end`, `copy-select`, `copy-select-line`, `copy-yank`,
  `copy-cancel`; for the sidebar `focus-sidebar` and `sidebar-down|up|open|kill|rename|move-down|move-up|new|respawn|export|search|leave`.
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
src/          keys.rs, action.rs, config.rs, protocol.rs, pty.rs, client.rs, procinfo.rs
plugin/, lua/   Neovim plugin for Ctrl-h/j/k/l navigation
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
