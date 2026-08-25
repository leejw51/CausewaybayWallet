--- Where everything goes, for either way up.
---
--- The game draws on a 480×270 canvas — or, turned on its side, a 270×480 one.
--- Every screen used to place itself against one shared table of hand-tuned
--- constants; this module *is* that table, computed from the canvas size, so
--- turning the canvas is one call rather than a hunt through every screen.
---
--- Pure arithmetic, no `love.*` anywhere: the numbers are testable headlessly,
--- and the landscape ones are pinned to exactly the values the game shipped
--- with — this module exists to add an orientation, not to move anything.
---
--- The two orientations are not a transform of each other. Landscape is two
--- columns (a list beside a card, a launch pad beside a form); portrait is two
--- bands (the list above the card, the form above the pad), because a column
--- 254 pixels wide cannot stand beside anything. What is shared is the chrome:
--- header, tabs, action bar, warning banner, in the same order either way.

local layout = {}

--- Everything the screens place themselves against, from one canvas size.
function layout.compute(width, height)
  local portrait = height > width
  local L = {
    portrait = portrait,
    width = width,
    height = height,
    margin = 8,
    gutter = 8,
    button_h = 18,
  }

  -- ------------------------------------------------------------- the chrome
  --
  -- Landscape: one header row — title left, network and buttons right — and
  -- the tabs under it. Portrait has no row wide enough for all of that, so
  -- the header is two rows: title and LOGOUT, then the network and the other
  -- buttons. Same content, stacked instead of squeezed.
  if portrait then
    L.header = {
      band_h = 48,
      tabs_y = 52,
      -- The network, named by its key alone. The key carries the chain in its
      -- first word, and the chain twice on a 254-pixel row is once too often.
      network = { align = "left", x = L.margin, y = 27, max_w = width - 148 },
      spinner = { x = width - 142, y = 33 },
      buttons = {
        layout = { x = width - 136, y = 26, w = 40, h = 15 },
        sfx = { x = width - 92, y = 26, w = 36, h = 15 },
        mode = { x = width - 52, y = 26, w = 44, h = 15 },
        logout = { x = width - 62, y = 5, w = 54, h = 15 },
      },
    }
  else
    L.header = {
      band_h = 34,
      tabs_y = 38,
      network = { align = "right", x = width - 202, y = 11, max_w = 140 },
      spinner = { x = width - 210, y = 24 },
      buttons = {
        layout = { x = width - 196, y = 5, w = 40, h = 15 },
        sfx = { x = width - 152, y = 5, w = 36, h = 15 },
        mode = { x = width - 112, y = 5, w = 44, h = 15 },
        logout = { x = width - 62, y = 5, w = 54, h = 15 },
      },
    }
  end

  -- Content sits between the tabs and the action bar; the warning banner owns
  -- the last 17 rows in both orientations. Portrait needs two action rows —
  -- six buttons do not fit one 254-pixel line — so its content ends earlier.
  L.top = L.header.tabs_y + 30
  local banner_y = height - 17
  if portrait then
    L.bar2 = banner_y - 21
    L.bar = L.bar2 - 22
    L.bottom = L.bar - 6
  else
    L.bar = 230
    L.bar2 = L.bar
    L.bottom = 224
  end

  local inner_w = width - L.margin * 2
  local content_h = L.bottom - L.top

  -- ------------------------------------------------------------ the screens
  if portrait then
    -- Bands. The list keeps whole rows (27 each, inside a frame that spends
    -- 13 on its own title), and the card gets the rest.
    local list_h = 13 + 5 * 27
    L.list = { x = L.margin, y = L.top, w = inner_w, h = list_h }
    L.detail = {
      x = L.margin,
      y = L.top + list_h + L.gutter,
      w = inner_w,
      h = content_h - list_h - L.gutter,
    }
    -- The send screen leads with the form — it is the screen's whole point —
    -- and the launch pad takes what is left underneath.
    local form_h = 184
    L.form = { x = L.margin, y = L.top, w = inner_w, h = form_h }
    L.pad = {
      x = L.margin,
      y = L.top + form_h + L.gutter,
      w = inner_w,
      h = content_h - form_h - L.gutter,
    }
  else
    -- Columns, exactly as the game shipped: 196 of list, the rest of card.
    local column = 196
    local right = L.margin + column + L.gutter
    L.list = { x = L.margin, y = L.top, w = column, h = content_h }
    L.detail = { x = right, y = L.top, w = width - right - L.margin, h = content_h }
    L.pad = L.list
    L.form = L.detail
  end

  -- The network screen is one frame either way; how many columns of rows fit
  -- inside it is its own decision, made where it is drawn.
  L.net = { x = L.margin, y = L.top, w = inner_w, h = content_h }

  -- ---------------------------------------------------------- the action bar
  --
  -- Landscape: + NEW under the list, the card's verbs under the card, one row.
  -- Portrait: + NEW (and the status line beside it) on the first row, the
  -- verbs on their own row below — the same buttons, never smaller, just
  -- stacked like everything else.
  local h = L.button_h
  if portrait then
    L.actions = {
      new = { x = L.margin, y = L.bar, w = 84, h = h },
      refresh = { x = L.margin, y = L.bar2, w = 52, h = h },
      copy = { x = L.margin + 56, y = L.bar2, w = 40, h = h },
      use = { x = L.margin + 100, y = L.bar2, w = 48, h = h },
      save = { x = L.margin + 152, y = L.bar2, w = 40, h = h },
      keys = { x = L.margin + 196, y = L.bar2, w = 40, h = h },
    }
    -- The status line runs from + NEW to the edge; the verbs are on their own
    -- row and no longer bound it.
    L.status_edge = width - L.margin
  else
    local right = L.detail.x
    L.actions = {
      new = { x = L.margin, y = L.bar, w = 84, h = h },
      refresh = { x = right, y = L.bar, w = 60, h = h },
      copy = { x = right + 66, y = L.bar, w = 40, h = h },
      use = { x = right + 112, y = L.bar, w = 56, h = h },
      save = { x = right + 174, y = L.bar, w = 40, h = h },
      keys = { x = right + 220, y = L.bar, w = 40, h = h },
    }
    L.status_edge = right - 6
  end

  return L
end

return layout
