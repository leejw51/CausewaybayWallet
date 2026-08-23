--- LÖVE's configuration, read before anything else runs.

function love.conf(t)
  t.identity = "causewaybay-gui"
  t.version = "11.4"
  t.window.title = "Causewaybay Bank"

  -- Three times the 480x270 canvas. A whole multiple, so the nearest-neighbour
  -- scale in ui/theme.lua lands every canvas pixel on exactly nine screen ones
  -- and nothing is smeared.
  t.window.width = 1440
  t.window.height = 810
  t.window.minwidth = 480
  t.window.minheight = 270
  t.window.resizable = true
  t.window.vsync = 1
  -- No MSAA: this is pixel art, and the one thing it must not have is
  -- anti-aliased edges.
  t.window.msaa = 0

  t.modules.joystick = false
  t.modules.physics = false
  t.modules.video = false
end
