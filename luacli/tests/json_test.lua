--- Unit tests for the JSON codec.
---
--- It sits between the wallet and every caller, so a bug here looks like a
--- wallet bug. The cases that matter are the ones Lua gets wrong by default:
--- null against nil, `[]` against `{}`, integers against floats, and the
--- surrogate pairs the vector files actually contain.

local t = require("tests.runner")
local json = require("causewaybay.json")

t.suite("json / encoding", function()
  t.case("encodes the scalars", function()
    t.equal(json.encode(nil), "null")
    t.equal(json.encode(json.null), "null")
    t.equal(json.encode(true), "true")
    t.equal(json.encode(false), "false")
    t.equal(json.encode(42), "42")
    t.equal(json.encode(-7), "-7")
    t.equal(json.encode("hi"), '"hi"')
  end)

  t.case("keeps integers integral", function()
    -- A chain id rendered as 338.0 is not a chain id any node accepts.
    t.equal(json.encode(338), "338")
    t.equal(json.encode(1e15), "1000000000000000")
    t.equal(json.encode(0.5), "0.5")
  end)

  t.case("refuses what JSON cannot carry", function()
    t.raises("nan", function() json.encode(0 / 0) end)
    t.raises("inf", function() json.encode(math.huge) end)
    t.raises("function", function() json.encode(print) end)
  end)

  t.case("escapes what has to be escaped", function()
    t.equal(json.encode('a"b'), '"a\\"b"')
    t.equal(json.encode("a\\b"), '"a\\\\b"')
    t.equal(json.encode("line\nbreak"), '"line\\nbreak"')
    t.equal(json.encode("\1"), '"\\u0001"')
  end)

  t.case("passes UTF-8 through untouched", function()
    -- A label with an emoji in it must survive the round trip as bytes.
    local text = "héllo 🌏"
    t.equal(json.decode(json.encode(text)), text)
  end)

  t.case("tells an array from an object", function()
    t.equal(json.encode({ 1, 2, 3 }), "[1,2,3]")
    t.equal(json.encode({ a = 1 }), '{"a":1}')
    -- An empty table is ambiguous, and objects are the commoner intent.
    t.equal(json.encode({}), "{}")
    t.equal(json.encode(json.empty_array), "[]")
    -- A table with a gap is neither, and guessing would silently drop "c".
    t.raises("key", function() json.encode({ [1] = "a", [3] = "c" }) end)
  end)

  t.case("sorts object keys so output is reproducible", function()
    t.equal(json.encode({ b = 1, a = 2, c = 3 }), '{"a":2,"b":1,"c":3}')
  end)

  t.case("refuses a table that contains itself", function()
    local loop = {}
    loop.self = loop
    t.raises("itself", function() json.encode(loop) end)
  end)

  t.case("refuses a non-string object key", function()
    t.raises("key", function() json.encode({ [true] = 1 }) end)
  end)
end)

t.suite("json / decoding", function()
  t.case("decodes the scalars", function()
    t.equal(json.decode("true"), true)
    t.equal(json.decode("false"), false)
    t.equal(json.decode("null"), json.null)
    t.equal(json.decode("0"), 0)
    t.equal(json.decode("-1.5e3"), -1500)
    t.equal(json.decode('"text"'), "text")
  end)

  t.case("null is its own value, not nil", function()
    -- The distinction the wallet relies on: an absent field and a null one are
    -- different answers, and only one of them means "the wallet said so".
    local decoded = json.decode('{"active":null}')
    t.equal(decoded.active, json.null)
    t.not_equal(decoded.active, nil)
    t.equal(decoded.missing, nil)
  end)

  t.case("decodes nested structures", function()
    local value = json.decode('{"a":[1,{"b":"c"}],"d":{}}')
    t.equal(value.a[1], 1)
    t.equal(value.a[2].b, "c")
    t.equal(type(value.d), "table")
  end)

  t.case("handles the escapes", function()
    t.equal(json.decode('"a\\"b"'), 'a"b')
    t.equal(json.decode('"\\\\"'), "\\")
    t.equal(json.decode('"\\n\\t\\r\\b\\f\\/"'), "\n\t\r\b\f/")
    t.equal(json.decode('"\\u0041"'), "A")
  end)

  t.case("joins surrogate pairs into one code point", function()
    -- keccak.json really does contain "🌏"; decoding it wrong makes
    -- that vector fail with a hash mismatch that says nothing about why.
    t.equal(json.decode('"\\ud83c\\udf0f"'), "🌏")
    t.equal(json.decode('"\\u00e9"'), "é")
    t.equal(json.decode('"\\u20ac"'), "€")
  end)

  t.case("rejects a broken surrogate pair", function()
    t.raises("surrogate", function() json.decode('"\\ud83c"') end)
    t.raises("surrogate", function() json.decode('"\\ud83c\\u0041"') end)
  end)

  t.case("ignores whitespace between tokens", function()
    t.equal(json.decode(' \n\t{ "a" : [ 1 , 2 ] }\r\n ').a[2], 2)
  end)

  t.case("rejects malformed input", function()
    for _, bad in ipairs({
      "", "{", "[1,]", '{"a"}', '{"a":}', "tru", "01x", '"unterminated',
      "{} trailing", '"\\q"',
    }) do
      local value, err = json.try_decode(bad)
      t.equal(value, nil, "expected " .. string.format("%q", bad) .. " to be rejected")
      t.ok(err ~= nil)
    end
  end)

  t.case("try_decode reports rather than raises", function()
    local value, err = json.try_decode("nope")
    t.equal(value, nil)
    t.contains(err, "invalid JSON")
    t.equal(json.try_decode("[1]")[1], 1)
  end)

  t.case("insists on a string", function()
    t.raises("expects a string", function() json.decode(42) end)
  end)
end)

t.suite("json / round trips", function()
  t.case("survives the shapes the wallet actually sends", function()
    local cases = {
      { argv = { "account", "list" }, yes = true },
      { ok = true, data = { address = "0xabc", index = 0 } },
      { ok = false, error = { code = "usage", message = 'bad "quoted" input' } },
    }
    for _, value in ipairs(cases) do
      local encoded = json.encode(value)
      t.equal(json.encode(json.decode(encoded)), encoded)
    end
  end)

  t.case("an empty array survives the trip out but not back", function()
    -- Lua cannot tell `[]` from `{}` once decoded, so the asymmetry is real
    -- and worth pinning: it is why `empty_array` exists on the encode side.
    t.equal(json.encode({ argv = json.empty_array }), '{"argv":[]}')
    t.equal(json.encode(json.decode('{"argv":[]}')), '{"argv":{}}')
  end)
end)

return true
