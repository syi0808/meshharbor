use std::{env, io, net::SocketAddr, time::Duration};

use reqwest::{Method, StatusCode, header};
use uuid::Uuid;

use crate::{
    ApiErrorBody,
    blender_bridge::{SendRequest, SendStatus, TRANSFER_LIMIT},
};

pub(crate) const DEFAULT_SERVER: &str = "127.0.0.1:7432";
pub(crate) const SERVER_ENV: &str = "MESHHARBOR_BLENDER_SERVER";
const TOKEN_ENV: &str = "MESHHARBOR_API_TOKEN";
const MAX_REPLY_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn parse_server(value: &str) -> Result<SocketAddr, &'static str> {
    value
        .parse::<SocketAddr>()
        .ok()
        .filter(|server| server.ip().is_loopback() && server.port() != 0)
        .ok_or("Blender transfers require a loopback IP and port, such as 127.0.0.1:7432.")
}

pub(crate) fn configured_server() -> io::Result<SocketAddr> {
    match env::var(SERVER_ENV) {
        Ok(value) => parse_server(&value),
        Err(env::VarError::NotPresent) => parse_server(DEFAULT_SERVER),
        Err(env::VarError::NotUnicode(_)) => Err("MESHHARBOR_BLENDER_SERVER must be valid text."),
    }
    .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))
}

pub(crate) struct Client {
    server: SocketAddr,
    http: reqwest::Client,
}

impl Client {
    pub(crate) fn new(server: SocketAddr) -> io::Result<Self> {
        let token = env::var(TOKEN_ENV).ok().filter(|token| !token.trim().is_empty()).ok_or_else(
            || io::Error::new(
                io::ErrorKind::InvalidInput,
                "Set MESHHARBOR_API_TOKEN to the bearer token for the running MeshHarbor server.",
            ),
        )?;
        Self::with_token(server, &token)
    }

    fn with_token(server: SocketAddr, token: &str) -> io::Result<Self> {
        if !server.ip().is_loopback() || server.port() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Blender server must be on loopback.",
            ));
        }
        let mut bearer =
            header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "MESHHARBOR_API_TOKEN is not a valid bearer token.",
                )
            })?;
        bearer.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer);
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(REQUEST_TIMEOUT)
            .default_headers(headers)
            .build()
            .map_err(|_| {
                io::Error::other("Could not initialize the local Blender transfer client.")
            })?;
        Ok(Self { server, http })
    }

    pub(crate) async fn send(&self, asset_id: String) -> io::Result<SendStatus> {
        let request = self
            .request(Method::POST, "/api/v1/blender/send")
            .json(&SendRequest { asset_id });
        self.read_status(request).await
    }

    pub(crate) async fn status(&self, id: Uuid) -> io::Result<SendStatus> {
        let request = self.request(Method::GET, &format!("/api/v1/blender/send/{id}"));
        let result = self.read_status(request).await?;
        if result.id != id {
            return Err(io::Error::other(
                "The server returned a different Blender transfer ID.",
            ));
        }
        Ok(result)
    }

    pub(crate) async fn wait(&self, mut status: SendStatus) -> io::Result<SendStatus> {
        let id = status.id;
        let result = tokio::time::timeout(TRANSFER_LIMIT + REQUEST_TIMEOUT, async {
            while status.stage.active() {
                tokio::time::sleep(Duration::from_millis(500)).await;
                status = self.status(id).await?;
            }
            Ok(status)
        })
        .await
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The wait timed out.",
            ))
        });
        result.map_err(|error| io::Error::other(format!(
            "Could not confirm Blender transfer {id}: {error} Check it with blender transfer-status."
        )))
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("http://{}{path}", self.server))
    }

    async fn read_status(&self, request: reqwest::RequestBuilder) -> io::Result<SendStatus> {
        let mut reply = request.send().await.map_err(|error| {
            if error.is_connect() {
                io::Error::other(format!(
                    "No MeshHarbor server is reachable at {}. Start meshharbor serve first.",
                    self.server,
                ))
            } else {
                io::Error::other("Could not reach the MeshHarbor Blender transfer API.")
            }
        })?;
        let status = reply.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "MeshHarbor rejected the request. Check that MESHHARBOR_API_TOKEN matches the running server.",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = reply
            .chunk()
            .await
            .map_err(|_| io::Error::other("Could not read the Blender transfer response."))?
        {
            if chunk.len() > MAX_REPLY_BYTES - bytes.len() {
                return Err(io::Error::other(
                    "The Blender transfer response is too large.",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let message = serde_json::from_slice::<ApiErrorBody>(&bytes)
                .map(|result| result.error)
                .unwrap_or_else(|_| {
                    format!("MeshHarbor returned HTTP {status} for the Blender transfer.")
                });
            return Err(io::Error::other(message));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            io::Error::other("The MeshHarbor server returned an invalid Blender transfer response.")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_addresses_and_urls() {
        for invalid in [
            "0.0.0.0:7432",
            "10.0.0.2:7432",
            "::1",
            "[::1]:0",
            "http://127.0.0.1:7432",
            "localhost:7432",
        ] {
            assert!(parse_server(invalid).is_err(), "{invalid}");
        }
        for valid in ["127.0.0.1:7432", "[::1]:7433"] {
            assert!(parse_server(valid).is_ok(), "{valid}");
        }
    }
}
