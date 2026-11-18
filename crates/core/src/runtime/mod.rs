//! Runtime of the core (feature `runtime`): everything between the protocol bricks and the
//! apps. Four decoupled layers:
//!
//! | layer | module | depends on |
//! |-------|--------|------------|
//! | local persistence (SQLite) | [`store`] | protocol bricks only |
//! | transport trait (REST + doorbell) | [`transport`], [`http`] (feature `http`) | nothing |
//! | orchestration (session, sync, outbox, history transfer) | [`engine`], [`actions`], [`transfer`] | store + transport trait |
//! | pairing over the relay (founder, inviter, new device, resumption) | [`pairing`] | engine + the pure machines of [`crate::pairing`] |
//! | bindings (UniFFI, feature `ffi`) | `crate::ffi` | engine |
//!
//! The engine never talks to the network except through [`transport::RelayTransport`], so
//! it is tested against an in-memory double (`mock`) and a temporary SQLite file, without
//! any relay. The same double backs the demo mode of the apps (`demo`, feature `demo`).

pub mod actions;
#[cfg(feature = "demo")]
pub mod demo;
#[cfg(feature = "demo")]
mod demo_data;
pub mod engine;
#[cfg(feature = "http")]
pub mod http;
mod leave;
#[cfg(any(test, feature = "demo"))]
pub(crate) mod mock;
mod outbox;
pub mod pairing;
mod relay;
mod removal;
pub mod store;
#[cfg(test)]
pub(crate) mod tests;
pub mod transfer;
pub mod transport;

pub use actions::{
    AttachmentView, DeliveryState, DeliveryView, DeviceView, DispatchOutcome, DraftAttachment,
    DraftText, ItemContentView, ItemView, OutgoingDraft, SpaceStatus,
};
pub use engine::{Blocked, Clock, Engine, EngineConfig, Provision, SyncReport, SystemClock};
pub use leave::{DepartureKind, DepartureOutcome, DepartureView, PendingReason, RefusalReason};
pub use pairing::{
    AwaitedHistory, InviteOutcome, InvitePhase, InviteSession, JoinOutcome, JoinPhase, JoinPrompt,
    JoinSession, PairingPeer, ResumeOutcome, VerifyPrompt,
};
pub use removal::AttachmentRemoval;
pub use store::{DeviceState, ItemOrigin, SendStatus, Store};
pub use transfer::TransferView;
pub use transport::{RelayError, RelayTransport, Wake, WakeClosed, WakeSource};
