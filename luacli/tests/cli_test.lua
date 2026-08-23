--- Tests for the Lua CLI: streams, exit statuses, and the SPEC.md envelope.
---
--- `cli.run` returns its status instead of calling `os.exit`, and takes its
--- streams as an argument, so all of this runs in-process — no subprocess, no
--- temp files to parse, and a failure points at a line rather than at a shell.

local t = require("tests.runner")
local support = require("tests.support")
local cli = require("causewaybay.cli")
local json = require("causewaybay.json")

--- Run the CLI against a throwaway home, returning status, stdout, stderr.
local function run(argv, options)
  options = options or {}
  local home = options.home or support.temp_home()
  local streams, out, err = support.streams(options.stdin)

  local all = { "--home", home }
  for _, word in ipairs(argv) do all[#all + 1] = word end

  local status = cli.run(all, streams)
  return status, out:text(), err:text(), home
end

t.suite("cli / argument inspection", function()
  t.case("finds a flag as a whole word", function()
    t.equal(cli.has_flag({ "account", "--json" }, "--json"), true)
    t.equal(cli.has_flag({ "account" }, "--json"), false)
    -- Not a prefix match: `--json-lines` is a different flag.
    t.equal(cli.has_flag({ "--json-lines" }, "--json"), false)
    -- Everything after `--` is a positional.
    t.equal(cli.has_flag({ "sign", "--", "--json" }, "--json"), false)
  end)

  t.case("finds the subcommand", function()
    t.equal(cli.first_command({ "--json", "account", "list" }), "account")
    t.equal(cli.first_command({ "--json" }), nil)
    -- A global flag's value is not the subcommand, however much it looks
    -- like a bare word.
    t.equal(cli.first_command({ "--home", "/tmp/w", "tui" }), "tui")
    t.equal(cli.first_command({ "-n", "mainnet", "balance" }), "balance")
    -- The `=` form carries its value inline, so nothing is skipped after it.
    t.equal(cli.first_command({ "--home=/tmp/w", "info" }), "info")
  end)

  t.case("notices the lone dash that means stdin", function()
    t.equal(cli.wants_stdin({ "sign", "-" }), true)
    t.equal(cli.wants_stdin({ "sign", "-x" }), false)
    t.equal(cli.wants_stdin({ "sign", "hello" }), false)
  end)

  t.case("maps codes to the Rust CLI's exit statuses", function()
    t.equal(cli.exit_status("usage"), 2)
    t.equal(cli.exit_status("account_not_found"), 1)
    t.equal(cli.exit_status("rpc_error"), 1)
  end)
end)

t.suite("cli / human output", function()
  t.case("prints the result on stdout and the warning on stderr", function()
    local status, out, err = run({ "info" })
    t.equal(status, 0)
    t.contains(out, "Network")
    -- The banner must never land on stdout, or `… > file` captures it.
    t.contains(err, "unencrypted")
    t.equal(out:find("unencrypted"), nil)
  end)

  t.case("reports a failure on stderr with its code", function()
    local status, out, err = run({ "account", "show", "ghost" })
    t.equal(status, 1)
    t.equal(out, "")
    t.contains(err, "[account_not_found]")
    t.contains(err, "ghost")
  end)

  t.case("an unknown command exits 2", function()
    local status, _, err = run({ "teleport" })
    t.equal(status, 2)
    t.contains(err, "[usage]")
  end)

  t.case("--help succeeds and describes the commands", function()
    local status, out = run({ "--help" })
    t.equal(status, 0)
    t.contains(out, "Usage:")
    t.contains(out, "account")
  end)

  t.case("sends the terminal UI to the Rust CLI", function()
    -- Refusing is right; refusing without saying where to go is not.
    local status, _, err = run({ "tui" })
    t.equal(status, 2)
    t.contains(err, "cwbwallet tui")
    -- And points at the menu this front end does have.
    t.contains(err, "interactive")
  end)

  t.case("a real session refuses a confirmation until asked", function()
    -- `--yes` is not inherited into a session, so the REPL is the thing that
    -- asks. A session that had pre-answered would be the worst of both.
    local home = support.temp_home()
    local streams, out = support.streams()
    local answers, i = { "account new -l doomed", "account remove doomed", "n", "q" }, 0
    streams.read_line = function()
      i = i + 1
      return answers[i]
    end

    t.equal(cli.run({ "--home", home, "--yes", "interactive" }, streams), 0)
    t.contains(out:text(), "cancelled.")
  end)

  t.case("--help mentions the command only this front end has", function()
    local _, out = run({ "--help" })
    t.contains(out, "Only in this front end")
    t.contains(out, "interactive")
  end)

  t.case("a subcommand's help is left exactly as the core wrote it", function()
    local _, out = run({ "account", "--help" })
    t.equal(out:find("Only in this front end"), nil)
  end)
end)

t.suite("cli / the JSON envelope", function()
  t.case("emits exactly the SPEC.md success envelope", function()
    local status, out = run({ "--json", "account", "new", "--label", "alpha" })
    t.equal(status, 0)
    t.equal(select(2, out:gsub("\n", "")), 1, "one line, one newline")

    local envelope = json.decode(out)
    t.equal(envelope.ok, true)
    t.equal(envelope.data.label, "alpha")
    -- `human` is an FFI convenience; it must not leak into `--json` output,
    -- which is compared against the Rust CLI byte for byte.
    t.equal(envelope.human, nil)
    local keys = 0
    for _ in pairs(envelope) do keys = keys + 1 end
    t.equal(keys, 2)
  end)

  t.case("emits exactly the SPEC.md error envelope", function()
    local status, out, err = run({ "--json", "account", "show", "ghost" })
    t.equal(status, 1)
    -- In --json mode stdout stays the single channel, errors included.
    t.equal(err, "")
    local envelope = json.decode(out)
    t.equal(envelope.ok, false)
    t.equal(envelope.error.code, "account_not_found")
    t.ok(#envelope.error.message > 0)
  end)

  t.case("keeps state between invocations", function()
    local _, _, _, home = run({ "--json", "account", "new", "--label", "alpha" })
    local status, out = run({ "--json", "account", "list" }, { home = home })
    t.equal(status, 0)
    local accounts = json.decode(out).data
    t.equal(#accounts, 1)
    t.equal(accounts[1].label, "alpha")
  end)

  t.case("refuses a destructive command without --yes", function()
    local _, _, _, home = run({ "--json", "account", "new", "--label", "doomed" })
    local status, out = run({ "--json", "account", "remove", "doomed" }, { home = home })
    t.equal(status, 1)
    t.equal(json.decode(out).error.code, "confirmation_required")

    local ok_status = run({ "--json", "--yes", "account", "remove", "doomed" }, { home = home })
    t.equal(ok_status, 0)
  end)
end)

t.suite("cli / interactive", function()
  t.case("reads the globals a session should inherit", function()
    local globals = cli.globals_from({ "--home", "/tmp/w", "-n", "mainnet", "interactive" })
    t.equal(globals.home, "/tmp/w")
    t.equal(globals.network, "mainnet")
    -- `--yes` is read but deliberately not applied; see session_for.
    t.equal(cli.globals_from({ "-y", "interactive" }).yes, true)
    t.equal(cli.globals_from({ "interactive" }).yes, nil)
  end)

  t.case("accepts the --flag=value form", function()
    local globals = cli.globals_from({ "--home=/tmp/w", "--network=testnet" })
    t.equal(globals.home, "/tmp/w")
    t.equal(globals.network, "testnet")
  end)

  t.case("has no --json form", function()
    -- A script that asked for an envelope would hang on the first question.
    local status, out, err = run({ "--json", "interactive" })
    t.equal(status, 2)
    t.equal(out, "")
    t.contains(err, "no --json form")
  end)

  t.case("runs the menu against the home the arguments named", function()
    local home = support.temp_home()
    local streams, out = support.streams()
    -- One answer, then end of input, so the session opens and closes.
    local answers, next_answer = { "q" }, 0
    streams.read_line = function()
      next_answer = next_answer + 1
      return answers[next_answer]
    end

    local status = cli.run({ "--home", home, "interactive" }, streams)
    t.equal(status, 0)
    t.contains(out:text(), "Causewaybay Wallet — interactive")
    t.contains(out:text(), home)
  end)

  t.case("reports a bad network before asking anything", function()
    local streams, _, err = support.streams()
    streams.read_line = function() error("the session should not have started", 0) end
    local status = cli.run(
      { "--home", support.temp_home(), "-n", "ethereum", "interactive" }, streams)
    t.equal(status, 1)
    t.contains(err:text(), "[unknown_network]")
  end)
end)

t.suite("cli / standard input", function()
  t.case("a dash reads the piped text", function()
    local status, out = run(
      { "--json", "account", "import-mnemonic", "-m", "-", "--label", "seeded" },
      { stdin = support.MNEMONIC .. "\n" }
    )
    t.equal(status, 0)
    t.equal(json.decode(out).data.address, support.ADDRESS_0)
  end)

  t.case("stdin is only read when something asks for it", function()
    -- support.streams raises if read_stdin is called unexpectedly, so this
    -- passing is the assertion: no command hangs waiting on a pipe.
    local status = run({ "--json", "info" })
    t.equal(status, 0)
  end)
end)

t.suite("cli / envelope rendering", function()
  t.case("drops the human field and keeps key order", function()
    local rendered = cli.envelope_line({ ok = true, data = { b = 2, a = 1 }, human = "text" })
    t.equal(rendered, '{"data":{"a":1,"b":2},"ok":true}')
  end)

  t.case("fills in a missing error body rather than emitting null", function()
    local rendered = cli.envelope_line({ ok = false })
    local envelope = json.decode(rendered)
    t.equal(envelope.ok, false)
    t.equal(envelope.error.code, "internal")
  end)
end)

return true
