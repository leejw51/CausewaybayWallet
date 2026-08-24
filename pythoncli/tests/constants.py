"""Fixed values shared by the tests.

The mnemonic below is the canonical BIP-39 test phrase. It is published in every
wallet test suite in existence — never put anything of value at these addresses.
"""

TEST_MNEMONIC = (
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
)
TEST_ADDRESS_0 = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
TEST_ADDRESS_1 = "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0"
# A third address, for the cases that need a token contract *and* a recipient
# that is neither the sender nor the contract.
TEST_ADDRESS_2 = "0xb6716976A3ebe8D39aCEB04372f22Ff8e6802D7A"
TEST_PRIVATE_KEY = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"

# The addresses every Ethereum tool derives from the phrase above.
KNOWN_ADDRESSES = [
    "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
    "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0",
    "0xb6716976A3ebe8D39aCEB04372f22Ff8e6802D7A",
    "0xF3f50213C1d2e255e4B2bAD430F8A38EEF8D718E",
    "0x51cA8ff9f1C0a99f88E86B8112eA3237F55374cA",
]
