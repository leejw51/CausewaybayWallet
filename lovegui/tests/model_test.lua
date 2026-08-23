--- Tests for the model — the GUI with the drawing taken away.
---
--- Driven headlessly against a real wallet in a temp home, the same way
--- `luacli`'s interactive loop is. Passing `nil` for `jobs` makes every call
--- synchronous, so a test asserts on the result immediately instead of pumping
--- a thread that does not exist outside LÖVE.

local t = require("tests.runner")
local support = require("tests.support")
local Model = require("model")

--- A model over a fresh wallet, synchronous.
local function model_over(options)
  local wallet = support.wallet(options)
  return Model.new(wallet, nil), wallet
end

t.suite("model / opening", function()
  t.case("starts on the wallets screen with nothing in it", function()
    local model = model_over()
    t.equal(model.screen, "wallets")
    t.equal(#model.wallets, 0)
    t.equal(model.active, nil)
    t.equal(model.balance, nil)
  end)

  t.case("reads wallets that already exist", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    wallet:new_account({ label = "beta" })
    local model = Model.new(wallet, nil)
    t.equal(#model.wallets, 2)
    t.ok(model.active, "one of them should be active")
  end)

  t.case("an absent active account is nil, not json.null", function()
    -- `info` reports null when there is none, and the decoder turns that into
    -- a table. Left alone it would compare unequal to every address and read
    -- as truthy — the sort of bug that shows as "no wallet is ever selected".
    local model = model_over()
    t.equal(type(model.active), "nil")
  end)
end)

t.suite("model / wallets", function()
  t.case("creates one and selects it", function()
    local model = model_over()
    local account = model:create("treasury")
    t.ok(account, "should have returned the account")
    t.equal(#model.wallets, 1)
    t.equal(model.active, account.address)
    t.contains(model.status.text, "treasury")
  end)

  t.case("an empty label is allowed and the wallet names it", function()
    local model = model_over()
    local account = model:create("")
    t.ok(account and #account.label > 0)
  end)

  t.case("a duplicate label fails without throwing", function()
    local model = model_over()
    model:create("same")
    local again = model:create("same")
    t.equal(again, false)
    t.equal(model.status.kind, "error")
    t.equal(model.status.code, "duplicate_label")
    t.equal(#model.wallets, 1, "the failed one must not have been added")
  end)

  t.case("selecting switches the active wallet and drops the balance", function()
    local model = model_over()
    model:create("one")
    model:create("two")
    model.balance = { balance = "5", symbol = "TCRO" }

    t.ok(model:select(1))
    t.equal(model.active, model.wallets[1].address)
    -- A different wallet has a different balance; keeping the old number
    -- beside the new name is the worst of both.
    t.equal(model.balance, nil)
  end)

  t.case("creating one selects it without making it active", function()
    -- Two separate ideas that used to be one. The card should follow what was
    -- just made; what the wallet *spends* from should not move because a
    -- wallet was added.
    local model = model_over()
    model:create("one")
    local spending = model.active
    model.balance = { balance = "5", symbol = "TCRO" }

    model:create("two")
    model:create("three")
    t.equal(model.selected, 3, "the card should show the one just created")
    t.equal(model.active, spending, "but spending should not have moved")
    t.ok(model.balance ~= nil,
      "and the active wallet's balance is still that wallet's, so it stands")
  end)

  t.case("it opens on the wallet that is actually in use", function()
    -- The card shows whatever is selected. Starting on row 1 when the wallet
    -- is spending from row 2 means the first thing a person sees at every
    -- launch is a card that is not the one their money is in.
    local model = model_over()
    model:create("one")
    model:create("two")
    model:create("three")
    model:select(2)

    local fresh = Model.new(model.wallet, nil)
    fresh:refresh()
    t.equal(fresh.active, model.wallets[2].address, "two should be active")
    t.equal(fresh.selected, 2, "and the selection should have found it")
  end)

  t.case("aiming happens once and does not fight the arrow keys", function()
    local model = model_over()
    model:create("one")
    model:create("two")
    model:select(2)
    -- What pressing up does: the view moves `selected` directly.
    model.selected = 1
    model:refresh()
    t.equal(model.selected, 1, "a refresh must not drag it back to the active row")
  end)

  t.case("selecting nothing is refused rather than crashing", function()
    local model = model_over()
    t.equal(model:select(1), false)
    t.equal(model:select(99), false)
  end)
end)

t.suite("model / screens", function()
  t.case("goes to each named screen", function()
    local model = model_over()
    for _, name in ipairs(Model.SCREENS) do
      t.ok(model:go(name))
      t.equal(model.screen, name)
    end
  end)

  t.case("refuses a screen that does not exist", function()
    local model = model_over()
    t.equal(model:go("nowhere"), false)
    t.equal(model.screen, "wallets")
  end)
end)

t.suite("model / networks", function()
  t.case("lists them and switches", function()
    local model = model_over()
    t.equal(#model:networks(), 2)
    t.ok(model:switch_network("cronos-mainnet"))
    t.equal(model.info.chain_id, 25)
  end)

  t.case("an unknown network is an error, not a crash", function()
    local model = model_over()
    t.equal(model:switch_network("ethereum"), false)
    t.equal(model.status.code, "unknown_network")
  end)
end)

t.suite("model / sending", function()
  t.case("refuses an empty form before reaching a node", function()
    local model = model_over()
    t.equal(model:begin_send("", ""), false)
    t.equal(model.status.code, "usage")
    t.equal(model.confirm, nil)
  end)

  t.case("the confirmation drops the CLI's advice", function()
    -- The wallet's refusal ends with "— re-run with --yes to confirm", which
    -- is guidance for a shell and nonsense in a dialog with a button under it.
    t.equal(
      Model.plan_summary("Send 0.5 TCRO to 0xabc — re-run with --yes to confirm"),
      "Send 0.5 TCRO to 0xabc")
    t.equal(Model.plan_summary("Forget account old"), "Forget account old")
    t.equal(Model.plan_summary(nil), nil)
  end)

  t.case("cancelling clears the dialog and signs nothing", function()
    local model = model_over()
    model.confirm = { summary = "Send 1", to = "0xabc", amount = "1" }
    t.ok(model:cancel_send())
    t.equal(model.confirm, nil)
    t.contains(model.status.text, "nothing was signed")
  end)

  t.case("cancelling when nothing is pending does nothing", function()
    local model = model_over()
    t.equal(model:cancel_send(), false)
  end)

  t.case("confirming with no plan does nothing", function()
    local model = model_over()
    t.equal(model:confirm_send(), false)
  end)
end)

t.suite("model / the form", function()
  t.case("types, deletes and moves between fields", function()
    local model = model_over()
    model:type_into("0xab")
    t.equal(model.form.to, "0xab")
    model:backspace()
    t.equal(model.form.to, "0xa")

    model:next_field()
    t.equal(model.focus, "amount")
    model:type_into("1.5")
    t.equal(model.form.amount, "1.5")
    t.equal(model.form.to, "0xa", "the other field is untouched")

    model:next_field()
    t.equal(model.focus, "to", "it wraps")
  end)

  t.case("backspacing an empty field is harmless", function()
    local model = model_over()
    model:backspace()
    t.equal(model.form.to, "")
  end)

  t.case("the dialog owns the keyboard while it is up", function()
    -- Typing behind a modal is how a person edits a transaction they think
    -- they are confirming.
    local model = model_over()
    model.confirm = { summary = "Send 1", to = "0xabc", amount = "1" }
    model:type_into("9")
    model:backspace()
    t.equal(model.form.to, "")
  end)
end)

t.suite("model / events", function()
  t.case("reports what happened, once", function()
    local model = model_over()
    model:create("alpha")
    local events = model:drain()
    t.ok(#events > 0, "creating should have emitted something")
    t.equal(#model:drain(), 0, "draining twice must not repeat them")
  end)

  t.case("a failure emits an error for the view to react to", function()
    local model = model_over()
    model:switch_network("nowhere")
    local seen = false
    for _, event in ipairs(model:drain()) do
      if event == "error" then seen = true end
    end
    t.ok(seen)
  end)
end)

t.suite("model / asynchrony", function()
  t.case("a submitted request is not answered until it is pumped", function()
    -- The shape the worker thread gives the real GUI, faked with a queue.
    local queue, answers = {}, {}
    local jobs = {
      submit = function(request) queue[#queue + 1] = request end,
      poll = function()
        local answer = table.remove(answers, 1)
        if not answer then return nil end
        return answer.id, answer.envelope
      end,
    }
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local model = Model.new(wallet, jobs)

    model:fetch_balance()
    t.equal(#queue, 1, "it should have been submitted")
    t.ok(model:busy(), "and the model should say it is busy")
    t.equal(model.balance, nil, "with no answer yet")

    answers[1] = {
      id = queue[1].id,
      envelope = { ok = true, data = { balance = "12.5", symbol = "TCRO" } },
    }
    model:pump()
    t.equal(model.balance.balance, "12.5")
    t.equal(model:busy(), false)
  end)

  t.case("an error from the worker becomes a status, not a crash", function()
    local answers = {}
    local jobs = {
      submit = function(request)
        answers[#answers + 1] = {
          id = request.id,
          envelope = { ok = false, error = { code = "rpc_error", message = "node is down" } },
        }
      end,
      poll = function()
        local answer = table.remove(answers, 1)
        if not answer then return nil end
        return answer.id, answer.envelope
      end,
    }
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local model = Model.new(wallet, jobs)

    model:fetch_balance()
    model:pump()
    t.equal(model.status.kind, "error")
    t.equal(model.status.code, "rpc_error")
  end)

  t.case("a balance with no wallets is refused before the node", function()
    local model = model_over()
    t.equal(model:fetch_balance(), false)
    t.equal(model.status.code, "no_active_account")
  end)
end)

return true
