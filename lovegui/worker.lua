--- The thread that talks to the wallet, so the window never stops drawing.
---
--- `balance` and `send` reach a node, and a node takes as long as it takes.
--- Doing that on the main thread freezes everything — the animation stops, the
--- particles hang in the air, and a person reasonably concludes it has crashed.
--- So it happens here instead, and the answer comes back over a channel.
---
--- ## Why this opens its own wallet
---
--- The wallet handle is LuaJIT FFI state and cannot cross a thread boundary, so
--- it is not passed in — this opens its own over the same home directory. That
--- is not a workaround: the store is append-only and every front end already
--- assumes others are reading it, which is what `scripts/parity.sh` checks on
--- every run. Two handles over one home is the supported case.
---
--- Requests arrive as `{id, argv, stdin, yes}` and leave as `{id, envelope}`,
--- both plain tables, because that is all a channel can carry.

local repo, home, network, library = ...

package.path = repo .. "/luacli/?.lua;" .. repo .. "/luacli/?/init.lua;" .. package.path

local causewaybay = require("causewaybay")
local json = require("causewaybay.json")

local requests = love.thread.getChannel("cwb.requests")
local answers = love.thread.getChannel("cwb.answers")

--- Report a failure the same shape a real envelope has, so the model has one
--- code path for "it did not work" rather than two.
local function fail(id, code, message)
  answers:push({ id = id, envelope = { ok = false, error = { code = code, message = message } } })
end

local wallet, open_error = causewaybay.open({
  home = home ~= "" and home or nil,
  network = network ~= "" and network or nil,
  -- Worked out by main.lua and passed across, because a bundle's library sits
  -- outside the sandbox and this thread has no way to look for it.
  lib = library ~= "" and library or nil,
  -- The window asks its own questions and only submits `yes` on a request it
  -- has already confirmed, so the wallet underneath must not assume one.
  yes = false,
})

if not wallet then
  -- Answer every request with the same error rather than dying silently: a
  -- worker that exits leaves the model waiting forever.
  while true do
    local request = requests:demand()
    if request == "quit" then return end
    fail(request.id, open_error.code, open_error.message)
  end
end

while true do
  local request = requests:demand()
  if request == "quit" then break end

  -- A crash in here must not take the thread with it, or the first bad request
  -- silently ends every future one.
  local ok, envelope = pcall(function()
    return wallet:envelope(request.argv, {
      stdin = request.stdin,
      yes = request.yes,
    })
  end)

  if not ok then
    fail(request.id, "internal", tostring(envelope))
  elseif not envelope then
    fail(request.id, "internal", "the wallet returned nothing")
  else
    -- Channels deep-copy plain tables, but re-encoding is cheaper than
    -- trusting that for a structure this nested, and it keeps `json.null`
    -- from arriving as a table the model would have to special-case.
    answers:push({ id = request.id, json = json.encode(envelope) })
  end
end
