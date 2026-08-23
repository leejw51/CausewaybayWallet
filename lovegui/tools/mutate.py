#!/usr/bin/env python3
"""Break the code on purpose and check the tests notice.

    python3 lovegui/tools/mutate.py [name fragment ...]

A green suite says nothing about whether it would catch a regression — a test
that cannot fail is decoration. So each mutation below is a plausible edit: a
dropped clamp, a removed guard, an off-by-one, the kind of thing a refactor
does by accident. It is applied to a copy of the tree, the suite is run, and
the copy is thrown away.

A mutation that survives is a hole in the tests. That is the finding; the
number of tests is not.

## Equivalent mutants

Some mutations change no observable behaviour, and no test can or should catch
them. Those are marked `equivalent=` with the reason, and reported separately
from real gaps — otherwise the honest response to a stubborn survivor is to
write a test that asserts an implementation detail, which is worse than the
hole it papers over.

## Not run by `make check`

It copies the tree and runs the suite twenty-odd times. That is a thing to do
when the tests change, not on every build.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

GUI = Path(__file__).resolve().parent.parent
ROOT = GUI.parent

# (name, file, find, replace, suite, equivalent-reason-or-None)
MUTATIONS = [
    ("refresh: never aims at the active account", "model.lua",
     "if not self.aimed and self.active then", "if false then", "model", None),

    ("create: leaves the selection behind", "model.lua",
     "  for index, entry in ipairs(self.wallets) do\n    if entry.address == account.address then self.selected = index end\n  end\n  self:say(\"Created \"",
     "  for index, entry in ipairs(self.wallets) do\n    local _ = index, entry\n  end\n  self:say(\"Created \"", "model", None),

    ("scroll_by: loses its upper clamp", "model.lua",
     "self.scroll = math.max(0, math.min(most, self.scroll + delta))",
     "self.scroll = math.max(0, self.scroll + delta)", "model", None),

    ("reveal: off by one at the bottom", "model.lua",
     "self.scroll = index - visible", "self.scroll = index - visible + 1", "model", None),

    ("set_field: drops the confirmation guard", "model.lua",
     "function Model:set_field(field, text)\n  if self.confirm then return false end",
     "function Model:set_field(field, text)", "model", None),

    ("set_field: drops the type check", "model.lua",
     "  if type(text) ~= \"string\" then return false end\n", "", "model", None),

    ("logout: forgets to drop the balance", "model.lua",
     "  self:refresh()\n  self.balance = nil", "  self:refresh()", "model", None),

    ("login: skips validation", "model.lua",
     "  if not check.valid then", "  if false then", "model",
     # Was recorded as equivalent on the grounds that `validate_mnemonic` and
     # `derive` reject the same phrases with the same message. That was wrong,
     # and a security review found it: they reject the same phrases, but not
     # the same way. `derive` is an argument-parser call, and for input the
     # parser cannot make sense of — a phrase pasted with its bullet still
     # attached — it quotes the input back in the message. `validate` never
     # does. Deleting the call would have opened that path.
     #
     # It is still equivalent, but now because `Model.without_phrase` catches
     # the quoting rather than because the two commands agree. Which is the
     # difference between a mutant that cannot be caught and one that merely
     # is not: with the guard removed as well, this leaks.
     "the phrase guard catches what skipping validation would expose"),

    ("login: a new phrase is imported but not activated", "model.lua",
     "  local ok, use_error = self.wallet:use_account(account.address)\n  if not ok then return self:fail(use_error) end",
     "  local ok, use_error = true, nil\n  if not ok then return self:fail(use_error) end", "model", None),

    ("session: the list is not scoped to the phrase", "model.lua",
     "  if self.session then\n    local mine = {}",
     "  if false then\n    local mine = {}", "model", None),

    ("session: the scan stops at the first gap", "model.lua",
     "if index > 0 and misses >= Model.SESSION_GAP then break end",
     "if index > 0 and misses >= 1 then break end", "model", None),

    ("session: a wallet made inside it is not recorded", "model.lua",
     "  if self.session then\n    self.session.addresses[tostring(account.address):lower()] = true\n  end\n\n  self:refresh()",
     "  self:refresh()", "model", None),

    ("logout: the list stays scoped", "model.lua",
     "  self.session = nil\n  -- Unscoped again", "  self.session = nil\n  if true then else", "model", None),

    ("login: forget leaves the phrase behind", "login.lua",
     "function login:forget()\n  self.phrase = \"\"", "function login:forget()", "login", None),

    ("login: pasting no longer clears minted", "login.lua",
     "  self.phrase = login.tidy(text)\n  self.minted = false", "  self.phrase = login.tidy(text)", "login", None),

    ("login: an empty clipboard wipes the field", "login.lua",
     "  if not text or text:gsub(\"%s\", \"\") == \"\" then return false end", "", "login", None),

    ("launch: never ends unless an outcome arrives", "ui/launch.lua",
     "  if state.held then return state.held end\n  if not busy then return {} end",
     "  if state.held then return state.held end", "launch", None),

    ("launch: stays loud after the flight is over", "ui/launch.lua",
     "  return state ~= nil and state.time < launch.FLOOR",
     "  return state ~= nil", "launch", None),

    ("launch: thrust is not capped", "ui/launch.lua",
     "local thrust = math.min(1, state.time / launch.FLOOR)",
     "local thrust = state.time / launch.FLOOR", "launch", None),

    ("launch: an early outcome is announced immediately", "ui/launch.lua",
     "  if state.time < launch.FLOOR then return nil end", "", "launch", None),

    ("launch: an empty drain counts as held", "ui/launch.lua",
     "  if not state or #events == 0 then return false end",
     "  if not state then return false end", "launch", None),

    ("export: csv stops quoting a field that would split the row", "export.lua",
     "  if text:find('[\",\\n]') then", "  if false then", "export", None),

    ("export: markdown stops escaping a pipe", "export.lua",
     'row[i] = (value(account, column, position):gsub("|", "\\\\|"))',
     "row[i] = value(account, column, position)", "export", None),

    ("export: the secret file drops a column", "export.lua",
     '  "public_key_compressed",\n', "", "export", None),

    ("save: the address files carry the keys", "model.lua",
     "  for name, contents in pairs(export.addresses(self.wallets)) do",
     "  for name, contents in pairs(export.addresses(self.all_wallets or self.wallets)) do",
     "model", None),

    ("wipe: the exported keys survive", "model.lua",
     "    if os.remove(home .. \"/\" .. export.SECRET_FILE) then removed = removed + 1 end",
     "", "model", None),

    ("session: a snapshot for a missing wallet is trusted", "model.lua",
     "  if not found then return false end\n\n  local set = {}",
     "  found = found or { address = snapshot.address, label = snapshot.label or \"?\" }\n\n  local set = {}",
     "model",
     # Checked by hand: `use_account` is the real gate. It refuses an address
     # the store does not hold, so a fabricated `found` cannot produce a
     # session over wallets that are not there — the restore still returns
     # false. The explicit check decides which *label* the session carries,
     # not whether it happens, so removing it changes nothing observable.
     "use_account already refuses an address the store does not hold"),

    ("send: a second one can start under an open confirmation", "model.lua",
     "  if self.confirm then return false end\n\n  if to == \"\" or amount == \"\" then",
     "  if to == \"\" or amount == \"\" then", "model", None),

    ("boot: a missing library no longer halts", "boot.lua",
     "self.halted = true", "self.halted = false", "boot", None),

    ("boot: one key press hands straight over", "boot.lua",
     "  if not self.done then\n    self.shown = #self.lines",
     "  if false then\n    self.shown = #self.lines", "boot", None),

    ("boot: reports a made-up wallet count", "boot.lua",
     '("WALLETS %10d"):format(#accounts)', '("WALLETS %10d"):format(42)', "boot", None),

    ("login: the mask is dropped", "login.lua",
     'return (self.phrase:gsub("%S", MASK))', "return self.phrase", "login", None),

    ("login: typing no longer clears minted", "login.lua",
     "  -- A pasted mnemonic often arrives with newlines in it.\n  self.minted = false\n",
     "  -- A pasted mnemonic often arrives with newlines in it.\n", "login", None),

    ("login: tidy stops collapsing runs", "login.lua",
     'return (text:gsub("^%s+", ""):gsub("%s+$", ""):gsub("%s+", " "))',
     'return (text:gsub("^%s+", ""):gsub("%s+$", ""))', "login", None),

    ("card: the sigil stops being mirrored", "ui/card.lua",
     "grid[row][4] = grid[row][2]\n    grid[row][5] = grid[row][1]",
     "grid[row][4] = bit_of(bits, 3)\n    grid[row][5] = bit_of(bits, 4)", "card", None),

    ("card: the design ignores the address", "ui/card.lua",
     "scheme  = card.SCHEMES[b[1] % #card.SCHEMES + 1],",
     "scheme  = card.SCHEMES[1],", "card", None),

    ("card: the number drops a group", "ui/card.lua",
     "groups[#groups + 1] = hex:sub(i, i + 3)",
     "if i > 1 then groups[#groups + 1] = hex:sub(i, i + 3) end", "card", None),

    ("card: the swipe does not land on the mark", "ui/card.lua",
     "x = direction * travel * (1 - eased),",
     "x = direction * travel * (1 - eased) + 4,", "card", None),

    ("card: the swipe scrolls the wrong way", "ui/card.lua",
     "x = -direction * travel * eased,",
     "x = direction * travel * eased,", "card", None),

    ("card: the swipe stops easing", "ui/card.lua",
     "local eased = anim.quad_out(math.min(1, math.max(0, progress)))",
     "local eased = math.min(1, math.max(0, progress))", "card", None),

    ("card: the swipe front-loads again", "ui/card.lua",
     "local eased = anim.quad_out(math.min(1, math.max(0, progress)))",
     "local eased = anim.expo_out(math.min(1, math.max(0, progress)))", "card", None),

    ("card: the outgoing card fades instead of being clipped", "ui/card.lua",
     "    scale = 1 - 0.06 * eased,\n    alpha = 1,",
     "    scale = 1 - 0.06 * eased,\n    alpha = 1 - eased,", "card", None),

    ("sound: mute stops muting", "ui/sound.lua",
     "if not sound.enabled then return false end", "", "sound", None),

    ("sound: the throttle is removed", "ui/sound.lua",
     "if last and sound.clock - last < gate then return false end", "", "sound", None),

    ("shake: a settled shake still moves the screen", "ui/anim.lua",
     "if not amount or amount < SETTLED then return 0, 0 end",
     "if amount <= 0 then return 0, 0 end", "anim",
     # Equivalent once the offset is rounded, which is the load-bearing half of
     # that fix: a shake of 1e-39 rounds to zero whether or not this guard
     # stops it first. The guard skips the arithmetic and says in the code that
     # settled means settled, but on its own it changes nothing visible.
     #
     # Worth keeping straight: with *both* halves reverted the tremble comes
     # back, and the tests catch it. Neither reverted alone is enough, which is
     # why the rounding is the fix and this is the belt.
     "rounding already sends a settled shake to zero"),

    ("shake: the offset is floored instead of rounded", "ui/anim.lua",
     "  return math.floor(x + 0.5), math.floor(y + 0.5)",
     "  return math.floor(x), math.floor(y)", "anim", None),

    ("anim: decay becomes frame-dependent", "ui/anim.lua",
     "function anim.approach(current, target, rate, dt)",
     "function anim.approach(current, target, rate, dt)\n  return current + (target - current) * 0.1 --[[mutant]]",
     "anim", None),
]


# The copied tree has no rustcli/target for the binding to search, so the
# library is named explicitly. It is the real build — only the Lua is mutated.
LIB = ROOT / "rustcli" / "target" / "release" / "libcausewaybay_ffi.dylib"


def run(tree: Path, suite: str) -> bool:
    """True if the suite passes."""
    env = dict(os.environ, CAUSEWAYBAY_LIB=str(LIB))
    result = subprocess.run(
        ["luajit", "tests/init.lua", suite],
        cwd=tree / "lovegui", capture_output=True, text=True, env=env,
    )
    return result.returncode == 0


def main() -> int:
    only = sys.argv[1:]
    caught, survived, equivalent = [], [], []

    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp) / "tree"
        shutil.copytree(ROOT, base, symlinks=True,
                        ignore=shutil.ignore_patterns(".git", "target", "build", "shots"))

        # Baseline: the suites must pass before anything is broken, or every
        # "caught" below would be meaningless.
        for suite in sorted({m[4] for m in MUTATIONS}):
            if not run(base, suite):
                print(f"  BASELINE FAILS for '{suite}' — fix that first", file=sys.stderr)
                return 2
        print(f"  baseline: every suite passes\n")

        for name, rel, find, replace, suite, known in MUTATIONS:
            if only and not any(o in name for o in only):
                continue
            path = base / "lovegui" / rel
            original = path.read_text()
            if find not in original:
                # The code moved and the mutation no longer applies. Reported
                # as a failure: a stale mutation silently tests nothing.
                print(f"  STALE     {name}  (pattern gone from {rel})")
                survived.append(name)
                continue
            path.write_text(original.replace(find, replace, 1))
            passed = run(base, suite)
            path.write_text(original)

            if passed and known:
                print(f"  equivalent {name}  ({known})")
                equivalent.append(name)
            elif passed:
                print(f"  SURVIVED  {name}  [{suite}]")
                survived.append(name)
            elif known:
                # It was expected to be unobservable and the tests caught it
                # anyway, which means the reasoning behind `equivalent` is
                # wrong. Worth knowing.
                print(f"  caught    {name}  [{suite}]  (marked equivalent — re-check that)")
                caught.append(name)
            else:
                print(f"  caught    {name}  [{suite}]")
                caught.append(name)

    print()
    print(f"  {len(caught)} caught, {len(survived)} survived, "
          f"{len(equivalent)} equivalent")
    if survived:
        print("\n  Not covered:")
        for name in survived:
            print(f"    - {name}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
