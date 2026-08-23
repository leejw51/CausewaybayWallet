--- Rendering the wallet list to files.
---
--- Two jobs that look similar and are not:
---
--- **Saving** writes addresses — what a wallet *is* in public. Four formats at
--- once, because the point of saving a list of addresses is to paste it
--- somewhere, and which somewhere decides the format. Harmless to lose.
---
--- **Exporting** writes the keys. One format, because there is exactly one
--- reason to produce this file — moving these wallets into another program —
--- and a spreadsheet is not that reason. Anyone who reads it owns the money.
---
--- Everything here is a pure function from a list of tables to a string.
--- Nothing opens a file, nothing touches `love.*`, nothing reaches the wallet.
--- The caller gathers the rows and the caller writes them, which is what lets
--- the whole of it be tested against fixtures.

local json = require("causewaybay.json")

local export = {}

--- What the saved files are called, and what a caller should offer to write.
export.ADDRESS_FILES = { "wallets.jsonl", "wallets.csv", "wallets.md", "wallets.txt" }

--- What the exported one is called.
---
--- `-secret` is not decoration. The repository ignores `*secret*.jsonl` by
--- name at its root, so a file produced here cannot be committed by accident
--- — which is the single most likely way for it to escape.
export.SECRET_FILE = "wallets-secret.jsonl"

--- The columns of the public list, in the order every format uses.
local COLUMNS = { "position", "label", "address", "index", "source", "derivation_path" }

local function value(account, column, position)
  if column == "position" then return tostring(position) end
  if column == "index" then return tostring(account.index or 0) end
  local field = account[column]
  if field == nil or field == json.null then return "" end
  return tostring(field)
end

--- A CSV field: quoted only when it has to be, escaped when it is.
local function csv_field(text)
  if text:find('[",\n]') then
    return '"' .. text:gsub('"', '""') .. '"'
  end
  return text
end

function export.csv(accounts)
  local lines = { table.concat(COLUMNS, ",") }
  for position, account in ipairs(accounts) do
    local row = {}
    for i, column in ipairs(COLUMNS) do
      row[i] = csv_field(value(account, column, position))
    end
    lines[#lines + 1] = table.concat(row, ",")
  end
  return table.concat(lines, "\n") .. "\n"
end

function export.jsonl(accounts)
  local lines = {}
  for position, account in ipairs(accounts) do
    lines[#lines + 1] = json.encode({
      position = position,
      label = value(account, "label", position),
      address = value(account, "address", position),
      index = account.index or 0,
      source = value(account, "source", position),
      derivation_path = value(account, "derivation_path", position),
    })
  end
  return table.concat(lines, "\n") .. (#lines > 0 and "\n" or "")
end

--- A pipe table. Escaped, because a label is free text and a `|` in one would
--- otherwise split the row into a column that is not there.
function export.md(accounts)
  local lines = {
    "| " .. table.concat(COLUMNS, " | ") .. " |",
    "| " .. ("--- | "):rep(#COLUMNS - 1) .. "--- |",
  }
  for position, account in ipairs(accounts) do
    local row = {}
    for i, column in ipairs(COLUMNS) do
      row[i] = (value(account, column, position):gsub("|", "\\|"))
    end
    lines[#lines + 1] = "| " .. table.concat(row, " | ") .. " |"
  end
  return table.concat(lines, "\n") .. "\n"
end

--- Columns padded to line up, for reading in a terminal.
function export.txt(accounts)
  local widths = {}
  for i, column in ipairs(COLUMNS) do widths[i] = #column end
  for position, account in ipairs(accounts) do
    for i, column in ipairs(COLUMNS) do
      widths[i] = math.max(widths[i], #value(account, column, position))
    end
  end

  local function render(cells)
    local row = {}
    for i, cell in ipairs(cells) do
      row[i] = cell .. (" "):rep(widths[i] - #cell)
    end
    return (table.concat(row, "  "):gsub("%s+$", ""))
  end

  local lines = { render(COLUMNS) }
  local rule = {}
  for i in ipairs(COLUMNS) do rule[i] = ("-"):rep(widths[i]) end
  lines[#lines + 1] = render(rule)
  for position, account in ipairs(accounts) do
    local cells = {}
    for i, column in ipairs(COLUMNS) do
      cells[i] = value(account, column, position)
    end
    lines[#lines + 1] = render(cells)
  end
  return table.concat(lines, "\n") .. "\n"
end

--- Every address format, keyed by the name it should be written under.
function export.addresses(accounts)
  return {
    ["wallets.jsonl"] = export.jsonl(accounts),
    ["wallets.csv"] = export.csv(accounts),
    ["wallets.md"] = export.md(accounts),
    ["wallets.txt"] = export.txt(accounts),
  }
end

-- ------------------------------------------------------------------ secrets

--- The fields of the export, in order, and what each one is.
---
--- Both spellings of the address are here on purpose. The checksummed form is
--- what a person compares by eye and what most tools display; the lower-case
--- form is what many of them compare *with*, because EIP-55 is a property of
--- the text and not of the address. A file that carried only one of them would
--- send somebody to write the conversion themselves, and getting EIP-55 subtly
--- wrong is a way to lose money.
export.SECRET_COLUMNS = {
  "mnemonic",
  "index",
  "address_checksummed",
  "address",
  "private_key",
  "public_key_compressed",
  "public_key",
}

--- One line per wallet, JSON, and nothing else.
---
--- No header, no totals, no comment saying what it is. A comment would be a
--- line that is not JSON in a file whose only purpose is to be parsed, and
--- anything reading this already knows what it asked for.
function export.secrets(rows)
  local lines = {}
  for _, row in ipairs(rows) do
    local record = {}
    for _, column in ipairs(export.SECRET_COLUMNS) do
      record[column] = row[column]
    end
    lines[#lines + 1] = json.encode(record)
  end
  return table.concat(lines, "\n") .. (#lines > 0 and "\n" or "")
end

return export
