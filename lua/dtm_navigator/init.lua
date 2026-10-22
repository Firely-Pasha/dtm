-- dtm-navigator: move between Neovim windows and dtm panes with the same keys.
--
-- dtm exports DTM_PANE, DTM_SOCKET and DTM_BIN into every pane. When a move
-- would leave Neovim's windows, this asks dtm to move pane focus instead:
--   dtm select-pane -t $DTM_PANE -L|-D|-U|-R|-l
-- Outside dtm (no $DTM_PANE) it only moves between Neovim windows.

local M = {}

local flags = { h = "-L", j = "-D", k = "-U", l = "-R", p = "-l" }

-- True while the last move went to a dtm pane, so "previous" can toggle back.
local last_pane = false

local function option(name, default)
  local v = vim.g["dtm_navigator_" .. name]
  if v == nil then
    return default
  end
  if v == true then
    return 1
  elseif v == false then
    return 0
  end
  return v
end

local function pane()
  local p = vim.env.DTM_PANE
  if p ~= nil and p ~= "" then
    return p
  end
end

--- Run the dtm CLI without a shell. Returns its output and whether it succeeded.
local function dtm(args)
  local bin = vim.env.DTM_BIN
  if bin == nil or bin == "" then
    bin = "dtm"
  end
  local cmd = { bin }
  vim.list_extend(cmd, args)
  local out = vim.fn.system(cmd)
  return out, vim.v.shell_error == 0
end

local function zoomed()
  local out, ok = dtm({ "query", "-t", pane(), "#{window_zoomed_flag}" })
  return ok and vim.trim(out) == "1"
end

--- Move in direction `dir` (h, j, k, l, or p for the previous pane).
function M.navigate(dir)
  local nr = vim.fn.winnr()
  local to_last = dir == "p" and last_pane
  if not to_last then
    local ok, err = pcall(vim.cmd, "wincmd " .. dir)
    if not ok then
      vim.api.nvim_echo({ { err, "ErrorMsg" } }, true, {})
    end
  end
  -- Hand over to dtm when focus did not move inside Neovim (or when toggling
  -- back to the previous dtm pane).
  if pane() and (to_last or nr == vim.fn.winnr()) then
    if option("disable_when_zoomed", 0) == 1 and zoomed() then
      return
    end
    local save = option("save_on_switch", 0)
    if save == 1 then
      pcall(vim.cmd, "update")
    elseif save == 2 then
      pcall(vim.cmd, "wall")
    end
    dtm({ "select-pane", "-t", pane(), flags[dir] })
    last_pane = true
  else
    last_pane = false
  end
end

--- Optional: set options from Lua. `no_mappings` must be set before the plugin
--- loads (vim.g.dtm_navigator_no_mappings = 1), the rest can be set any time.
function M.setup(opts)
  for k, v in pairs(opts or {}) do
    vim.g["dtm_navigator_" .. k] = v
  end
end

return M
