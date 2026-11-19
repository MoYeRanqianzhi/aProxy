//! Protocol 1 for external format programs. Each UTF-8 JSON line sent to a worker
//! has exactly one reply carrying the same sequence number. Use [`Message`] for
//! inputs, [`HelloReply`] for negotiation, and [`Reply`] for stage results.

mod http;
pub mod legacy;
mod messages;
mod reports;

pub use http::*;
pub use messages::*;
pub use reports::*;

#[cfg(test)]
mod tests;
