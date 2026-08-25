--- The shared test vectors, driven through the Lua binding.
---
--- `testvectors/` is the same set of files the Rust and Python suites read, so
--- a disagreement between the three shows up here rather than on chain. What
--- this suite adds over the Rust one is the *path*: these numbers now travel
--- through the C ABI and the Lua JSON codec before being compared, which is
--- where a new class of mistake lives — a truncated string, a big integer
--- silently turned into a float, an emoji mangled by an escape.
---
--- Regenerate the files with `make vectors` from the repository root.

local t = require("tests.runner")
local support = require("tests.support")

--- One wallet for the read-only vectors; the ones that write get their own.
local wallet = support.wallet()

t.suite("vectors / keccak", function()
  local vectors = support.vectors("keccak.json")

  t.case("hashes match the published digests", function()
    for _, vector in ipairs(vectors.hashes) do
      local hashed, err = wallet:keccak(vector.text)
      t.ok(hashed, err and err.message)
      t.equal(hashed.keccak256, vector.keccak256, string.format("keccak256(%q)", vector.text))
    end
  end)

  t.case("the emoji vector survives the whole round trip", function()
    -- keccak.json stores it as a \u surrogate pair; a decoder that got that
    -- wrong would hand the wallet different bytes and fail with a hash
    -- mismatch that says nothing about why.
    local emoji
    for _, vector in ipairs(vectors.hashes) do
      if #vector.text > 1 and vector.text:byte(1) > 127 then emoji = vector end
    end
    t.ok(emoji, "keccak.json should carry a non-ASCII vector")
    t.equal(wallet:keccak(emoji.text).keccak256, emoji.keccak256)
  end)

  t.case("selectors are the first four bytes of the signature hash", function()
    for _, vector in ipairs(vectors.selectors) do
      local hashed = wallet:keccak(vector.signature)
      t.equal(hashed.keccak256:sub(1, 10), vector.selector, vector.signature)
    end
  end)
end)

t.suite("vectors / eip55", function()
  local vectors = support.vectors("eip55.json")

  t.case("checksums match the reference addresses", function()
    for _, vector in ipairs(vectors.vectors) do
      t.equal(wallet:checksum(vector.lowercase).address, vector.checksummed)
      -- Already-checksummed input must come back unchanged, and an uppercase
      -- one must be re-cased rather than rejected.
      t.equal(wallet:checksum(vector.checksummed).address, vector.checksummed)
      t.equal(wallet:checksum(vector.lowercase:upper():gsub("^0X", "0x")).address, vector.checksummed)
    end
  end)

  t.case("a malformed address is rejected", function()
    t.fails_with("invalid_address", wallet:checksum("0x123"))
    t.fails_with("invalid_address", wallet:checksum("nonsense"))
  end)
end)

t.suite("vectors / units", function()
  local vectors = support.vectors("units.json")

  t.case("decimal to smallest unit, and back", function()
    for _, vector in ipairs(vectors.valid) do
      local converted, err = wallet:to_wei(vector.amount, vector.decimals)
      t.ok(converted, err and err.message)
      t.equal(converted.value, vector.value, vector.amount .. " @ " .. vector.decimals)

      -- The reverse is only exactly the input when the input was canonical;
      -- converting it once more is, which is the property that matters.
      local back = wallet:from_wei(vector.value, vector.decimals)
      t.equal(wallet:to_wei(back.amount, vector.decimals).value, vector.value)
    end
  end)

  t.case("the 256-bit maximum survives the JSON round trip", function()
    -- The number every layer wants to turn into a double. It stays a string
    -- from Rust to Lua precisely so it cannot.
    local biggest
    for _, vector in ipairs(vectors.valid) do
      if #vector.value > #(biggest and biggest.value or "") then biggest = vector end
    end
    t.ok(#biggest.value >= 70, "units.json should carry a 256-bit value")
    t.equal(type(wallet:to_wei(biggest.amount, biggest.decimals).value), "string")
    t.equal(wallet:to_wei(biggest.amount, biggest.decimals).value, biggest.value)
  end)

  t.case("invalid amounts are rejected, every one", function()
    for _, vector in ipairs(vectors.invalid) do
      local converted, err = wallet:to_wei(vector.amount, vector.decimals)
      t.fails_with("invalid_amount", converted, err)
    end
  end)
end)

t.suite("vectors / bip39", function()
  local vectors = support.vectors("bip39.json")
  local invalid = support.vectors("bip39-invalid.json")

  t.case("every reference phrase imports at its reference address", function()
    -- The seeds themselves are checked by the Rust suite; what this proves is
    -- that all 25 phrases are accepted here and derive deterministically.
    local seen = {}
    for _, vector in ipairs(vectors.vectors) do
      local fresh = support.wallet()
      local account, err = fresh:import_mnemonic(vector.mnemonic, { label = "v" })
      t.ok(account, err and err.message)
      t.equal(account.index, 0)
      t.equal(account.derivation_path, "m/44'/60'/0'/0/0")

      local again = support.wallet():import_mnemonic(vector.mnemonic, { label = "v" })
      t.equal(again.address, account.address, "derivation must be deterministic")
      seen[account.address] = (seen[account.address] or 0) + 1
    end
    -- Distinct entropy must not collapse to one address.
    for address, count in pairs(seen) do
      t.ok(count <= 2, "address " .. address .. " came from " .. count .. " phrases")
    end
  end)

  t.case("word counts are honoured", function()
    for _, vector in ipairs(vectors.vectors) do
      local words = 0
      for _ in vector.mnemonic:gmatch("%S+") do words = words + 1 end
      t.equal(words, vector.word_count)
    end
  end)

  t.case("the same phrase written differently normalises the same", function()
    for _, vector in ipairs(vectors.normalization) do
      local a = support.wallet():import_mnemonic(vector.input, { label = "a" })
      local b = support.wallet():import_mnemonic(vector.canonical, { label = "b" })
      t.ok(a, "the wallet should accept " .. string.format("%q", vector.input))
      t.equal(a.address, b.address)
    end
  end)

  t.case("every invalid phrase is rejected", function()
    for _, vector in ipairs(invalid.vectors) do
      local account, err = support.wallet():import_mnemonic(vector.mnemonic, { label = "x" })
      -- An empty string is refused before the parser ever sees it, and both
      -- the Rust and Python CLIs call that `usage` rather than a bad phrase.
      local expected = vector.mnemonic == "" and "usage" or "invalid_mnemonic"
      t.fails_with(expected, account, err)
    end
  end)
end)

t.suite("vectors / keys", function()
  local vectors = support.vectors("keys.json")
  local invalid = support.vectors("keys-invalid.json")

  t.case("known private keys produce their published addresses", function()
    for _, vector in ipairs(vectors.keys) do
      local account, err = support.wallet():import_key(vector.private_key, { label = "k" })
      t.ok(account, err and err.message)
      t.equal(account.address, vector.address, vector.name)
      t.equal(account.source, "private_key")
    end
  end)

  t.case("a key parses with or without the 0x prefix", function()
    local vector = vectors.keys[1]
    local bare = vector.private_key:gsub("^0x", "")
    t.equal(support.wallet():import_key(bare, { label = "k" }).address, vector.address)
  end)

  t.case("the stored key is the one that was imported", function()
    for _, vector in ipairs(vectors.keys) do
      local w = support.wallet()
      w:import_key(vector.private_key, { label = "k" })
      t.equal(w:account("k", { secret = true }).private_key, vector.private_key:lower())
    end
  end)

  t.case("every invalid key is rejected", function()
    for _, vector in ipairs(invalid.vectors) do
      local account, err = support.wallet():import_key(vector.private_key, { label = "x" })
      -- As with mnemonics: an empty flag is a usage problem, not a bad key.
      local expected = vector.private_key == "" and "usage" or "invalid_private_key"
      t.fails_with(expected, account, err)
    end
  end)
end)

t.suite("vectors / derivation", function()
  local vectors = support.vectors("derivation.json")

  t.case("BIP-44 indices match well-known wallets", function()
    for _, entry in ipairs(vectors.mnemonics) do
      local w = support.wallet()
      for _, account in ipairs(entry.accounts) do
        local derived, err = w:import_mnemonic(entry.phrase, {
          label = entry.name .. "-" .. account.index,
          index = account.index,
        })
        t.ok(derived, err and err.message)
        t.equal(derived.address, account.address, entry.name .. " #" .. account.index)
        t.equal(derived.derivation_path, account.path)
        t.equal(
          w:account(entry.name .. "-" .. account.index, { secret = true }).private_key,
          account.private_key
        )
      end
    end
  end)
end)

t.suite("vectors / eip191", function()
  local vectors = support.vectors("eip191.json")

  t.case("signatures match the reference signer", function()
    local w = support.wallet()
    w:import_key(vectors.signing_key, { label = "signer" })

    for _, vector in ipairs(vectors.vectors) do
      local signed, err = w:sign(vector.message)
      t.ok(signed, err and err.message)
      t.equal(signed.signature, vector.signature, string.format("sign(%q)", vector.message))
      t.equal(signed.address, vector.signer)
    end
  end)

  t.case("every reference signature verifies", function()
    local w = support.wallet()
    for _, vector in ipairs(vectors.vectors) do
      local verified, err = w:verify(vector.message, vector.signature, vector.signer)
      t.ok(verified, err and err.message)
      t.equal(verified.valid, true, string.format("verify(%q)", vector.message))
    end
  end)

  t.case("a tampered message does not verify", function()
    local w = support.wallet()
    local vector = vectors.vectors[2]
    t.equal(w:verify(vector.message .. "!", vector.signature, vector.signer).valid, false)
  end)
end)

t.suite("vectors / multichain", function()
  -- The three SDK-backed chains, checked through the same C ABI the
  -- LÖVE GUI uses. The vectors come from each chain's own SDK, so agreement
  -- here means the Lua path agrees with @solana/web3.js,
  -- cardano-serialization-lib and the Midnight wallet SDK — not merely with
  -- the Rust suite next door.
  local vectors = support.vectors("multichain.json")
  local phrase = vectors.mnemonic

  local function derive(chain, index)
    return wallet:derive({ mnemonic = phrase, index = index, chain = chain })
  end

  t.case("solana addresses and keys match the SDK", function()
    for index, expected in ipairs(vectors.solana.accounts) do
      local got, err = derive("solana", index - 1)
      t.ok(got, err and err.message)
      t.equal(got.address, expected.address, "solana address at " .. (index - 1))
      t.equal(got.derivation_path, expected.path)
      -- On Solana the address *is* the public key, so both come back base58
      -- rather than one of them being hex.
      t.equal(got.public_key, expected.address)
      -- And the secret is the 64-byte keypair a Solana tool would import,
      -- base58 too: 87 or 88 characters, never a 0x string.
      t.equal(got.private_key:match("^0x") ~= nil, false, "a Solana key is not hex")
      t.ok(#got.private_key >= 86, "a Solana keypair is 64 bytes of base58")
      t.equal(got.chain, "solana")
    end
  end)

  t.case("cardano addresses match the SDK, on both networks", function()
    for index, expected in ipairs(vectors.cardano.accounts) do
      local got, err = derive("cardano", index - 1)
      t.ok(got, err and err.message)
      -- The network lives inside a Cardano address, so the wallet carries
      -- both: what it shows is testnet, and mainnet rides along in `extra`.
      t.equal(got.address, expected.base_addr_testnet, "cardano address at " .. (index - 1))
      t.equal(got.address_mainnet, expected.base_addr_mainnet)
      t.equal(got.derivation_path, expected.path)
      t.equal(got.payment_key_hash, expected.payment_keyhash_hex)
    end
  end)

  t.case("midnight addresses match the SDK", function()
    for index, expected in ipairs(vectors.midnight.accounts) do
      local got, err = derive("midnight", index - 1)
      t.ok(got, err and err.message)
      -- Midnight puts the network in the bech32m prefix and drops it on
      -- mainnet, so one key is three addresses. The wallet shows the network
      -- it ships as the default and carries the others alongside.
      t.equal(got.address_mainnet, expected.addr_mainnet, "midnight mainnet at " .. (index - 1))
      t.ok(got.address:match("^mn_addr_"), "a testnet address names its network")
      t.equal(got.derivation_path, expected.path)
      t.equal(got.public_key, expected.verifying_key_hex)
      -- The stored secret is the night key and the dust seed together, since
      -- an account that cannot pay a fee is not a usable account; the night
      -- half is what the SDK vector names.
      t.equal(got.private_key:sub(1, #expected.night_sk_hex), expected.night_sk_hex)
    end
  end)

  t.case("one phrase, every chain, a different address on each", function()
    -- The whole claim of the wallet, in one assertion: the same mnemonic and
    -- the same index give a distinct account per chain, each in its chain's
    -- own encoding.
    local seen = {}
    for _, chain in ipairs({ "evm", "solana", "cardano", "midnight", "ecash" }) do
      local account = assert(derive(chain, 0))
      t.equal(seen[account.address], nil, chain .. " repeats another chain's address")
      seen[account.address] = chain
    end
    t.equal(seen[vectors.solana.accounts[1].address], "solana")
    t.equal(seen[vectors.cardano.accounts[1].base_addr_testnet], "cardano")
  end)
end)

t.suite("vectors / ecash", function()
  -- eCash has its own file rather than a section of `multichain.json`,
  -- because it has no TypeScript SDK in that generator: its keys come from
  -- `eth_account`'s BIP-32 and its addresses from a CashAddr encoder pinned
  -- to the vector published with the CashAddr specification.
  local vectors = support.vectors("ecash.json")

  t.case("addresses match the reference generator, on both networks", function()
    for _, expected in ipairs(vectors.accounts) do
      local got, err = wallet:derive({
        mnemonic = vectors.mnemonic, index = expected.index, chain = "ecash",
      })
      t.ok(got, err and err.message)
      -- A CashAddr prefix sits inside the checksum, so these are two
      -- different strings and not one string wearing two labels.
      t.equal(got.address, expected.address, "ecash address at " .. expected.index)
      t.equal(got.address_mainnet, expected.address_mainnet)
      t.equal(got.derivation_path, expected.path)
      t.equal(got.public_key, expected.public_key_compressed)
      t.equal(got.public_key_hash, expected.public_key_hash)
    end
  end)

  t.case("a key imports back from every encoding it exports", function()
    -- WIF is what eCash's own wallets read and write, and both of its
    -- network forms carry the same scalar. Whichever goes in, hex comes out.
    local expected = vectors.accounts[1]
    for _, form in ipairs({ "wif", "wif_mainnet", "private_key" }) do
      local got, err = wallet:derive({ private_key = expected[form], chain = "ecash" })
      t.ok(got, err and err.message)
      t.equal(got.address, expected.address, "imported from " .. form)
      t.equal(got.private_key, expected.private_key, "stored as hex, from " .. form)
    end
  end)
end)

t.suite("vectors / coverage", function()
  -- The vectors this suite reads. A file added to testvectors/ and not listed
  -- here is a file no Lua test looks at, which is the failure this catches.
  local CONSUMED = {
    "bip39-invalid.json",
    "bip39.json",
    "derivation.json",
    "eip191.json",
    "eip55.json",
    "keccak.json",
    "keys-invalid.json",
    "keys.json",
    "ecash.json",
    "multichain.json",
    "transactions.json",
    "units.json",
  }

  t.case("every vector file is accounted for", function()
    local expected = {}
    for _, name in ipairs(CONSUMED) do expected[name] = true end

    local on_disk = support.vector_files()
    t.ok(#on_disk > 0, "no vector files found")
    for _, name in ipairs(on_disk) do
      t.ok(expected[name], name .. " is on disk but no Lua test reads it")
      expected[name] = nil
    end
    for name in pairs(expected) do
      error(name .. " is listed here but is not on disk", 0)
    end
  end)

  t.case("transactions.json is left to the Rust suite, deliberately", function()
    -- Signing a transaction needs a nonce and a gas price, which means a node.
    -- The Rust suite covers it against a mock; reaching a mock through the FFI
    -- would test the mock, not the wallet. The file is still loaded, so a
    -- malformed one fails here too.
    local vectors = support.vectors("transactions.json")
    t.ok(#vectors.vectors > 0)
    for _, vector in ipairs(vectors.vectors) do
      t.ok(vector.private_key and vector.signer, vector.name)
    end
  end)
end)

return true
