//! The MCP server tixschema generates from the `Remargin` service: the trait, its dispatcher, the
//! handlers, the guards every call runs first and the stdio host.

#[macro_use]
pub mod service;

pub mod context;
pub mod dispatcher;
pub mod handlers;
pub mod host;

#[cfg(test)]
mod tests;
