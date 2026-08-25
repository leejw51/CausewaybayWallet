#!/usr/bin/env bash
#
# Refuse reads of globals that nothing defines.
#
# `local` declarations are only in scope *below* themselves, so a function
# that calls a helper declared later compiles cleanly and crashes at runtime
# with "attempt to call a nil value" — on whatever screen first presses the
# key. That is a shape of bug a compile check cannot see and a headless test
# only sees if it happens to walk the exact path; the bytecode sees it every
# time, as a GGET of a name that is not on the short list of real globals.
#
# LuaJIT's `-bl` listing prints one line per global read:
#     GGET 0 1 ; "love"
# Everything this codebase legitimately reads globally is the standard
# library and `love` itself; any other name is a latent crash, named here at
# lint time instead of discovered by whoever presses Ctrl+V.

set -euo pipefail
cd "$(dirname "$0")/.."

ALLOWED="love require pairs ipairs next type select error assert pcall xpcall \
tostring tonumber print unpack arg collectgarbage load loadstring dofile \
setmetatable getmetatable rawget rawset rawequal rawlen \
math table string os io package coroutine debug jit bit"

status=0
for file in *.lua ui/*.lua tests/*.lua tools/*.lua; do
  [ -e "$file" ] || continue
  while IFS= read -r name; do
    case " $ALLOWED " in
      *" $name "*) ;;
      *)
        echo "  $file reads the global '$name', which nothing defines" >&2
        status=1
        ;;
    esac
  done < <(luajit -bl "$file" 2>/dev/null | grep GGET | sed -n 's/.*; *"\(.*\)"/\1/p' | sort -u)
done

if [ "$status" -ne 0 ]; then
  echo "  a global read like this is a nil at runtime — declare it above its first use" >&2
fi
exit "$status"
