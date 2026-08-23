--- The LuaJIT binding to `libcausewaybay_ffi`.
---
--- Everything unsafe about the C ABI is confined to this file: finding the
--- shared library, declaring its six functions, and making sure every string
--- it hands back is freed exactly once. Above this line the wallet is Lua
--- tables and nothing else.
---
--- Requires LuaJIT (LÖVE has it built in). Plain Lua 5.x has no `ffi`.

local ffi = require("ffi")

local M = {}

--- The ABI this binding was written against. A library reporting anything else
--- is refused rather than guessed at: the envelope shape is the whole contract.
M.ABI_VERSION = 1

-- Kept byte-identical to rustcli/ffi/include/causewaybay.h.
ffi.cdef([[
int   cwb_abi_version(void);
char *cwb_version(void);
char *cwb_describe(void);
char *cwb_commands(void);
char *cwb_execute(const char *request_json);
void  cwb_string_free(char *s);
]])

--- The platform's name for a Rust cdylib.
local function library_name()
  if ffi.os == "Windows" then return "causewaybay_ffi.dll" end
  if ffi.os == "OSX" then return "libcausewaybay_ffi.dylib" end
  return "libcausewaybay_ffi.so"
end

M.library_name = library_name

--- Collapse `a/b/../c` into `a/c`, purely textually.
---
--- Not a substitute for `realpath` — it does not resolve symlinks and does not
--- touch the disk. It exists so the "could not find it" message names
--- directories a person can read, which is the whole value of that message.
local function normalize(path)
  local absolute = path:sub(1, 1) == "/"
  local parts = {}
  for segment in path:gmatch("[^/]+") do
    if segment == ".." and #parts > 0 and parts[#parts] ~= ".." then
      parts[#parts] = nil
    elseif segment ~= "." then
      parts[#parts + 1] = segment
    end
  end
  return (absolute and "/" or "") .. table.concat(parts, "/")
end

M.normalize = normalize

--- Where to look, in order, for the shared library.
---
--- `root` is the directory holding this file, so everything is relative to the
--- checkout rather than to whatever directory the user is standing in.
--- `override` is `$CAUSEWAYBAY_LIB` — passed in rather than read here, so this
--- function is a pure list and a test can ask what the order *is* without the
--- environment it happens to run in changing the answer.
local function search_paths(root, override)
  local name = library_name()
  local bundle = normalize(root .. "/..") -- causewaybay/ -> alongside cwbwallet.lua
  local repo = normalize(root .. "/../..") -- luacli/ -> the checkout
  local paths = {}
  -- An exact path beats every guess: it is how a packaged layout, an unusual
  -- install and a test run all say "this one".
  if override and override ~= "" then paths[#paths + 1] = override end
  -- A packaged copy carries its library next to the entry point, so a bundle
  -- that was moved anywhere still finds its own and not a stale build.
  paths[#paths + 1] = bundle .. "/" .. name
  -- Then a checkout, freshest first. `make build` produces the debug library,
  -- so that is the one a working tree has just rebuilt; release and ./dist come
  -- only from `make package`, which stages a bundle that wins above anyway.
  -- Getting this order backwards means a library from last week's release
  -- silently answering for the code you just changed.
  paths[#paths + 1] = repo .. "/rustcli/target/debug/" .. name
  paths[#paths + 1] = repo .. "/rustcli/target/release/" .. name
  paths[#paths + 1] = repo .. "/dist/" .. name
  -- Last resort: whatever the system linker can find on its own.
  paths[#paths + 1] = "causewaybay_ffi"
  return paths
end

M.search_paths = search_paths

--- The directory holding this module, so the search is relative to the
--- checkout rather than to whatever directory the user happened to be in.
local function module_root()
  local source = debug.getinfo(1, "S").source
  local path = source:match("^@(.*)/[^/]*$")
  return path or "."
end

M.module_root = module_root

local loaded = nil

--- Load the shared library, once per process.
---
--- Returns the FFI namespace, or `nil, message` listing everywhere it looked —
--- a missing library is the single most likely thing to go wrong here, so the
--- message has to be enough to fix it without reading this file.
function M.load(explicit)
  if loaded and not explicit then return loaded end

  local paths = explicit and { explicit }
    or search_paths(module_root(), os.getenv("CAUSEWAYBAY_LIB"))
  local tried = {}
  for _, path in ipairs(paths) do
    local ok, lib = pcall(ffi.load, path)
    if ok then
      local reported = lib.cwb_abi_version()
      if reported ~= M.ABI_VERSION then
        return nil,
          ("%s speaks ABI %d, this binding expects %d — rebuild both"):format(
            path, reported, M.ABI_VERSION)
      end
      if not explicit then loaded = lib end
      return lib
    end
    tried[#tried + 1] = "  " .. path
  end

  return nil,
    "cannot find "
      .. library_name()
      .. ". Build it with `make -C rustcli ffi`, or set CAUSEWAYBAY_LIB.\nLooked in:\n"
      .. table.concat(tried, "\n")
end

--- Take ownership of a `char *` the library returned, and free it.
---
--- Every one of these must be freed, and the copy has to happen before the
--- free, so it is done in exactly one place.
local function take(lib, pointer)
  if pointer == nil then return nil end
  local text = ffi.string(pointer)
  lib.cwb_string_free(pointer)
  return text
end

M.take = take

--- The wallet version reported by the loaded library.
function M.version(lib)
  return take(lib, lib.cwb_version())
end

--- The library's self-description, as raw JSON text.
function M.describe(lib)
  return take(lib, lib.cwb_describe())
end

--- The command tree the library accepts, as raw JSON text.
function M.commands(lib)
  return take(lib, lib.cwb_commands())
end

--- Run one request. `request_json` is text in, envelope text out.
function M.execute(lib, request_json)
  return take(lib, lib.cwb_execute(request_json))
end

return M
