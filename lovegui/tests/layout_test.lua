--- The layout arithmetic, in both orientations.
---
--- Drawing cannot be tested headlessly, but where things go can — and "the
--- card overlaps the list" is a layout number being wrong, not a drawing
--- being wrong. Landscape is pinned to the exact values the game shipped
--- with, because the layout module was extracted to add portrait, not to
--- move anything. Portrait is held to invariants: everything inside the
--- canvas, nothing overlapping, in the same top-to-bottom order.

local t = require("tests.runner")
local layout = require("ui.layout")

local function overlaps(a, b)
  return a.x < b.x + b.w and b.x < a.x + a.w
    and a.y < b.y + b.h and b.y < a.y + a.h
end

local function inside(box, width, height)
  return box.x >= 0 and box.y >= 0
    and box.x + box.w <= width and box.y + box.h <= height
end

t.suite("layout / landscape", function()
  local L = layout.compute(480, 270)

  t.case("is exactly the layout the game shipped with", function()
    t.equal(L.portrait, false)
    t.equal(L.top, 68)
    t.equal(L.bottom, 224)
    t.equal(L.bar, 230)
    t.equal(L.list.x, 8)
    t.equal(L.list.w, 196)
    t.equal(L.detail.x, 212)
    t.equal(L.detail.w, 260)
    t.equal(L.list.h, 156)
    -- The action bar, button by button: these were hand-placed offsets, and
    -- the extraction must reproduce them to the pixel.
    t.equal(L.actions.new.x, 8)
    t.equal(L.actions.refresh.x, 212)
    t.equal(L.actions.copy.x, 278)
    t.equal(L.actions.use.x, 324)
    t.equal(L.actions.save.x, 386)
    t.equal(L.actions.keys.x, 432)
    t.equal(L.status_edge, 206)
  end)

  t.case("the send screen shares the wallets screen's columns", function()
    t.equal(L.pad.x, L.list.x)
    t.equal(L.pad.w, L.list.w)
    t.equal(L.form.x, L.detail.x)
    t.equal(L.form.w, L.detail.w)
  end)
end)

t.suite("layout / portrait", function()
  local L = layout.compute(270, 480)

  t.case("everything fits the canvas", function()
    t.equal(L.portrait, true)
    for _, name in ipairs({ "list", "detail", "pad", "form", "net" }) do
      t.ok(inside(L[name], 270, 480), name .. " runs off the canvas")
    end
    for name, box in pairs(L.actions) do
      t.ok(inside(box, 270, 480), name .. " runs off the canvas")
    end
    for name, box in pairs(L.header.buttons) do
      t.ok(inside(box, 270, 480), "header " .. name .. " runs off the canvas")
    end
  end)

  t.case("the bands stack without touching", function()
    t.ok(not overlaps(L.list, L.detail), "the card is under the list, not on it")
    t.ok(not overlaps(L.form, L.pad), "the pad is under the form, not on it")
    t.ok(L.list.y < L.detail.y, "list first, card second")
    t.ok(L.form.y < L.pad.y, "form first, pad second")
  end)

  t.case("the chrome keeps its order: tabs, content, actions, banner", function()
    t.ok(L.header.tabs_y >= L.header.band_h, "tabs below the header band")
    t.ok(L.top > L.header.tabs_y, "content below the tabs")
    t.ok(L.bar >= L.bottom, "actions below the content")
    t.ok(L.bar2 > L.bar, "the verb row below the + NEW row")
    t.ok(L.bar2 + L.button_h <= 480 - 17, "nothing under the warning banner")
  end)

  t.case("the action rows fit, at full size", function()
    for name, box in pairs(L.actions) do
      t.ok(box.h == L.button_h, name .. " was shrunk to fit")
    end
    local verbs = { "refresh", "copy", "use", "save", "keys" }
    for i = 2, #verbs do
      local a, b = L.actions[verbs[i - 1]], L.actions[verbs[i]]
      t.ok(a.x + a.w <= b.x, verbs[i - 1] .. " overlaps " .. verbs[i])
    end
  end)

  t.case("a card with a card's proportions fits the detail band", function()
    local face_h = math.min(L.detail.h, math.floor(L.detail.w / 1.585))
    local face_w = math.floor(face_h * 1.585)
    t.ok(face_w <= L.detail.w and face_h <= L.detail.h)
    t.ok(face_h >= 120, "a card too small to read is not a card")
  end)

  t.case("ten networks fit one column with no scrolling", function()
    local grid = layout.net_grid(L.net.w - 10, L.net.h - 13, 10)
    t.equal(grid.columns, 1, "a 254-wide canvas has no room for two columns")
    t.ok(grid.row_h >= 24, "a network row needs room for its name and symbol")
  end)
end)

t.suite("layout / the network grid", function()
  -- The bug this suite exists for: the last row of networks was drawn over
  -- the sentence under the grid, because the rows divided the whole frame and
  -- the footer was placed inside the same space.
  t.case("the rows stop above the footer, either way up", function()
    for _, size in ipairs({ { 480, 270 }, { 270, 480 } }) do
      local L = layout.compute(size[1], size[2])
      -- The inside of the frame, as `widgets.frame` hands it back.
      local w, h = L.net.w - 10, L.net.h - 13
      -- Well past the ten the wallet has, so adding a chain does not need
      -- this sum revisited.
      for count = 1, 20 do
        local grid = layout.net_grid(w, h, count)
        local bottom = grid.rows * (grid.row_h + grid.gap) - grid.gap
        t.ok(bottom <= grid.footer_y,
          ("%d networks at %dx%d: the rows reach %d, the footer is at %d")
            :format(count, size[1], size[2], bottom, grid.footer_y))
        t.ok(grid.footer_y + 16 <= h, "the footer is drawn through the border")
      end
    end
  end)

  t.case("the columns divide the width without overlapping", function()
    local grid = layout.net_grid(454, 143, 10)
    t.equal(grid.columns, 2, "a 454-wide frame holds two names side by side")
    t.equal(grid.rows, 5)
    t.ok(grid.columns * grid.column_w + grid.column_gap <= 454 + 1,
      "the second column runs off the frame")
  end)

  t.case("no networks is not a division by zero", function()
    local grid = layout.net_grid(454, 143, 0)
    t.equal(grid.rows, 1)
    t.ok(grid.row_h > 0)
  end)
end)

t.suite("layout / the login gate", function()
  local Login = require("login")

  local function boxes_overlap(a, b)
    return a.x < b.x + b.w and b.x < a.x + a.w
      and a.y < b.y + (a.h or 21) and b.y < a.y + (b.h or 21)
  end

  t.case("landscape is the gate as it shipped", function()
    local at = Login.places(480, 270)
    t.equal(at.frame.x, 50)
    t.equal(at.frame.w, 380)
    t.equal(at.enter.x, 50)
    t.equal(at.mint.x, 252)
    t.equal(at.enter.y, 178)
    t.equal(#at.title, 1)
  end)

  t.case("the doors never overlap, either way up", function()
    -- The portrait bug this suite exists for: two 178-wide doors side by
    -- side on a 270-wide canvas sat on top of each other, and the one drawn
    -- second hid the other.
    for _, size in ipairs({ { 480, 270 }, { 270, 480 } }) do
      local at = Login.places(size[1], size[2])
      t.ok(not boxes_overlap(at.enter, at.mint),
        ("UNLOCK and NEW MNEMONIC collide at %dx%d"):format(size[1], size[2]))
    end
  end)

  t.case("portrait stacks and spreads instead of crowding the top", function()
    local at = Login.places(270, 480)
    t.ok(at.mint.y > at.enter.y + at.enter.h, "the doors stack")
    t.equal(at.enter.x, at.mint.x)
    t.equal(at.enter.w, at.mint.w)
    t.ok(at.frame.w >= 200, "the phrase box takes the width")
    t.equal(#at.title, 2, "the name breaks into two lines")
    -- Top to bottom, in reading order, with the honesty line near the foot.
    t.ok(at.logo_y < at.title_y)
    t.ok(at.title_y < at.frame.y)
    t.ok(at.frame.y + at.frame.h < at.enter.y)
    t.ok(at.note_y > at.mint.y + at.mint.h)
    t.ok(at.honesty_y > 480 * 0.8, "the honesty line sits near the bottom")
    -- And everything on the canvas.
    for _, name in ipairs({ "frame", "enter", "mint" }) do
      local b = at[name]
      t.ok(b.x >= 0 and b.x + b.w <= 270, name .. " runs off the canvas")
      t.ok(b.y >= 0 and b.y + (b.h or 21) <= 480, name .. " runs off the canvas")
    end
  end)
end)

return true
