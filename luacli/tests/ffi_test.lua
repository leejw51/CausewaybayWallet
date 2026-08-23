--- Tests for the binding layer: loading the library, and the memory contract.
---
--- Everything here is about the boundary rather than the wallet. If these
--- pass, a string crossing from Rust to Lua is copied before it is freed and
--- freed exactly once, and a library that does not match this binding is
--- refused rather than called.

local t = require("tests.runner")
local binding = require("causewaybay.ffi")
local json = require("causewaybay.json")
local ffi = require("ffi")

t.suite("ffi / discovery", function()
  t.case("names the library the way the platform does", function()
    local name = binding.library_name()
    if ffi.os == "OSX" then
      t.equal(name, "libcausewaybay_ffi.dylib")
    elseif ffi.os == "Windows" then
      t.equal(name, "causewaybay_ffi.dll")
    else
      t.equal(name, "libcausewaybay_ffi.so")
    end
  end)

  t.case("collapses .. so the paths it prints are readable", function()
    t.equal(binding.normalize("/a/b/../c"), "/a/c")
    t.equal(binding.normalize("/a/./b/"), "/a/b")
    t.equal(binding.normalize("a/b/../.."), "")
    -- Nothing to collapse against: the `..` has to stay.
    t.equal(binding.normalize("../x"), "../x")
  end)

  t.case("looks in the cargo output directories", function()
    local paths = binding.search_paths("/checkout/luacli/causewaybay")
    local joined = table.concat(paths, "\n")
    t.contains(joined, "/checkout/rustcli/target/debug/")
    t.contains(joined, "/checkout/rustcli/target/release/")
    t.contains(joined, "/checkout/dist/")
  end)

  t.case("a packaged bundle finds its own library first", function()
    -- `make package` stages the library next to cwbwallet.lua, so a bundle
    -- copied onto another machine is self-contained.
    local paths = binding.search_paths("/anywhere/cwbwallet-lua/causewaybay")
    t.equal(paths[1], "/anywhere/cwbwallet-lua/" .. binding.library_name())
  end)

  t.case("an explicit override beats every guess", function()
    local paths = binding.search_paths("/checkout/luacli/causewaybay", "/exact/lib.so")
    t.equal(paths[1], "/exact/lib.so")
    -- And an empty variable is treated as unset, not as the path "".
    t.equal(binding.search_paths("/checkout/luacli/causewaybay", "")[1],
      "/checkout/luacli/" .. binding.library_name())
  end)

  t.case("prefers what a working tree just built", function()
    -- `make build` produces the debug library, so it is the one that tracks
    -- the source. Release and ./dist come from `make package` and can be
    -- weeks old; either silently answering for freshly changed code is the
    -- failure this ordering exists to prevent.
    local paths = binding.search_paths("/checkout/luacli/causewaybay")
    local at = {}
    for i, path in ipairs(paths) do
      for _, kind in ipairs({ "/release/", "/debug/", "/dist/" }) do
        if path:find(kind, 1, true) then at[kind] = i end
      end
    end
    t.ok(at["/debug/"] < at["/release/"], "debug is what `make build` writes")
    t.ok(at["/release/"] < at["/dist/"], "./dist is the least fresh of the three")
  end)

  t.case("an explicit path is reported when it does not exist", function()
    local lib, err = binding.load("/definitely/not/a/library.dylib")
    t.equal(lib, nil)
    t.contains(err, "cannot find")
    -- The message has to be actionable on its own.
    t.contains(err, "CAUSEWAYBAY_LIB")
  end)
end)

t.suite("ffi / the loaded library", function()
  local lib = assert(binding.load())

  t.case("agrees with this binding on the ABI", function()
    t.equal(lib.cwb_abi_version(), binding.ABI_VERSION)
  end)

  t.case("reports a version", function()
    local version = binding.version(lib)
    t.ok(version:match("^%d+%.%d+%.%d+"), "got " .. tostring(version))
  end)

  t.case("describes itself as JSON", function()
    local described = json.decode(binding.describe(lib))
    t.equal(described.ok, true)
    t.equal(described.data.name, "causewaybay-wallet")
    t.equal(described.data.abi, binding.ABI_VERSION)
    t.ok(#described.data.networks >= 2)
  end)

  t.case("executes a request and returns an envelope", function()
    local reply = binding.execute(lib, '{"argv":["utils","keccak","hello"]}')
    local envelope = json.decode(reply)
    t.equal(envelope.ok, true)
    t.equal(
      envelope.data.keccak256,
      "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"
    )
  end)

  t.case("turns a malformed request into an envelope, not a crash", function()
    local envelope = json.decode(binding.execute(lib, "not json"))
    t.equal(envelope.ok, false)
    t.equal(envelope.error.code, "usage")
  end)

  t.case("survives many calls without leaking", function()
    -- Not a leak *detector* — it is the shape of the bug that would show up
    -- here first: a missing free, or a free before the copy, over 500 strings.
    for _ = 1, 500 do
      local envelope = json.decode(binding.execute(lib, '{"argv":["utils","checksum","0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed"]}'))
      t.equal(envelope.data.address, "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed")
    end
    collectgarbage()
  end)

  t.case("the same library is returned on every load", function()
    t.equal(binding.load(), lib)
  end)
end)

return true
