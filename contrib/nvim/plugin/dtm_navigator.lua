-- dtm-navigator: seamless <C-h/j/k/l> navigation between Neovim windows and dtm panes.
if vim.g.loaded_dtm_navigator then
  return
end
vim.g.loaded_dtm_navigator = 1

local nav = require("dtm_navigator")

local commands = { Left = "h", Down = "j", Up = "k", Right = "l", Previous = "p" }
for name, dir in pairs(commands) do
  vim.api.nvim_create_user_command("DtmNavigate" .. name, function()
    nav.navigate(dir)
  end, { desc = "Move to the " .. name:lower() .. " Neovim window or dtm pane" })
end

if vim.g.dtm_navigator_no_mappings ~= 1 and vim.g.dtm_navigator_no_mappings ~= true then
  local keys = { ["<C-h>"] = "h", ["<C-j>"] = "j", ["<C-k>"] = "k", ["<C-l>"] = "l" }
  for lhs, dir in pairs(keys) do
    vim.keymap.set("n", lhs, function()
      nav.navigate(dir)
    end, { silent = true, desc = "dtm navigate" })
    -- In a terminal buffer: leave terminal mode first, except inside fzf.
    vim.keymap.set("t", lhs, function()
      if vim.bo.filetype == "fzf" then
        return vim.api.nvim_replace_termcodes(lhs, true, false, true)
      end
      vim.schedule(function()
        nav.navigate(dir)
      end)
      return vim.api.nvim_replace_termcodes("<C-\\><C-n>", true, false, true)
    end, { expr = true, silent = true, desc = "dtm navigate" })
  end
  vim.keymap.set("n", "<C-\\>", function()
    nav.navigate("p")
  end, { silent = true, desc = "dtm navigate to the previous pane" })
end

-- netrw maps <C-l> to refresh; keep navigation working there.
if vim.g.dtm_navigator_disable_netrw_workaround ~= 1 then
  local maps = vim.g.Netrw_UserMaps or {}
  table.insert(maps, { "<C-l>", "<C-U>DtmNavigateRight<cr>" })
  vim.g.Netrw_UserMaps = maps
end
