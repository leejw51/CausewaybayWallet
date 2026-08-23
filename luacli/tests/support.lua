--- Shared test scaffolding: throwaway wallet homes, captured output, vectors.

local causewaybay = require("causewaybay")
local json = require("causewaybay.json")

local support = {}

--- The canonical BIP-39 test phrase; address index 0 is 0x9858EfFD…
support.MNEMONIC =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
support.ADDRESS_0 = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
support.ADDRESS_1 = "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0"
support.PRIVATE_KEY = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"

--- Every home this run created, removed by `support.cleanup`.
local homes = {}

--- A wallet home that does not exist yet, in the system temp directory.
---
--- The wallet creates it on first use with mode 0700, so handing it a name
--- that no file occupies is exactly right — and it is never `~`, which is the
--- point: a test must not be able to touch a real wallet.
function support.temp_home()
  local name = os.tmpname()
  os.remove(name)
  homes[#homes + 1] = name
  return name
end

--- Remove every temp home this process made. Called at the end of a run.
function support.cleanup()
  for _, home in ipairs(homes) do
    -- Quoted because a temp path on macOS has directories with no spaces but
    -- there is no reason to bet a `rm -rf` on that.
    os.execute("rm -rf '" .. home .. "'")
  end
  homes = {}
end

--- A wallet over a fresh home. `options` is merged into `causewaybay.open`.
function support.wallet(options)
  options = options or {}
  options.home = options.home or support.temp_home()
  if options.yes == nil then options.yes = true end
  local wallet, err = causewaybay.open(options)
  if not wallet then
    error("cannot open a wallet: " .. tostring(err and err.message or err), 0)
  end
  return wallet, options.home
end

--- A wallet already holding the reference account, labelled "main".
function support.seeded_wallet(options)
  local wallet, home = support.wallet(options)
  local account, err = wallet:import_mnemonic(support.MNEMONIC, { label = "main" })
  if not account then
    error("cannot seed the wallet: " .. tostring(err and err.message or err), 0)
  end
  return wallet, home, account
end

-- ------------------------------------------------------------ captured output

local Capture = {}
Capture.__index = Capture

function Capture:write(...)
  for _, part in ipairs({ ... }) do
    self.parts[#self.parts + 1] = tostring(part)
  end
end

function Capture:text()
  return table.concat(self.parts)
end

--- A stand-in for `io.stdout` that keeps what was written.
function support.capture()
  return setmetatable({ parts = {} }, Capture)
end

--- Streams for `cli.run`, so a CLI test needs no subprocess.
---
--- `stdin` is the text a `-` argument should read as; without it a test that
--- accidentally asks for stdin fails loudly instead of blocking on a terminal.
function support.streams(stdin)
  local out, err = support.capture(), support.capture()
  return {
    stdout = out,
    stderr = err,
    read_stdin = function()
      if stdin == nil then error("this test did not expect stdin to be read", 0) end
      return stdin
    end,
  }, out, err
end

-- -------------------------------------------------------------- test vectors

--- The directory holding this file, so the vectors are found from anywhere.
local function tests_dir()
  return debug.getinfo(1, "S").source:match("^@(.*)/[^/]*$") or "."
end

--- Load one file from `testvectors/`, shared with the Rust and Python suites.
function support.vectors(name)
  local path = tests_dir() .. "/../../testvectors/" .. name
  local file = io.open(path, "r")
  if not file then
    error("cannot read " .. path .. "\nrun `make vectors` from the repository root", 0)
  end
  local text = file:read("*a")
  file:close()
  local value, err = json.try_decode(text)
  if not value then error(path .. " is not valid JSON: " .. tostring(err), 0) end
  return value
end

--- Every vector file on disk, so a suite can prove it reads all of them.
function support.vector_files()
  local names = {}
  local pipe = io.popen("ls '" .. tests_dir() .. "/../../testvectors' 2>/dev/null")
  if not pipe then return names end
  for line in pipe:lines() do
    if line:match("%.json$") then names[#names + 1] = line end
  end
  pipe:close()
  table.sort(names)
  return names
end

return support
