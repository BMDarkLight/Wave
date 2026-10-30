// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Calls to another Wave's `/wave/` API. Every call after pairing names this
//! Wave in `X-Wave-Peer` and carries the pair's secret.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder, Response};

use super::api::{AnswerBody, AskBody, CatalogPage, Hello, UnpairBody};
use super::PeerRecord;

/// Why a call to another Wave failed.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// No answer, or not a Wave answering.
    Unreachable,
    /// The other Wave no longer knows this one.
    Forgotten,
    Refused(u16),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Unreachable => write!(f, "The other device didn't answer"),
            CallError::Forgotten => write!(f, "The other device no longer shares with this one"),
            CallError::Refused(code) => write!(f, "The other device said no ({code})"),
        }
    }
}

/// Short calls: a peer on the same network answers well inside this.
fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .no_proxy()
            .user_agent(concat!("Wave/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

fn checked(sent: Result<Response, reqwest::Error>) -> Result<Response, CallError> {
    let response = sent.map_err(|_| CallError::Unreachable)?;
    match response.status().as_u16() {
        200..=299 => Ok(response),
        403 => Err(CallError::Forgotten),
        code => Err(CallError::Refused(code)),
    }
}

/// Adds who is asking and the pair's secret.
pub fn signed(request: RequestBuilder, own_id: &str, peer: &PeerRecord) -> RequestBuilder {
    request
        .header("X-Wave-Peer", own_id)
        .header("X-Wave-Secret", &peer.secret)
}

pub fn hello(address: &str) -> Result<Hello, CallError> {
    checked(
        client()
            .get(format!("{address}/wave/hello"))
            .timeout(Duration::from_secs(2))
            .send(),
    )?
    .json()
    .map_err(|_| CallError::Unreachable)
}

pub fn ask(address: &str, body: &AskBody) -> Result<(), CallError> {
    checked(
        client()
            .post(format!("{address}/wave/request"))
            .json(body)
            .send(),
    )
    .map(drop)
}

pub fn answer(address: &str, body: &AnswerBody) -> Result<(), CallError> {
    checked(
        client()
            .post(format!("{address}/wave/answer"))
            .json(body)
            .send(),
    )
    .map(drop)
}

pub fn unpair(address: &str, body: &UnpairBody) -> Result<(), CallError> {
    checked(
        client()
            .post(format!("{address}/wave/unpair"))
            .json(body)
            .send(),
    )
    .map(drop)
}

pub fn catalog(peer: &PeerRecord, own_id: &str, cursor: usize) -> Result<CatalogPage, CallError> {
    let request = client().get(format!("{}/wave/catalog?cursor={cursor}", peer.address));
    checked(signed(request, own_id, peer).send())?
        .json()
        .map_err(|_| CallError::Unreachable)
}

pub fn media_url(address: &str, track_id: &str, extension: &str) -> String {
    format!(
        "{address}/wave/media/{}.{extension}",
        crate::remote::uri::percent_encode(track_id)
    )
}

pub fn art_url(address: &str, track_id: &str) -> String {
    format!(
        "{address}/wave/art/{}.jpg",
        crate::remote::uri::percent_encode(track_id)
    )
}
