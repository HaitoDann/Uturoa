//! Client de l'API d'un nœud (HTTP/1.1 minimal, sans dépendance réseau).

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;
use uturoa_core::{Hash, MintShare, SignedTransaction, encode};

#[derive(Clone, Debug)]
pub struct Node {
    pub addr: String,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Empreinte hexadécimale de 32 octets.
pub fn hash_from_hex(s: &str) -> Result<Hash> {
    let bytes = hex::decode(s.trim()).context("empreinte : hexadécimal invalide")?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("une empreinte fait 32 octets"))
}

impl Node {
    pub fn new(addr: impl Into<String>) -> Self {
        Self { addr: addr.into() }
    }

    fn socket(&self) -> Result<SocketAddr> {
        self.addr
            .to_socket_addrs()
            .with_context(|| format!("adresse du nœud invalide : {}", self.addr))?
            .next()
            .ok_or_else(|| anyhow!("adresse du nœud introuvable : {}", self.addr))
    }

    /// Requête vers l'API ; renvoie le corps JSON, ou l'erreur du nœud.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let addr = self.socket()?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .with_context(|| format!("nœud injoignable ({})", self.addr))?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let payload = body.map(Value::to_string).unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            self.addr,
            payload.len()
        );
        stream.write_all(request.as_bytes())?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        let (head, body) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| anyhow!("réponse invalide du nœud"))?;
        let ok = head
            .lines()
            .next()
            .is_some_and(|l| l.split_whitespace().nth(1) == Some("200"));
        let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        if !ok {
            bail!(
                "{}",
                json.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("refusé par le nœud")
            );
        }
        Ok(json)
    }

    pub fn status(&self) -> Result<Value> {
        self.call("GET", "/status", None)
    }

    pub fn account(&self, address: &str) -> Result<Value> {
        self.call("GET", &format!("/accounts/{address}"), None)
    }

    pub fn validators(&self) -> Result<Value> {
        self.call("GET", "/validators", None)
    }

    pub fn mint_work(&self) -> Result<Value> {
        self.call("GET", "/mint/work", None)
    }

    pub fn submit_tx(&self, tx: &SignedTransaction) -> Result<String> {
        let r = self.call(
            "POST",
            "/transactions",
            Some(&serde_json::json!({ "hex": to_hex(&encode(tx)) })),
        )?;
        Ok(r.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// Renvoie le nombre de parts acceptées.
    pub fn submit_shares(&self, shares: &[MintShare]) -> Result<u64> {
        let r = self.call(
            "POST",
            "/shares",
            Some(&serde_json::json!({ "hex": to_hex(&encode(&shares.to_vec())) })),
        )?;
        Ok(r.get("accepted").and_then(Value::as_u64).unwrap_or(0))
    }
}
