//! Whether a daemon is there, found out by asking it.
//!
//! The terminal tries each endpoint `vitna_protocol::endpoint` declares and
//! completes the declared handshake with the first that answers. Only an
//! accepted handshake counts as a daemon: a pipe that opens and then says
//! nothing, or says something this terminal cannot read, is reported as that,
//! never as connected. ADR-0005 keeps the daemon off TCP, so there is no port
//! to fall back to, and nothing here tries one.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use vitna_protocol::messages::{HandshakeRequest, HandshakeResponse};
use vitna_protocol::{type_url, ProtocolEnvelope, MAX_FRAME_SIZE_BYTES};

/// Long enough for a daemon under load, short enough that a pipe which opens
/// and never answers does not leave the terminal looking forever.
const PATIENCE: Duration = Duration::from_secs(3);

/// A handshake response is a few hundred bytes. Any process can create a pipe
/// whose name starts `vitna-`, and the endpoint list enumerates those, so the
/// reply's own length prefix is not trusted to size an allocation.
const MAX_REPLY_BYTES: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Probing,
    /// Nothing answered at any declared endpoint.
    Absent {
        why: String,
    },
    /// Something answered and the handshake did not complete: a refusal in the
    /// daemon's own words, silence, or a reply this terminal could not read.
    Trouble {
        what: String,
    },
    /// A daemon accepted the handshake.
    Open {
        version: String,
    },
}

impl Link {
    pub fn is_open(&self) -> bool {
        matches!(self, Link::Open { .. })
    }
}

/// Asks every declared endpoint in turn and reports the first real answer.
pub async fn probe() -> Link {
    let endpoints = vitna_protocol::endpoint::candidates();
    if endpoints.is_empty() {
        let why = vitna_protocol::endpoint::preferred()
            .err()
            .unwrap_or_else(|| "no endpoint is declared for this platform".to_string());
        return Link::Absent {
            why: crate::text::clean(&why),
        };
    }
    for endpoint in &endpoints {
        let Some(stream) = connect(endpoint).await else {
            continue;
        };
        return match tokio::time::timeout(PATIENCE, handshake(stream)).await {
            Ok(Ok(reply)) if reply.accepted => Link::Open {
                version: format!(
                    "{}.{}",
                    reply.selected_version_major, reply.selected_version_minor
                ),
            },
            Ok(Ok(reply)) => Link::Trouble {
                what: crate::text::clean(&if reply.rejection_reason.is_empty() {
                    "the daemon refused the handshake and gave no reason".to_string()
                } else {
                    format!(
                        "the daemon refused the handshake: {}",
                        reply.rejection_reason
                    )
                }),
            },
            Ok(Err(e)) => Link::Trouble {
                what: crate::text::clean(&format!("the handshake failed: {e}")),
            },
            Err(_) => Link::Trouble {
                what: format!(
                    "the daemon did not answer the handshake within {} seconds",
                    PATIENCE.as_secs()
                ),
            },
        };
    }
    Link::Absent {
        why: "nothing is listening at the declared endpoint".to_string(),
    }
}

#[cfg(windows)]
async fn connect(endpoint: &str) -> Option<tokio::net::windows::named_pipe::NamedPipeClient> {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .ok()
}

#[cfg(unix)]
async fn connect(endpoint: &str) -> Option<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(endpoint).await.ok()
}

async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
) -> Result<HandshakeResponse, String> {
    let request = ProtocolEnvelope::carrying(
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        "",
        0,
        "vitna-tui-probe",
        &HandshakeRequest {
            min_supported_version: vitna_protocol::PROTOCOL_VERSION_MAJOR,
            max_supported_version: vitna_protocol::PROTOCOL_VERSION_MAJOR,
            client_identifier: format!("vitna-tui/{}", env!("CARGO_PKG_VERSION")),
        },
    )
    .map_err(|e| e.to_string())?;
    let frame = request.encode_frame().map_err(|e| e.to_string())?;
    stream.write_all(&frame).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;

    let mut len = [0u8; 4];
    stream
        .read_exact(&mut len)
        .await
        .map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len);
    if len > MAX_REPLY_BYTES.min(MAX_FRAME_SIZE_BYTES) {
        return Err(format!(
            "the reply claims {len} bytes, far more than a handshake needs"
        ));
    }
    let mut body = vec![0u8; len as usize];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    let reply: ProtocolEnvelope = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if reply.type_url != type_url::envelope::HANDSHAKE_RESPONSE {
        return Err(format!(
            "the reply was a {}, not a handshake response",
            reply.type_url
        ));
    }
    reply
        .payload_as::<HandshakeResponse>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same stream a daemon would answer on, with this crate's own frames
    /// read back by a stand-in that accepts, so the client half is checked
    /// without starting a daemon.
    #[tokio::test]
    async fn an_accepted_handshake_reads_as_open_and_a_refusal_as_trouble() {
        for (accepted, reason) in [(true, ""), (false, "protocol 9 only")] {
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            let answer = tokio::spawn(async move {
                let mut len = [0u8; 4];
                server.read_exact(&mut len).await.unwrap();
                let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
                server.read_exact(&mut body).await.unwrap();
                let asked: ProtocolEnvelope = serde_json::from_slice(&body).unwrap();
                assert_eq!(asked.type_url, type_url::envelope::HANDSHAKE_REQUEST);
                let reply = ProtocolEnvelope::carrying(
                    type_url::envelope::HANDSHAKE_RESPONSE,
                    "",
                    "",
                    0,
                    "stand-in",
                    &HandshakeResponse {
                        selected_version_major: 1,
                        selected_version_minor: 0,
                        daemon_build_commit: String::new(),
                        accepted,
                        rejection_reason: reason.to_string(),
                    },
                )
                .unwrap();
                server
                    .write_all(&reply.encode_frame().unwrap())
                    .await
                    .unwrap();
            });
            let reply = handshake(client).await.expect("handshake");
            answer.await.unwrap();
            assert_eq!(reply.accepted, accepted);
            assert_eq!(reply.rejection_reason, reason);
        }
    }

    #[tokio::test]
    async fn a_peer_that_hangs_up_is_an_error_not_a_daemon() {
        let (client, server) = tokio::io::duplex(1024);
        drop(server);
        assert!(handshake(client).await.is_err());
    }
}
