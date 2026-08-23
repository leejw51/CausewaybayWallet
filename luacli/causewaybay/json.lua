--- A JSON encoder/decoder, small enough to read in one sitting.
---
--- Written rather than depended on: LÖVE ships no JSON, LuaRocks is not
--- something a wallet should require at runtime, and the only JSON this ever
--- meets is what `causewaybay-core` emitted a microsecond earlier. That makes
--- the job narrow — RFC 8259, UTF-8, no comments, no trailing commas.
---
--- Two shapes need care in Lua and both are handled explicitly:
---
---   * `null` decodes to `json.null`, a unique sentinel, because `nil` in a
---     table is indistinguishable from an absent key.
---   * `{}` and `[]` both decode to an empty table, so `json.empty_array` marks
---     the ones that must re-encode as `[]`.

local json = {}

--- The value JSON `null` decodes to. Compare with `==`.
json.null = setmetatable({}, { __tostring = function() return "null" end })

--- Marker for a table that must encode as `[]` rather than `{}`.
json.empty_array = setmetatable({}, { __tostring = function() return "[]" end })

-- ------------------------------------------------------------------- encoding

local escapes = {
  ['"'] = '\\"',
  ["\\"] = "\\\\",
  ["\b"] = "\\b",
  ["\f"] = "\\f",
  ["\n"] = "\\n",
  ["\r"] = "\\r",
  ["\t"] = "\\t",
}

local function escape_char(c)
  return escapes[c] or string.format("\\u%04x", string.byte(c))
end

local function encode_string(value)
  -- Control characters must be escaped; everything else, including UTF-8
  -- above 0x7f, goes through as the bytes it already is.
  return '"' .. value:gsub('[%z\1-\31\\"]', escape_char) .. '"'
end

local function encode_number(value)
  if value ~= value or value == math.huge or value == -math.huge then
    error("cannot encode " .. tostring(value) .. " as JSON", 0)
  end
  -- Integers must not come out as "1.0": a chain id is not a float.
  if value == math.floor(value) and math.abs(value) < 2 ^ 53 then
    return string.format("%d", value)
  end
  return string.format("%.14g", value)
end

--- True when `t` should encode as a JSON array.
---
--- A Lua table is both a list and a map, so this asks the only question that
--- matters: are the keys exactly 1..n?
local function is_array(t)
  if t == json.empty_array then return true end
  local count = 0
  for _ in pairs(t) do count = count + 1 end
  if count == 0 then return false end
  for i = 1, count do
    if rawget(t, i) == nil then return false end
  end
  return true
end

local encode_value

local function encode_table(value, seen)
  if seen[value] then error("cannot encode a table that contains itself", 0) end
  seen[value] = true

  local out
  if value == json.empty_array then
    out = "[]"
  elseif is_array(value) then
    local parts = {}
    for i = 1, #value do
      parts[i] = encode_value(value[i], seen)
    end
    out = "[" .. table.concat(parts, ",") .. "]"
  else
    -- Sorted so the same table always encodes to the same bytes, which makes
    -- a request diffable and a test assertable.
    local keys = {}
    for k in pairs(value) do
      if type(k) ~= "string" then
        error("a JSON object key must be a string, got " .. type(k), 0)
      end
      keys[#keys + 1] = k
    end
    table.sort(keys)
    local parts = {}
    for i, k in ipairs(keys) do
      parts[i] = encode_string(k) .. ":" .. encode_value(value[k], seen)
    end
    out = "{" .. table.concat(parts, ",") .. "}"
  end

  seen[value] = nil
  return out
end

encode_value = function(value, seen)
  if value == nil or value == json.null then return "null" end
  local kind = type(value)
  if kind == "boolean" then return tostring(value) end
  if kind == "number" then return encode_number(value) end
  if kind == "string" then return encode_string(value) end
  if kind == "table" then return encode_table(value, seen) end
  error("cannot encode a " .. kind .. " as JSON", 0)
end

--- Encode a Lua value as compact JSON text.
function json.encode(value)
  return encode_value(value, {})
end

-- ------------------------------------------------------------------- decoding

local Parser = {}
Parser.__index = Parser

local function fail(self, message)
  error(("invalid JSON at byte %d: %s"):format(self.pos, message), 0)
end

function Parser:skip_whitespace()
  local _, stop = self.text:find("^[ \t\r\n]*", self.pos)
  self.pos = stop + 1
end

function Parser:literal(word, value)
  if self.text:sub(self.pos, self.pos + #word - 1) == word then
    self.pos = self.pos + #word
    return value
  end
  fail(self, "expected " .. word)
end

--- Turn a \uXXXX escape (and its surrogate pair, if any) into UTF-8 bytes.
function Parser:unicode_escape()
  local hex = self.text:sub(self.pos, self.pos + 3)
  if not hex:match("^%x%x%x%x$") then fail(self, "malformed \\u escape") end
  self.pos = self.pos + 4
  local code = tonumber(hex, 16)

  -- A code point above the BMP arrives as a surrogate pair; joining them is
  -- the only way to get an emoji in a label back out intact.
  if code >= 0xD800 and code <= 0xDBFF then
    if self.text:sub(self.pos, self.pos + 1) ~= "\\u" then
      fail(self, "high surrogate without a low surrogate")
    end
    local low_hex = self.text:sub(self.pos + 2, self.pos + 5)
    if not low_hex:match("^%x%x%x%x$") then fail(self, "malformed low surrogate") end
    local low = tonumber(low_hex, 16)
    if low < 0xDC00 or low > 0xDFFF then fail(self, "invalid low surrogate") end
    self.pos = self.pos + 6
    code = 0x10000 + (code - 0xD800) * 0x400 + (low - 0xDC00)
  end

  local function byte(b) return string.char(b) end
  if code < 0x80 then
    return byte(code)
  elseif code < 0x800 then
    return byte(0xC0 + math.floor(code / 0x40)) .. byte(0x80 + code % 0x40)
  elseif code < 0x10000 then
    return byte(0xE0 + math.floor(code / 0x1000))
      .. byte(0x80 + math.floor(code / 0x40) % 0x40)
      .. byte(0x80 + code % 0x40)
  else
    return byte(0xF0 + math.floor(code / 0x40000))
      .. byte(0x80 + math.floor(code / 0x1000) % 0x40)
      .. byte(0x80 + math.floor(code / 0x40) % 0x40)
      .. byte(0x80 + code % 0x40)
  end
end

local string_escapes = {
  ['"'] = '"', ["\\"] = "\\", ["/"] = "/", b = "\b",
  f = "\f", n = "\n", r = "\r", t = "\t",
}

function Parser:parse_string()
  self.pos = self.pos + 1 -- the opening quote
  local parts, start = {}, self.pos
  while true do
    local c = self.text:sub(self.pos, self.pos)
    if c == "" then fail(self, "unterminated string") end
    if c == '"' then
      parts[#parts + 1] = self.text:sub(start, self.pos - 1)
      self.pos = self.pos + 1
      return table.concat(parts)
    end
    if c == "\\" then
      parts[#parts + 1] = self.text:sub(start, self.pos - 1)
      self.pos = self.pos + 1
      local e = self.text:sub(self.pos, self.pos)
      if e == "u" then
        self.pos = self.pos + 1
        parts[#parts + 1] = self:unicode_escape()
      elseif string_escapes[e] then
        parts[#parts + 1] = string_escapes[e]
        self.pos = self.pos + 1
      else
        fail(self, "unknown escape \\" .. e)
      end
      start = self.pos
    else
      self.pos = self.pos + 1
    end
  end
end

function Parser:parse_number()
  local text = self.text:match("^-?%d+%.?%d*[eE]?[-+]?%d*", self.pos)
  local value = text and tonumber(text)
  if not value then fail(self, "malformed number") end
  self.pos = self.pos + #text
  return value
end

function Parser:parse_array()
  self.pos = self.pos + 1
  local out = {}
  self:skip_whitespace()
  if self.text:sub(self.pos, self.pos) == "]" then
    self.pos = self.pos + 1
    return out
  end
  while true do
    out[#out + 1] = self:parse_value()
    self:skip_whitespace()
    local c = self.text:sub(self.pos, self.pos)
    self.pos = self.pos + 1
    if c == "]" then return out end
    if c ~= "," then fail(self, "expected , or ] in an array") end
    self:skip_whitespace()
  end
end

function Parser:parse_object()
  self.pos = self.pos + 1
  local out = {}
  self:skip_whitespace()
  if self.text:sub(self.pos, self.pos) == "}" then
    self.pos = self.pos + 1
    return out
  end
  while true do
    self:skip_whitespace()
    if self.text:sub(self.pos, self.pos) ~= '"' then fail(self, "expected an object key") end
    local key = self:parse_string()
    self:skip_whitespace()
    if self.text:sub(self.pos, self.pos) ~= ":" then fail(self, "expected : after a key") end
    self.pos = self.pos + 1
    out[key] = self:parse_value()
    self:skip_whitespace()
    local c = self.text:sub(self.pos, self.pos)
    self.pos = self.pos + 1
    if c == "}" then return out end
    if c ~= "," then fail(self, "expected , or } in an object") end
  end
end

function Parser:parse_value()
  self:skip_whitespace()
  local c = self.text:sub(self.pos, self.pos)
  if c == "" then fail(self, "unexpected end of input") end
  if c == '"' then return self:parse_string() end
  if c == "{" then return self:parse_object() end
  if c == "[" then return self:parse_array() end
  if c == "t" then return self:literal("true", true) end
  if c == "f" then return self:literal("false", false) end
  if c == "n" then return self:literal("null", json.null) end
  if c:match("[%-%d]") then return self:parse_number() end
  fail(self, "unexpected character " .. string.format("%q", c))
end

--- Decode JSON text. Raises on malformed input; use `json.try_decode` to catch.
function json.decode(text)
  if type(text) ~= "string" then
    error("json.decode expects a string, got " .. type(text), 0)
  end
  local parser = setmetatable({ text = text, pos = 1 }, Parser)
  local value = parser:parse_value()
  parser:skip_whitespace()
  if parser.pos <= #text then
    fail(parser, "trailing content after the value")
  end
  return value
end

--- `pcall`-wrapped decode: returns `value` or `nil, message`.
function json.try_decode(text)
  local ok, result = pcall(json.decode, text)
  if ok then return result end
  return nil, result
end

return json
