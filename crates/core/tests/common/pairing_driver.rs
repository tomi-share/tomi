//! Replays the cases of `spec/vectors/pairing-v1/pairing.json` through the pairing machines of
//! `tomi-core` (public API and `pairing::kat` only).
//!
//! The same interpreter serves the generator (`examples/gen_pairing_vectors.rs`, which checks
//! every case it writes) and the conformance test (`tests/pairing_vectors.rs`, which checks the
//! frozen file). Include it next to `pairing_reference.rs` with `#[path]`.
//!
//! A case drives **one** machine (the receiver) with frames taken from the files, so that every
//! frame of the other side is fixed bytes, as it would be for an implementation in another
//! language:
//!
//! ```json
//! { "id": "...", "receiver": "joiner" | "inviter",
//!   "setup": { "joiner_ranks": [466, 1633], "offer": false },
//!   "steps": [
//!     { "do": "receive", "frame": FRAME, "expect": "ok" | "ignored" | "ok_or_pairing_failed" | CODE,
//!       "produces": FRAME },
//!     { "do": "user_confirm" },                        // inviter: the user pressed "Add"
//!     { "do": "joiner_confirm", "history": true, "produces": FRAME },  // joiner: user accepted
//!     { "do": "provision", "produces": FRAME } ],      // inviter: build and return the batch
//!   "final_error": CODE | null }
//! FRAME = { "nominal": k } | { "other": k } | { "hex": "..." } | { "padded": { "prefix", "len" } }
//! ```
//!
//! The first error ends the case: it must be the `expect` of the step (when exact) and the
//! `final_error` of the case; a case with `final_error: null` ends without error.

#![allow(dead_code)]

use serde_json::Value;
use tomi_core::pairing::{
    self, kat, Inviter, InviterEvent, InviterParams, Joiner, JoinerEvent, JoinerParams, PairingCode,
};
use tomi_core::{CoreError, DeviceKeys, SpaceKey, TrustStore};

use super::reference as r;

pub use pairing::HistoryOffer;

/// The devices, the space and the randomness of the nominal session, as live objects.
pub struct Fixture {
    pub inputs: r::Inputs,
    pub keys_inviter: DeviceKeys,
    pub trust_inviter: TrustStore,
    pub space_key: SpaceKey,
}

pub fn device_keys(d: &r::Device) -> DeviceKeys {
    DeviceKeys::from_secrets(&d.ed_seed, &d.x_secret).expect("device keys")
}

impl Fixture {
    pub fn new(inputs: &r::Inputs) -> Self {
        let keys_inviter = device_keys(&inputs.inviter);
        let mut trust_inviter =
            TrustStore::new(&inputs.space_id, &keys_inviter.public(), inputs.key_epoch)
                .expect("trust store");
        trust_inviter
            .add_verified_peer(&device_keys(&inputs.other).public())
            .expect("other member");
        Self {
            inputs: inputs.clone(),
            keys_inviter,
            trust_inviter,
            space_key: SpaceKey::from_bytes(inputs.key_epoch, &inputs.space_key)
                .expect("space key"),
        }
    }

    /// Randomness of the inviter: ranks, `sid`, `ya`, then the sealing secret and nonce and the
    /// transfer id (`pairing::kat::inviter_start`).
    pub fn inviter_script(&self) -> Vec<Vec<u8>> {
        let i = &self.inputs;
        vec![
            i.ranks[0].to_be_bytes().to_vec(),
            i.ranks[1].to_be_bytes().to_vec(),
            i.sid.to_vec(),
            i.ya.to_vec(),
            i.seal_ephemeral.to_vec(),
            i.seal_nonce.to_vec(),
            i.transfer_random.to_vec(),
        ]
    }

    pub fn inviter_params(&self, offer: Option<(u64, u64)>) -> InviterParams {
        InviterParams {
            relay_authority: self.inputs.relay_authority.clone(),
            channel_id: self.inputs.channel_id.clone(),
            device_name: self.inputs.inviter_name.clone(),
            platform: self.inputs.inviter_platform.clone(),
            history_offer: offer.map(|(items, bytes)| HistoryOffer { items, bytes }),
        }
    }

    pub fn start_inviter(&self, offer: Option<(u64, u64)>) -> Inviter {
        kat::inviter_start(
            self.inviter_params(offer),
            &self.keys_inviter,
            &self.trust_inviter,
            self.inviter_script(),
        )
        .expect("inviter")
    }

    /// The code as the user of N types it.
    pub fn code(&self, ranks: [u16; 2]) -> PairingCode {
        let word = |r| pairing::word(r).expect("rank in range");
        PairingCode::parse(&format!(
            "{}-{}-{}",
            self.inputs.channel_id,
            word(ranks[0]),
            word(ranks[1])
        ))
        .expect("code")
    }

    pub fn start_joiner(&self, ranks: [u16; 2], yb: [u8; 32]) -> Joiner {
        kat::joiner_new(
            JoinerParams {
                relay_authority: self.inputs.relay_authority.clone(),
                code: self.code(ranks),
                device_name: self.inputs.joiner_name.clone(),
                platform: self.inputs.joiner_platform.clone(),
            },
            device_keys(&self.inputs.joiner),
            vec![yb.to_vec()],
        )
        .expect("joiner")
    }
}

/// The frames a case may refer to.
pub struct Frames<'a> {
    pub nominal: &'a [Vec<u8>],
    pub other: &'a [Vec<u8>],
}

/// Resolves a `FRAME` of a case.
pub fn resolve(spec: &Value, frames: &Frames<'_>) -> Result<Vec<u8>, String> {
    if let Some(k) = spec.get("nominal").and_then(Value::as_u64) {
        return frames
            .nominal
            .get(k as usize)
            .cloned()
            .ok_or_else(|| format!("no nominal frame {k}"));
    }
    if let Some(k) = spec.get("other").and_then(Value::as_u64) {
        return frames
            .other
            .get(k as usize)
            .cloned()
            .ok_or_else(|| format!("no other-session frame {k}"));
    }
    if let Some(hex) = spec.get("hex").and_then(Value::as_str) {
        return hex::decode(hex).map_err(|e| format!("bad hex: {e}"));
    }
    if let Some(p) = spec.get("padded") {
        let prefix =
            hex::decode(p["prefix"].as_str().ok_or("padded.prefix")?).map_err(|e| e.to_string())?;
        let len = p["len"].as_u64().ok_or("padded.len")? as usize;
        let mut v = prefix;
        v.resize(len, 0);
        return Ok(v);
    }
    Err(format!("unknown frame spec {spec}"))
}

enum Machine {
    Joiner(Box<Joiner>),
    Inviter(Box<Inviter>, Option<(u64, u64)>),
}

fn event_name(e: &Result<(Option<Vec<u8>>, &'static str), CoreError>) -> String {
    match e {
        Ok((_, name)) => (*name).to_owned(),
        Err(err) => err.code().to_owned(),
    }
}

/// Replays one case; `Err` describes the first divergence.
pub fn run_case(fx: &Fixture, case: &Value, frames: &Frames<'_>) -> Result<(), String> {
    let id = case["id"].as_str().unwrap_or("?");
    let fail = |msg: String| Err(format!("case {id}: {msg}"));
    let setup = &case["setup"];
    let ranks = match setup.get("joiner_ranks").and_then(Value::as_array) {
        Some(a) => [
            a[0].as_u64().unwrap_or(0) as u16,
            a[1].as_u64().unwrap_or(0) as u16,
        ],
        None => fx.inputs.ranks,
    };
    let yb = match setup.get("joiner_yb").and_then(Value::as_str) {
        Some(h) => r::arr(&r::unhex(h)),
        None => fx.inputs.yb,
    };
    let offer = match setup.get("offer").and_then(Value::as_bool) {
        Some(false) => None,
        _ => fx.inputs.offer,
    };
    let mut machine = match case["receiver"].as_str() {
        Some("joiner") => Machine::Joiner(Box::new(fx.start_joiner(ranks, yb))),
        Some("inviter") => Machine::Inviter(Box::new(fx.start_inviter(offer)), offer),
        other => return fail(format!("unknown receiver {other:?}")),
    };
    let final_error = case["final_error"].as_str();
    let steps = case["steps"].as_array().ok_or("steps")?;
    for (n, step) in steps.iter().enumerate() {
        let at = format!("case {id}, step {n}");
        let action = step["do"].as_str().unwrap_or("?");
        // What the machine returned: an error code, or the frame it produced.
        let outcome: Result<(Option<Vec<u8>>, &'static str), CoreError> =
            match (&mut machine, action) {
                (Machine::Joiner(j), "receive") => {
                    let frame = resolve(&step["frame"], frames)?;
                    j.receive(&frame).map(|s| {
                        (
                            s.send,
                            match s.event {
                                JoinerEvent::Shared => "ok",
                                JoinerEvent::InviterHello => "ok",
                                JoinerEvent::ProvisionVerified => "ok",
                                JoinerEvent::Ignored => "ignored",
                            },
                        )
                    })
                }
                (Machine::Inviter(a, _), "receive") => {
                    let frame = resolve(&step["frame"], frames)?;
                    a.receive(&frame).map(|s| {
                        (
                            s.send,
                            match s.event {
                                InviterEvent::Ignored => "ignored",
                                _ => "ok",
                            },
                        )
                    })
                }
                (Machine::Joiner(j), "joiner_confirm") => {
                    let history = step["history"].as_bool().unwrap_or(false);
                    j.confirm(history).map(|f| (Some(f), "ok"))
                }
                (Machine::Inviter(a, _), "user_confirm") => a.confirm().map(|()| (None, "ok")),
                (Machine::Inviter(a, _), "provision") => a
                    .provision(
                        &fx.keys_inviter,
                        &fx.trust_inviter,
                        &fx.space_key,
                        fx.inputs.now_ms,
                    )
                    .map(|f| (Some(f), "ok")),
                (_, other) => return Err(format!("{at}: unsupported action {other}")),
            };
        let expect = step.get("expect").and_then(Value::as_str).unwrap_or("ok");
        match &outcome {
            Err(e) => {
                let code = e.code();
                let allowed = expect == code
                    || (expect == "ok_or_pairing_failed" && code == "pairing_failed");
                if !allowed {
                    return Err(format!("{at}: expected {expect}, got error {code}"));
                }
                return match final_error {
                    Some(f) if f == code => Ok(()),
                    other => Err(format!("{at}: error {code}, case final_error is {other:?}")),
                };
            }
            Ok((send, name)) => {
                let ok_expected = expect == "ok" || expect == "ok_or_pairing_failed";
                if !((ok_expected && *name == "ok") || (expect == "ignored" && *name == "ignored"))
                {
                    return Err(format!(
                        "{at}: expected {expect}, got {}",
                        event_name(&outcome)
                    ));
                }
                if let Some(want) = step.get("produces") {
                    let want = resolve(want, frames)?;
                    if send.as_deref() != Some(want.as_slice()) {
                        return Err(format!(
                            "{at}: produced {:?}, expected {}",
                            send.as_ref().map(hex::encode),
                            hex::encode(&want)
                        ));
                    }
                }
            }
        }
    }
    match final_error {
        None => Ok(()),
        Some(f) => Err(format!("case {id}: ended without the expected error {f}")),
    }
}

/// What the relay-in-the-middle of P11 chooses.
pub struct Attacker {
    pub sid: [u8; 16],
    pub ya: [u8; 32],
    /// Its scalar toward the inviter (it plays the new device there).
    pub yb: [u8; 32],
}

/// P11: a relay that knows the code runs a session with A (as a joiner) and one with N (as an
/// inviter). Returns the verification codes of A and of N, and the `commit` frame the attacker
/// sent to N before it saw the share of N.
pub fn run_mitm(fx: &Fixture, attacker: &Attacker, yb_n: [u8; 32]) -> (String, String, Vec<u8>) {
    let i = &fx.inputs;
    let mut a = fx.start_inviter(None);
    let mut n = fx.start_joiner(i.ranks, yb_n);
    // The attacker is a device of its own space.
    let keys_m = device_keys(&r::Device::new("M", 0x66, 0x67));
    let trust_m = TrustStore::new("00000000-0000-4000-8000-000000000001", &keys_m.public(), 1)
        .expect("trust");
    let mut toward_n = kat::inviter_start(
        InviterParams {
            device_name: "Relais".into(),
            platform: "linux".into(),
            ..fx.inviter_params(None)
        },
        &keys_m,
        &trust_m,
        vec![
            i.ranks[0].to_be_bytes().to_vec(),
            i.ranks[1].to_be_bytes().to_vec(),
            attacker.sid.to_vec(),
            attacker.ya.to_vec(),
        ],
    )
    .expect("attacker inviter");
    let mut toward_a = kat::joiner_new(
        JoinerParams {
            relay_authority: i.relay_authority.clone(),
            code: fx.code(i.ranks),
            device_name: "Relais".into(),
            platform: "linux".into(),
        },
        keys_m,
        vec![attacker.yb.to_vec()],
    )
    .expect("attacker joiner");
    // With A: the attacker plays N, with the code.
    let share_m = toward_a
        .receive(a.commit_frame())
        .expect("commit")
        .send
        .expect("share");
    let reveal_a = a.receive(&share_m).expect("share").send.expect("reveal");
    toward_a.receive(&reveal_a).expect("reveal");
    // With N: the attacker plays A; its commit goes out before it sees the share of N.
    let commit_m = toward_n.commit_frame().to_vec();
    let share_n = n.receive(&commit_m).expect("commit").send.expect("share");
    let reveal_m = toward_n
        .receive(&share_n)
        .expect("share")
        .send
        .expect("reveal");
    n.receive(&reveal_m).expect("reveal");
    assert_eq!(toward_a.sas(), a.sas(), "the attacker sees the code of A");
    assert_eq!(toward_n.sas(), n.sas(), "and the code of N");
    (
        a.sas().expect("A shows a code").digits(),
        n.sas().expect("N shows a code").digits(),
        commit_m,
    )
}

/// Runs the whole nominal sequence through both machines and returns the seven frames, the
/// verification codes seen at A7 / N7 and the final results.
pub struct NominalRun {
    pub frames: Vec<Vec<u8>>,
    pub sas_inviter: String,
    pub sas_joiner: String,
    pub outcome: tomi_core::pairing::InviterOutcome,
    pub provision: tomi_core::pairing::JoinerProvision,
}

pub fn run_nominal(fx: &Fixture) -> NominalRun {
    let i = &fx.inputs;
    let mut a = fx.start_inviter(i.offer);
    let mut n = fx.start_joiner(i.ranks, i.yb);
    let mut frames = vec![a.commit_frame().to_vec()];
    frames.push(n.receive(&frames[0]).expect("commit").send.expect("share"));
    frames.push(a.receive(&frames[1]).expect("share").send.expect("reveal"));
    frames.push(n.receive(&frames[2]).expect("reveal").send.expect("hello"));
    a.receive(&frames[3]).expect("hello");
    let sas_inviter = a.sas().expect("A shows the code").digits();
    let sas_joiner = n.sas().expect("N shows the code").digits();
    a.confirm().expect("A confirms");
    frames.push(n.confirm(i.joiner_wants_history).expect("N confirms"));
    a.receive(&frames[4]).expect("joiner_confirm");
    frames.push(
        a.provision(&fx.keys_inviter, &fx.trust_inviter, &fx.space_key, i.now_ms)
            .expect("provision"),
    );
    frames.push(n.receive(&frames[5]).expect("provision").send.expect("ack"));
    a.receive(&frames[6]).expect("ack");
    NominalRun {
        frames,
        sas_inviter,
        sas_joiner,
        outcome: a.finish().expect("outcome"),
        provision: n.take_provision().expect("provision"),
    }
}
