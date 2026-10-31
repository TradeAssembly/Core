//! Dedicated bounded data transport. Legacy Reach's response limit is unchanged.
use crate::service::reach_data_consumer::{
    DataDelivery, DataNodeCall, DataNodeOperation, DataNodeRecord, ReachDataTransport,
};
use reqwest::{blocking::Client, redirect::Policy, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use std::{io::Read, time::Duration};
use url::Url;
pub const MAX_DATA_RESPONSE_BYTES: usize = 655360;
pub struct HttpReachDataTransport {
    base: Url,
    client: Client,
}
impl HttpReachDataTransport {
    pub fn new(origin: &str) -> Result<Self, String> {
        let mut base = Url::parse(origin).map_err(|_| "reach_data_origin_invalid")?;
        let loopback = base.host_str().is_some_and(|s| {
            s.eq_ignore_ascii_case("localhost")
                || s.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback())
        });
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !matches!(base.path(), "" | "/")
            || !(base.scheme() == "https" || base.scheme() == "http" && loopback)
        {
            return Err("reach_data_origin_invalid".into());
        }
        base.set_path("/");
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "reach_data_transport_unavailable")?;
        Ok(Self { base, client })
    }
    fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        operation: &str,
        body: &T,
        max_request: usize,
    ) -> Result<R, String> {
        let bytes = serde_json::to_vec(body).map_err(|_| "reach_data_request_invalid")?;
        if bytes.len() > max_request {
            return Err("reach_data_request_too_large".into());
        }
        let url = self
            .base
            .join(&format!("v1/reach/data/node/{operation}"))
            .map_err(|_| "reach_data_origin_invalid")?;
        let mut response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .map_err(|_| "reach_data_transport_unavailable")?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err("reach_data_authority_refused".into())
            }
            StatusCode::CONFLICT => return Err("reach_data_revision_conflict".into()),
            s if s.is_server_error() => return Err("reach_data_transport_unavailable".into()),
            _ => return Err("reach_data_response_invalid".into()),
        }
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take((MAX_DATA_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "reach_data_transport_unavailable")?;
        if bytes.len() > MAX_DATA_RESPONSE_BYTES {
            return Err("reach_data_response_too_large".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "reach_data_response_invalid".into())
    }
    fn request(&self, r: &DataNodeCall, op: DataNodeOperation) -> Result<(), String> {
        r.signing_payload()?;
        if r.operation != op || r.signature.is_empty() {
            return Err("reach_data_node_request_invalid".into());
        }
        Ok(())
    }
}
impl ReachDataTransport for HttpReachDataTransport {
    fn claim(&self, r: &DataNodeCall) -> Result<Option<DataDelivery>, String> {
        self.request(r, DataNodeOperation::Claim)?;
        let response: serde_json::Value = self.post("claim", r, 8192)?;
        if response.is_null() {
            return Ok(None);
        }
        let c: DataDelivery =
            serde_json::from_value(response).map_err(|_| "reach_data_response_invalid")?;
        c.validate()?;
        Ok(Some(c))
    }
    fn accept(&self, r: &DataNodeCall) -> Result<DataDelivery, String> {
        self.request(r, DataNodeOperation::Accept)?;
        let c: DataDelivery = self.post("accept", r, 8192)?;
        c.validate()?;
        Ok(c)
    }
    fn record(&self, r: &DataNodeRecord) -> Result<DataDelivery, String> {
        self.request(&r.request, DataNodeOperation::Record)?;
        if r.request.receipt_sha256.as_deref() != Some(r.receipt.receipt_sha256.as_str())
            || r.receipt.execution_authority_granted
        {
            return Err("reach_data_record_invalid".into());
        }
        let c: DataDelivery = self.post("record", r, 327680)?;
        c.validate()?;
        Ok(c)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    fn call() -> DataNodeCall {
        DataNodeCall {
            schema: crate::service::reach_data_consumer::NODE_CALL_SCHEMA.into(),
            node: crate::ports::ReachNodeScope {
                tenant_id: "tenant".into(),
                workspace_id: "workspace".into(),
                node_id: "node".into(),
                actor_ref: "actor".into(),
            },
            request_id: "request".into(),
            nonce: "nonce".into(),
            issued_at_ms: 1000,
            source_binding_sha256: "a".repeat(64),
            operation: DataNodeOperation::Claim,
            command_sha256: None,
            receipt_sha256: None,
            expected_revision: None,
            signature: "fixture.signature".into(),
        }
    }
    fn serve(status: u16, body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut block = [0u8; 4096];
            loop {
                let n = stream.read(&mut block).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&block[..n]);
                if let Some(i) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..i]);
                    let size = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .and_then(|s| s.parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= i + 4 + size {
                        assert!(headers.starts_with("POST /v1/reach/data/node/claim HTTP/1.1"));
                        break;
                    }
                }
            }
            let headers=format!("HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:9/not-followed\r\nConnection: close\r\n\r\n",body.len());
            stream.write_all(headers.as_bytes()).unwrap();
            let _ = stream.write_all(&body);
        });
        (origin, handle)
    }
    #[test]
    fn dedicated_wire_bounds_status_and_redirects_fail_closed() {
        for (status, body, expected) in [
            (200, b"null".to_vec(), None),
            (
                403,
                b"fake-secret-must-not-echo".to_vec(),
                Some("reach_data_authority_refused"),
            ),
            (409, vec![], Some("reach_data_revision_conflict")),
            (503, vec![], Some("reach_data_transport_unavailable")),
            (302, vec![], Some("reach_data_response_invalid")),
            (200, b"{}".to_vec(), Some("reach_data_response_invalid")),
            (
                200,
                b"{\"command\":null,\"extra\":true}".to_vec(),
                Some("reach_data_response_invalid"),
            ),
            (
                200,
                vec![b' '; MAX_DATA_RESPONSE_BYTES + 1],
                Some("reach_data_response_too_large"),
            ),
        ] {
            let (origin, server) = serve(status, body);
            let result = HttpReachDataTransport::new(&origin).unwrap().claim(&call());
            match expected {
                Some(code) => assert_eq!(result.unwrap_err(), code),
                None => assert!(result.unwrap().is_none()),
            };
            server.join().unwrap();
        }
    }
    #[test]
    fn unsafe_origins_are_rejected() {
        for s in [
            "http://example.com",
            "https://u:p@example.com",
            "https://example.com/path",
            "https://example.com?q=x",
            "https://example.com#x",
            "file:///tmp/reach",
        ] {
            assert!(HttpReachDataTransport::new(s).is_err(), "{s}");
        }
        assert!(HttpReachDataTransport::new("https://example.com").is_ok());
        assert!(HttpReachDataTransport::new("http://127.0.0.1:1234").is_ok());
    }
}
