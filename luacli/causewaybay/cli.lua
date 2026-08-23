--- `cwbwallet-lua` — the Lua command line, over the Rust core's C ABI.
---
--- Deliberately thin. It does not parse the wallet's arguments: it collects
--- argv, adds the two things a library cannot get for itself (piped stdin, and
--- where to find the shared library), and prints what comes back. The command
--- surface is defined once, in Rust, so this CLI cannot drift from `cwbwallet`
--- the way a hand-written second parser would.
---
--- What is left for it to decide is what a *terminal* needs: which stream each
--- thing goes to, what the exit status is, and whether the reply should be
--- rendered as text or as the JSON envelope from SPEC.md.

local causewaybay = require("causewaybay")
local json = require("causewaybay.json")

local M = {}

--- Exit statuses, matching the Rust CLI so scripts can drive either.
M.EXIT_OK = 0
M.EXIT_FAILURE = 1
M.EXIT_USAGE = 2

--- The commands only a terminal front end can run. The core refuses them, but
--- it can only say "not available here"; this can say where to go instead.
local ELSEWHERE = {
  tui = "the terminal UI is only in the Rust CLI — run `cwbwallet tui`, "
    .. "or `cwbwallet-lua interactive` for the menu",
}

--- The one command this front end has that the wallet does not.
---
--- It is a prompt loop rather than a wallet operation, so it never becomes a
--- request: the core would rightly refuse it. It is intercepted here instead,
--- before argv is sent anywhere.
local INTERACTIVE = "interactive"

--- True if `argv` contains `flag` as a whole word.
local function has_flag(argv, flag)
  for _, word in ipairs(argv) do
    if word == flag then return true end
    -- Everything after `--` is a positional, not a flag.
    if word == "--" then return false end
  end
  return false
end

M.has_flag = has_flag

--- The global flags that consume the word after them.
---
--- Only the globals need listing: every other flag comes *after* the
--- subcommand, so by then the answer has already been found. Without this,
--- `--home /tmp/w tui` reads "/tmp/w" as the command.
local VALUE_FLAGS = {
  ["--home"] = true,
  ["--network"] = true,
  ["-n"] = true,
}

--- The first non-flag word: the subcommand, when there is one.
local function first_command(argv)
  local skip = false
  for _, word in ipairs(argv) do
    if skip then
      skip = false
    elseif word == "--" then
      skip = false
    elseif VALUE_FLAGS[word] then
      skip = true
    elseif word:sub(1, 1) ~= "-" then
      return word
    end
  end
  return nil
end

M.first_command = first_command

--- Whether any argument is the lone `-` that means "read it from stdin".
local function wants_stdin(argv)
  for _, word in ipairs(argv) do
    if word == "-" then return true end
  end
  return false
end

M.wants_stdin = wants_stdin

--- The exit status for an error code, matching the Rust CLI.
local function exit_status(code)
  if code == "usage" then return M.EXIT_USAGE end
  return M.EXIT_FAILURE
end

M.exit_status = exit_status

--- Rebuild the SPEC.md envelope from an FFI reply.
---
--- The FFI adds a `human` field that `cwbwallet --json` does not print, so it
--- is dropped here rather than shipped: `--json` output has to be identical
--- across implementations, and parity is checked byte for byte.
local function envelope_line(envelope)
  if envelope.ok then
    return json.encode({ ok = true, data = envelope.data })
  end
  local e = envelope.error or {}
  return json.encode({
    ok = false,
    error = { code = e.code or "internal", message = e.message or "" },
  })
end

M.envelope_line = envelope_line

--- The globals in `argv` that an interactive session should inherit.
---
--- `interactive` never reaches the core, so nothing parses its `--home` and
--- `-n` for us. Rather than growing a second argument parser here, the few
--- globals are read directly: there are four, they are fixed, and getting
--- `--home` wrong would mean opening the wrong wallet.
local function globals_from(argv)
  local found = {}
  local i = 1
  while i <= #argv do
    local word = argv[i]
    local long, inline = word:match("^(%-%-[%w-]+)=(.*)$")
    long = long or word
    local function value()
      if inline then return inline end
      i = i + 1
      return argv[i]
    end
    if long == "--home" then
      found.home = value()
    elseif long == "--network" or long == "-n" then
      found.network = value()
    elseif long == "--yes" or long == "-y" then
      found.yes = true
    end
    i = i + 1
  end
  return found
end

M.globals_from = globals_from

--- Open the wallet an interactive session should drive.
local function session_for(argv)
  local globals = globals_from(argv)
  local session, open_err = causewaybay.open({
    lib = os.getenv("CAUSEWAYBAY_LIB"),
    home = globals.home,
    network = globals.network,
    -- `--yes` is deliberately not inherited. The menu asks its own questions,
    -- and a session that silently answered yes to all of them would be the
    -- worst of both shapes.
    yes = false,
  })
  if not session then return nil, open_err end

  -- Fail now rather than three questions in, if the home or network is bad.
  local ok, err = session:info()
  if not ok then return nil, err end
  return session
end

--- Run the CLI.
---
--- `argv` is the argument list without the program name. `io_streams` lets the
--- tests capture output instead of writing to the terminal; it defaults to the
--- real ones. Returns the exit status rather than calling `os.exit`, so a test
--- can assert on it and a host can embed this.
function M.run(argv, io_streams)
  local out = (io_streams and io_streams.stdout) or io.stdout
  local err = (io_streams and io_streams.stderr) or io.stderr
  local read_stdin = (io_streams and io_streams.read_stdin)
    or function() return io.read("*a") end

  local as_json = has_flag(argv, "--json")

  local command = first_command(argv)
  if command and ELSEWHERE[command] then
    err:write("error [usage]: " .. ELSEWHERE[command] .. "\n")
    return M.EXIT_USAGE
  end

  if command == INTERACTIVE then
    if as_json then
      -- A menu has nobody to read an envelope, and a caller that asked for one
      -- is a script that would hang on the first question.
      err:write("error [usage]: `interactive` is a prompt; it has no --json form\n")
      return M.EXIT_USAGE
    end
    local session, session_err = session_for(argv)
    if not session then
      err:write(("error [%s]: %s\n"):format(session_err.code, session_err.message))
      return exit_status(session_err.code)
    end
    return require("causewaybay.interactive").run(session, io_streams)
  end

  local wallet, open_err = causewaybay.open({ lib = os.getenv("CAUSEWAYBAY_LIB") })
  if not wallet then
    -- A missing library is not a wallet error, and printing it as an envelope
    -- would suggest the wallet ran and declined. It did not run at all.
    err:write("error [" .. open_err.code .. "]: " .. open_err.message .. "\n")
    return M.EXIT_FAILURE
  end

  local options = {}
  if wants_stdin(argv) then options.stdin = read_stdin() end

  local envelope, call_err = wallet:envelope(argv, options)
  if not envelope then
    err:write("error [" .. call_err.code .. "]: " .. call_err.message .. "\n")
    return M.EXIT_FAILURE
  end

  if as_json then
    -- One envelope on stdout, which stays the single machine-readable channel.
    out:write(envelope_line(envelope) .. "\n")
    return envelope.ok and M.EXIT_OK or exit_status((envelope.error or {}).code)
  end

  if not envelope.ok then
    local e = envelope.error or {}
    err:write(("error [%s]: %s\n"):format(e.code or "internal", e.message or ""))
    return exit_status(e.code)
  end

  local asked_for_help = has_flag(argv, "--help") or has_flag(argv, "-h")

  -- The banner goes to stderr so `cwbwallet-lua account list > file` is clean,
  -- and not at all for --help/--version, which the Rust CLI answers before it
  -- has a wallet to warn about.
  if not (asked_for_help or has_flag(argv, "--version") or has_flag(argv, "-V")) then
    local described = wallet:describe()
    if described and described.warning then err:write(described.warning .. "\n") end
  end

  out:write((envelope.human or "") .. "\n")

  -- The help text comes from the core, which does not know about the one
  -- command this front end adds. Rather than rewrite its help, say the extra
  -- part afterwards — someone reading `--help` should not have to find out
  -- about `interactive` from the README.
  if asked_for_help and first_command(argv) == nil then
    out:write("\nOnly in this front end:\n  " .. INTERACTIVE
      .. "     Create, list, export and send from a menu\n")
  end
  return M.EXIT_OK
end

return M
