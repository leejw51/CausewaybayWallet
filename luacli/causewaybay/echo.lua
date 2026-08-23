--- Turning terminal echo off, so a mnemonic is not typed in plain sight.
---
--- A seed phrase read at a prompt is a password: it should not appear on the
--- screen, and it should not survive in the scrollback of a shared terminal or
--- a recorded session.
---
--- ## Why `stty` and not termios through the ffi
---
--- The direct route is `tcgetattr`/`tcsetattr` and clearing `ECHO` in
--- `c_lflag`. It needs `struct termios`, whose layout is *not* the same on
--- macOS and Linux — `tcflag_t` is 8 bytes on one and 4 on the other, and
--- `NCCS` is 20 against 32. Guessing wrong there does not raise an error; it
--- writes plausible garbage into the terminal's settings and leaves the shell
--- unusable after the program exits. `stty` is POSIX, is already correct on
--- every platform it runs on, and costs one fork per prompt — which, for
--- something a person is about to type a sentence into, is free.
---
--- Where there is no `stty` — Windows, mainly — echo stays on and the caller is
--- told so, because a prompt that silently fails to hide what it promised to
--- hide is worse than one that admits it.

local ffi = require("ffi")

ffi.cdef([[ int isatty(int fd); ]])

local echo = {}

--- File descriptor 0. Named because `isatty(0)` reads as a riddle.
local STDIN = 0

--- True when standard input is a terminal.
---
--- A pipe echoes nothing to begin with, so there is nothing to hide and no
--- reason to run `stty` — which would fail against a pipe anyway.
function echo.is_tty()
  local ok, result = pcall(function() return ffi.C.isatty(STDIN) end)
  return ok and result == 1
end

--- Run `stty <mode>`, reporting whether it worked.
---
--- LuaJIT is Lua 5.1, where `os.execute` returns the exit status as a number;
--- 5.2 and later return a boolean. Both are accepted so this keeps working if
--- the interpreter underneath ever changes.
local function stty(mode)
  if not echo.is_tty() then return false end
  local result = os.execute("stty " .. mode .. " 2>/dev/null")
  return result == 0 or result == true
end

--- Stop the terminal echoing what is typed. Returns true if it took effect.
function echo.off()
  return stty("-echo")
end

--- Start it echoing again. Returns true if it took effect.
function echo.on()
  return stty("echo")
end

--- Whether a prompt can actually hide what is typed into it.
function echo.available()
  return echo.is_tty()
end

--- Read one line with echo off, restoring it afterwards no matter what.
---
--- `read_line` is the reader to use and `write` is where the newline goes —
--- the terminal did not echo the one the person pressed, so without it the
--- next thing printed lands on the same line.
---
--- Restoring is the whole point of the function: an error thrown while echo is
--- off would otherwise leave the shell silently swallowing everything the
--- person types next.
function echo.read_hidden(read_line, write)
  local hidden = echo.off()
  local ok, line = pcall(read_line)
  if hidden then
    echo.on()
    if write then write("\n") end
  end
  if not ok then error(line, 0) end
  return line
end

return echo
