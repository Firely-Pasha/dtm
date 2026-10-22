# dtm-navigator

Move between Neovim windows and [dtm](../..) panes with the same keys.
`<C-h>` `<C-j>` `<C-k>` `<C-l>` move inside Neovim first; when there is no window in
that direction they ask dtm to move pane focus (`dtm select-pane`).
`<C-\>` goes to the previous pane. This is a port of the idea behind
vim-tmux-navigator, which only works with tmux.

dtm hands these keys to Neovim only while Neovim is in the pane's foreground, so
nothing else is needed on the dtm side beyond its default `Ctrl-h/j/k/l` bindings.

## Install

lazy.nvim:

```lua
{ dir = "/path/to/dtm/contrib/nvim", name = "dtm-navigator" }
```

Or without a plugin manager, in `init.lua`:

```lua
vim.opt.rtp:append("/path/to/dtm/contrib/nvim")
```

Uses only APIs available since Neovim 0.7 (tested with 0.12). Outside dtm (no `$DTM_PANE`) the commands just move between
Neovim windows.

## Commands

`:DtmNavigateLeft`, `:DtmNavigateDown`, `:DtmNavigateUp`, `:DtmNavigateRight`,
`:DtmNavigatePrevious`.

## Options

Set before or after loading (except `no_mappings`, which must be set first):

| Variable | Default | |
|---|---|---|
| `g:dtm_navigator_no_mappings` | 0 | don't define the default mappings (define your own with the commands) |
| `g:dtm_navigator_save_on_switch` | 0 | 1: `:update` the buffer, 2: `:wall`, before leaving for another pane |
| `g:dtm_navigator_disable_when_zoomed` | 0 | stay in Neovim when the dtm window is zoomed |
| `g:dtm_navigator_disable_netrw_workaround` | 0 | don't make `<C-l>` work in netrw |

`require("dtm_navigator").setup({ save_on_switch = 1 })` sets them from Lua.
