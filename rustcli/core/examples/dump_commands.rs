//! Print the command tree `api::commands()` reports, for eyeballing it.
//!
//!     cargo run -p causewaybay-core --example dump_commands | python3 -m json.tool

fn main() {
    println!("{}", causewaybay_core::api::commands());
}
