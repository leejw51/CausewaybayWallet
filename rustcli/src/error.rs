//! Error type carrying a stable machine-readable code.
//!
//! Every code here is part of the public contract documented in `SPEC.md`; the
//! Python implementation emits the same strings so callers (including AI agents
//! driving the CLI) can branch on them.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Usage,
    NotFound,
    AccountNotFound,
    DuplicateLabel,
    InvalidMnemonic,
    InvalidPrivateKey,
    InvalidAddress,
    InvalidAmount,
    NoActiveAccount,
    UnknownNetwork,
    RpcError,
    InsufficientFunds,
    ConfirmationRequired,
    IoError,
    Internal,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Usage => "usage",
            Code::NotFound => "not_found",
            Code::AccountNotFound => "account_not_found",
            Code::DuplicateLabel => "duplicate_label",
            Code::InvalidMnemonic => "invalid_mnemonic",
            Code::InvalidPrivateKey => "invalid_private_key",
            Code::InvalidAddress => "invalid_address",
            Code::InvalidAmount => "invalid_amount",
            Code::NoActiveAccount => "no_active_account",
            Code::UnknownNetwork => "unknown_network",
            Code::RpcError => "rpc_error",
            Code::InsufficientFunds => "insufficient_funds",
            Code::ConfirmationRequired => "confirmation_required",
            Code::IoError => "io_error",
            Code::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Error {
    pub code: Code,
    pub message: String,
}

impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Error {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::new(Code::IoError, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Shorthand constructors, one per code that is raised from more than one site.
macro_rules! ctor {
    ($name:ident, $code:ident) => {
        pub fn $name(message: impl Into<String>) -> Error {
            Error::new(Code::$code, message)
        }
    };
}

ctor!(usage, Usage);
ctor!(not_found, NotFound);
ctor!(account_not_found, AccountNotFound);
ctor!(duplicate_label, DuplicateLabel);
ctor!(invalid_mnemonic, InvalidMnemonic);
ctor!(invalid_private_key, InvalidPrivateKey);
ctor!(invalid_address, InvalidAddress);
ctor!(invalid_amount, InvalidAmount);
ctor!(no_active_account, NoActiveAccount);
ctor!(unknown_network, UnknownNetwork);
ctor!(rpc_error, RpcError);
ctor!(insufficient_funds, InsufficientFunds);
ctor!(confirmation_required, ConfirmationRequired);
ctor!(internal, Internal);
