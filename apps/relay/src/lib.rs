//! Tomi relay: a blind store-and-forward server (`spec/api-v1.md`).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// `ApiError` is the single error type carried on every handler's `Result`. It holds the RFC
// 9457 fields (detail, field errors, extension members, response headers) so a handler can
// build a complete `application/problem+json` document in one value. That makes it larger
// than 128 bytes, which `result_large_err` flags on the cold error path. Boxing it on every
// `Result<_, ApiError>` across the whole crate would add churn and an allocation on the error
// path for no real gain on an I/O-bound server. We accept the size deliberately.
#![allow(clippy::result_large_err)]

pub mod admin;
pub mod admin_web;
pub mod app;
pub mod blobstore;
pub mod clock;
pub mod config;
pub mod db;
pub mod disk;
pub mod error;
pub mod fcm;
pub mod gate;
pub mod hub;
pub mod input;
pub mod install;
pub mod logging;
pub mod maintenance;
pub mod middleware;
pub mod migrate;
pub mod purge;
pub mod push;
pub mod ratelimit;
pub mod revoke;
pub mod routes;
pub mod spaces;
pub mod state;
pub mod store;
pub mod webauthn;
pub mod wire;
