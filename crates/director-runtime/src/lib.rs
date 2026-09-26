//! Versioned, bounded local IPC. The sidecar computes decisions but never owns
//! equipment, credentials, or acquisition dispatch. Persistence is explicitly opt-in.

use psf_guard_director_core::{evaluate_json, Request, Response, CONTRACT_VERSION, ENGINE_VERSION};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

pub mod storage;
pub const PROTOCOL_VERSION: u32 = 6;
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_FRAME_BYTES: usize = psf_guard_director_core::MAX_REQUEST_BYTES + 4096;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub protocol_version: u32,
    pub session_id: String,
    pub request_id: u64,
    pub payload: Command,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Hello {
        runtime_version: String,
        engine_version: String,
        contract_version: u32,
        rig_id: String,
    },
    Evaluate {
        request: Box<serde_json::value::RawValue>,
    },
    Ledger {
        operation: Box<serde_json::value::RawValue>,
    },
    Ping,
    Shutdown,
}

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Internally tagged enum buffering cannot preserve RawValue. Decode the
        // tag separately, then deserialize directly from the original JSON.
        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        #[derive(Deserialize)]
        struct Tag {
            r#type: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Evaluation {
            r#type: String,
            request: Box<serde_json::value::RawValue>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LedgerOperation {
            r#type: String,
            operation: Box<serde_json::value::RawValue>,
        }
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum Control {
            Hello {
                runtime_version: String,
                engine_version: String,
                contract_version: u32,
                rig_id: String,
            },
            Ping {},
            Shutdown {},
        }
        let tag: Tag = serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
        if tag.r#type == "evaluate" {
            let evaluation: Evaluation =
                serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
            debug_assert_eq!(evaluation.r#type, "evaluate");
            return Ok(Self::Evaluate {
                request: evaluation.request,
            });
        }
        if tag.r#type == "ledger" {
            let operation: LedgerOperation =
                serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
            debug_assert_eq!(operation.r#type, "ledger");
            return Ok(Self::Ledger {
                operation: operation.operation,
            });
        }
        let control: Control = serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
        Ok(match control {
            Control::Hello {
                runtime_version,
                engine_version,
                contract_version,
                rig_id,
            } => Self::Hello {
                runtime_version,
                engine_version,
                contract_version,
                rig_id,
            },
            Control::Ping {} => Self::Ping,
            Control::Shutdown {} => Self::Shutdown,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub protocol_version: u32,
    pub session_id: String,
    pub request_id: u64,
    pub payload: ResultMessage,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultMessage {
    Ready {
        runtime_version: String,
        engine_version: String,
        contract_version: u32,
        rig_id: String,
        storage_enabled: bool,
    },
    Decision {
        response: Response,
    },
    Ledger {
        response: storage::StorageReply,
    },
    Pong,
    Stopped,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Transport,
    Timeout,
    FrameSize,
    InvalidMessage,
    VersionMismatch,
    InvalidHandshake,
    SessionMismatch,
    RequestOrder,
    WrongRig,
    Serialization,
    StorageFailure,
    UntrackedEvaluation,
}

/// Read one little-endian u32 length-prefixed UTF-8 JSON frame. Limits are
/// checked before allocating the body; the deadline covers header AND body.
/// Clean EOF between frames is distinct from a truncated frame.
pub async fn read_frame<S: AsyncRead + Unpin>(
    stream: &mut S,
    deadline: Duration,
) -> Result<Option<Vec<u8>>, ProtocolError> {
    timeout(deadline, async {
        let mut header = [0; 4];
        if stream
            .read(&mut header[..1])
            .await
            .map_err(|_| ProtocolError::Transport)?
            == 0
        {
            return Ok(None);
        }
        stream
            .read_exact(&mut header[1..])
            .await
            .map_err(|_| ProtocolError::Transport)?;
        let len = u32::from_le_bytes(header) as usize;
        if len == 0 || len > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameSize);
        }
        let mut body = vec![0; len];
        stream
            .read_exact(&mut body)
            .await
            .map_err(|_| ProtocolError::Transport)?;
        Ok(Some(body))
    })
    .await
    .map_err(|_| ProtocolError::Timeout)?
}

pub async fn write_frame<S: AsyncWrite + Unpin>(
    stream: &mut S,
    bytes: &[u8],
) -> Result<(), ProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameSize);
    }
    timeout(WRITE_TIMEOUT, async {
        stream
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await
            .map_err(|_| ProtocolError::Transport)?;
        stream
            .write_all(bytes)
            .await
            .map_err(|_| ProtocolError::Transport)?;
        stream.flush().await.map_err(|_| ProtocolError::Transport)
    })
    .await
    .map_err(|_| ProtocolError::Timeout)?
}

async fn receive<S: AsyncRead + Unpin>(
    stream: &mut S,
    deadline: Duration,
) -> Result<Option<Message>, ProtocolError> {
    read_frame(stream, deadline)
        .await?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| ProtocolError::InvalidMessage))
        .transpose()
}

async fn reply<S: AsyncWrite + Unpin>(
    stream: &mut S,
    session_id: &str,
    request_id: u64,
    payload: ResultMessage,
) -> Result<(), ProtocolError> {
    let bytes = serde_json::to_vec(&Reply {
        protocol_version: PROTOCOL_VERSION,
        session_id: session_id.into(),
        request_id,
        payload,
    })
    .map_err(|_| ProtocolError::Serialization)?;
    write_frame(stream, &bytes).await
}

/// Run one pipe session. Any protocol violation terminates it, with no cached
/// decision replay or reconnect. The host must create a new session and fresh
/// snapshot after failure. Heartbeats consume request IDs like any other command.
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) -> Result<(), ProtocolError> {
    serve_with_storage(&mut stream, None).await
}

pub async fn serve_with_storage<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    mut storage: Option<storage::Storage>,
) -> Result<(), ProtocolError> {
    let Some(hello) = receive(&mut stream, HANDSHAKE_TIMEOUT).await? else {
        return Ok(());
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch);
    }
    if hello.request_id != 0
        || hello.session_id.len() != 32
        || !hello.session_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ProtocolError::InvalidHandshake);
    }
    let Command::Hello {
        runtime_version,
        engine_version,
        contract_version,
        rig_id,
    } = hello.payload
    else {
        return Err(ProtocolError::InvalidHandshake);
    };
    if runtime_version != RUNTIME_VERSION
        || engine_version != ENGINE_VERSION
        || contract_version != CONTRACT_VERSION
    {
        return Err(ProtocolError::VersionMismatch);
    }
    if rig_id.is_empty() || rig_id.len() > 128 || !rig_id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ProtocolError::InvalidHandshake);
    }
    reply(
        &mut stream,
        &hello.session_id,
        0,
        ResultMessage::Ready {
            runtime_version,
            engine_version,
            contract_version,
            rig_id: rig_id.clone(),
            storage_enabled: storage.is_some(),
        },
    )
    .await?;
    let mut expected_id = 1;
    while let Some(message) = receive(&mut stream, IDLE_TIMEOUT).await? {
        if message.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::VersionMismatch);
        }
        if message.session_id != hello.session_id {
            return Err(ProtocolError::SessionMismatch);
        }
        if message.request_id != expected_id {
            return Err(ProtocolError::RequestOrder);
        }
        expected_id = expected_id
            .checked_add(1)
            .ok_or(ProtocolError::RequestOrder)?;
        let payload = match message.payload {
            Command::Hello { .. } => return Err(ProtocolError::InvalidHandshake),
            Command::Ping => ResultMessage::Pong,
            Command::Shutdown => {
                reply(
                    &mut stream,
                    &hello.session_id,
                    message.request_id,
                    ResultMessage::Stopped,
                )
                .await?;
                // A completed pipe write does not prove the peer read the reply.
                // Keep the handle alive until the host acknowledges it by closing.
                return match read_frame(&mut stream, Duration::from_secs(2)).await? {
                    None => Ok(()),
                    Some(_) => Err(ProtocolError::InvalidMessage),
                };
            }
            Command::Evaluate { request } => {
                // Once durable accounting is active, caller-supplied progress
                // must not bypass the ledger's pending work and attempt budget.
                if storage.as_ref().is_some_and(storage::Storage::is_open) {
                    return Err(ProtocolError::UntrackedEvaluation);
                }
                // Invalid planning input remains a core error response, distinct
                // from invalid IPC. Valid snapshots must belong to this rig.
                if let Ok(snapshot) = serde_json::from_str::<Request>(request.get())
                    && snapshot.assignment.rig_id != rig_id
                {
                    return Err(ProtocolError::WrongRig);
                }
                ResultMessage::Decision {
                    response: evaluate_json(request.get().as_bytes()),
                }
            }
            Command::Ledger { operation } => {
                if operation.get().len() > psf_guard_director_core::MAX_REQUEST_BYTES {
                    ResultMessage::Ledger {
                        response: storage::StorageReply::Error {
                            code: storage::StorageError::InvalidInput,
                        },
                    }
                } else {
                    let operation = serde_json::from_str(operation.get())
                        .map_err(|_| ProtocolError::InvalidMessage)?;
                    ResultMessage::Ledger {
                        response: storage::execute(&mut storage, operation, &rig_id).await?,
                    }
                }
            }
        };
        reply(&mut stream, &hello.session_id, message.request_id, payload).await?;
    }
    Ok(())
}

#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod preparation_tests;
#[cfg(test)]
mod program_tests;
#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod tests;
