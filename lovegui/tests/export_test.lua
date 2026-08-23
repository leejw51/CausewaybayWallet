--- Tests for rendering the wallet list to files.
---
--- Two things are worth pinning here and they are not the same thing.
---
--- The **address** formats are text a person or another program will read, so
--- what matters is that a label cannot break the file it is written into. A
--- label is free text: it can hold a comma, a quote, a newline, a pipe. Every
--- one of those is a separator in one of these four formats.
---
--- The **secret** format is the file that carries the keys, and what matters
--- is that it holds exactly the fields it promises and nothing else. A field
--- silently missing is somebody discovering, on the day they need it, that
--- their backup is not one.

local t = require("tests.runner")
local export = require("export")
local json = require("causewaybay.json")

local ACCOUNTS = {
  {
    label = "treasury",
    address = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
    index = 0, source = "mnemonic", derivation_path = "m/44'/60'/0'/0/0",
  },
  {
    label = "spending",
    address = "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0",
    index = 1, source = "mnemonic", derivation_path = "m/44'/60'/0'/0/1",
  },
}

--- A label containing every separator the four formats use.
local AWKWARD = {
  {
    label = 'a,b "c" |d|\ne',
    address = "0x0000000000000000000000000000000000000001",
    index = 7, source = "mnemonic", derivation_path = "m/44'/60'/0'/0/7",
  },
}

t.suite("export / the four address formats", function()
  t.case("every format is produced, under its own name", function()
    local files = export.addresses(ACCOUNTS)
    for _, name in ipairs(export.ADDRESS_FILES) do
      t.ok(files[name] ~= nil, "missing " .. name)
      t.ok(#files[name] > 0, name .. " should not be empty")
    end
  end)

  t.case("each carries every wallet", function()
    for name, contents in pairs(export.addresses(ACCOUNTS)) do
      for _, account in ipairs(ACCOUNTS) do
        t.contains(contents, account.address, account.label .. " missing from " .. name)
      end
    end
  end)

  t.case("csv quotes a field that would otherwise split the row", function()
    local rows = {}
    for line in export.csv(AWKWARD):gmatch("[^\n]*\n?") do
      if #line > 0 then rows[#rows + 1] = line end
    end
    -- The header, then one record — which spans two physical lines, because
    -- the label has a newline in it and CSV keeps that inside the quotes.
    t.contains(export.csv(AWKWARD), '"a,b ""c"" |d|', "quoted and doubled")
  end)

  t.case("a csv field with no separator is left alone", function()
    -- Quoting everything would also be correct and would make the file
    -- unreadable by eye, which is half of what these are for.
    t.contains(export.csv(ACCOUNTS), ",treasury,", "plain labels stay bare")
  end)

  t.case("markdown escapes a pipe", function()
    local text = export.md(AWKWARD)
    t.contains(text, "\\|d\\|", "an unescaped pipe would invent a column")
  end)

  t.case("markdown has a header and a rule of the right width", function()
    local text = export.md(ACCOUNTS)
    local header = text:match("^([^\n]*)")
    local rule = text:match("^[^\n]*\n([^\n]*)")
    local function bars(line)
      local n = 0
      for _ in line:gmatch("|") do n = n + 1 end
      return n
    end
    t.equal(bars(rule), bars(header), "the rule must match the header")
  end)

  t.case("text columns line up", function()
    local lines = {}
    for line in export.txt(ACCOUNTS):gmatch("[^\n]+") do lines[#lines + 1] = line end
    -- Header, rule, and one line per wallet.
    t.equal(#lines, #ACCOUNTS + 2)
    local at = lines[1]:find("address")
    t.ok(at ~= nil, "there should be an address column")
    for i = 3, #lines do
      t.contains(lines[i]:sub(at, at + 1), "0x",
        "line " .. i .. " should start its address in the same column")
    end
  end)

  t.case("jsonl is one parseable object per wallet", function()
    local seen = 0
    for line in export.jsonl(ACCOUNTS):gmatch("[^\n]+") do
      seen = seen + 1
      local record = json.decode(line)
      t.equal(record.address, ACCOUNTS[seen].address)
      t.equal(record.position, seen, "position counts from one")
      t.equal(record.index, ACCOUNTS[seen].index, "and the BIP-44 index is its own field")
    end
    t.equal(seen, #ACCOUNTS)
  end)

  t.case("an empty list is still a valid file", function()
    -- Saving with nothing selected should produce a header and no rows, not a
    -- broken file and not a crash.
    local files = export.addresses({})
    t.contains(files["wallets.csv"], "position,label,address")
    t.equal(files["wallets.jsonl"], "", "no records means no lines")
    for name, contents in pairs(files) do
      t.ok(type(contents) == "string", name .. " should still be a string")
    end
  end)
end)

t.suite("export / the secret file", function()
  local ROWS = {
    {
      mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
      index = 0,
      address_checksummed = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
      address = "0x9858effd232b4033e47d90003d41ec34ecaeda94",
      private_key = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727",
      public_key_compressed = "0x02" .. ("ab"):rep(32),
      public_key = "0x" .. ("cd"):rep(64),
    },
  }

  t.case("it carries exactly the fields it promises", function()
    local record = json.decode(export.secrets(ROWS))
    for _, column in ipairs(export.SECRET_COLUMNS) do
      t.ok(record[column] ~= nil, "missing " .. column)
    end
    local count = 0
    for _ in pairs(record) do count = count + 1 end
    t.equal(count, #export.SECRET_COLUMNS, "and nothing else")
  end)

  t.case("both spellings of the address are there and agree", function()
    -- EIP-55 is a property of the text, not of the address. A file with only
    -- one form sends somebody to write the conversion themselves, and getting
    -- it subtly wrong is a way to lose money.
    local record = json.decode(export.secrets(ROWS))
    t.equal(record.address, record.address_checksummed:lower(),
      "they must be the same address")
    t.not_equal(record.address, record.address_checksummed,
      "and not the same string, or one of them is wrong")
  end)

  t.case("both public keys are there, at their two lengths", function()
    local record = json.decode(export.secrets(ROWS))
    t.equal(#record.public_key_compressed, 68, "33 bytes as 0x plus 66 hex")
    t.equal(#record.public_key, 130, "64 bytes as 0x plus 128 hex")
  end)

  t.case("one line per wallet and nothing but json", function()
    local many = { ROWS[1], ROWS[1], ROWS[1] }
    local lines = 0
    for line in export.secrets(many):gmatch("[^\n]+") do
      lines = lines + 1
      t.ok(json.decode(line) ~= nil, "every line must parse")
    end
    t.equal(lines, 3)
  end)

  t.case("nothing to export is an empty file, not a broken one", function()
    t.equal(export.secrets({}), "")
  end)

  t.case("it is named so it cannot be committed by accident", function()
    -- The repository ignores `*secret*.jsonl` by name at its root, which is
    -- the single most likely way for this file to escape.
    t.contains(export.SECRET_FILE, "secret")
    t.contains(export.SECRET_FILE, ".jsonl")
  end)
end)
