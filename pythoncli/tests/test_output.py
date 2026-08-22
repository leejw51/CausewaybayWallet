"""The JSON envelope and human rendering helpers."""

import json

from causewaybay import errors, output
from causewaybay.output import CommandOutput


def test_success_envelope_is_one_well_shaped_line():
    rendered = output.success_envelope({"address": "0xabc"})
    assert "\n" not in rendered
    parsed = json.loads(rendered)
    assert parsed["ok"] is True
    assert parsed["data"]["address"] == "0xabc"


def test_error_envelope_carries_the_stable_code():
    rendered = output.error_envelope(errors.account_not_found("no account matching 'bob'"))
    assert "\n" not in rendered
    parsed = json.loads(rendered)
    assert parsed["ok"] is False
    assert parsed["error"]["code"] == "account_not_found"
    assert parsed["error"]["message"] == "no account matching 'bob'"


def test_every_error_code_is_snake_case():
    for code in errors.ALL_CODES:
        assert code
        assert all(c.islower() or c == "_" for c in code), code


def test_error_constructors_use_their_own_code():
    assert errors.usage("x").code == errors.USAGE
    assert errors.rpc_error("x").code == errors.RPC_ERROR
    assert errors.insufficient_funds("x").code == errors.INSUFFICIENT_FUNDS
    assert str(errors.internal("boom")) == "boom"


def test_tables_align_on_the_longest_key():
    rendered = output.table([("Address", "0x1"), ("Network", "Cronos")])
    assert rendered.splitlines() == ["Address  0x1", "Network  Cronos"]


def test_empty_tables_do_not_fail():
    assert output.table([]) == ""


def test_truncation_hides_the_middle_of_a_secret():
    secret = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
    short = output.truncate_secret(secret)
    assert short.startswith("0x1ab42c")
    assert short.endswith("b727")
    assert "b618bdea" not in short
    assert output.truncate_secret("0xabc") == "***"


def test_short_address_keeps_both_ends():
    short = output.short_address("0x9858EfFD232B4033E47d90003D41EC34EcaEda94")
    assert short.startswith("0x9858Ef")
    assert short.endswith("aEda94")
    assert len(short) < 42
    assert output.short_address("0xabc") == "0xabc"


def test_message_output_mirrors_data_and_text():
    result = CommandOutput.message("done")
    assert result.human == "done"
    assert result.data == {"message": "done"}


def test_envelopes_are_deterministically_ordered():
    # Sorted keys keep the output stable for diffing and for snapshot tests.
    assert output.success_envelope({"b": 1, "a": 2}) == '{"data":{"a":2,"b":1},"ok":true}'
