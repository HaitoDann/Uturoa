//! Petit client de l'API, pour les commandes `mine` et `send` : en attendant
//! les portefeuilles, de quoi frapper et envoyer des pièces sur un réseau de
//! test.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use uturoa_core::{Address, Amount, Hash, MintShare, Transaction, TxBody, UNITS_PER_COIN, encode};
use uturoa_crypto::KeyPair;
use uturoa_keystore::encoding::hash_from_hex;
use uturoa_state::PowHasher;

/// Requête HTTP/1.1 minimale vers l'API ; renvoie le corps JSON.
pub fn http(api: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let mut stream = TcpStream::connect_timeout(&api, Duration::from_secs(5))
        .with_context(|| format!("connexion à l'API {api}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let payload = body.map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {api}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("réponse HTTP invalide"))?;
    let ok = head
        .lines()
        .next()
        .is_some_and(|l| l.split_whitespace().nth(1) == Some("200"));
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if !ok {
        bail!(
            "l'API a refusé : {}",
            json.get("error").and_then(Value::as_str).unwrap_or(body)
        );
    }
    Ok(json)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a Value> {
    v.get(name).ok_or_else(|| anyhow!("champ {name} absent"))
}

/// Montant en pièces (« 1,5 » ou « 1.5 ») converti en unités (9 décimales).
pub fn parse_coins(s: &str) -> Result<Amount> {
    let s = s.trim().replace(',', ".");
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    if frac.len() > 9 || int.is_empty() && frac.is_empty() {
        bail!("montant invalide : {s} (9 décimales au plus)");
    }
    let int: Amount = if int.is_empty() { 0 } else { int.parse()? };
    let frac: Amount = format!("{frac:0<9}").parse()?;
    int.checked_mul(UNITS_PER_COIN)
        .and_then(|v| v.checked_add(frac))
        .ok_or_else(|| anyhow!("montant trop grand"))
}

/// Frappe en continu pour `beneficiary`, en interrogeant l'API. Une erreur
/// (nœud pas encore démarré, redémarrage) est affichée, puis on réessaie.
pub fn mine(api: SocketAddr, beneficiary: Address, pow: &dyn PowHasher) -> Result<()> {
    let mut nonce: u64 = 0;
    loop {
        if let Err(e) = mine_round(api, beneficiary, pow, &mut nonce) {
            eprintln!("frappe interrompue : {e:#} ; nouvel essai dans 2 s");
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

fn mine_round(
    api: SocketAddr,
    beneficiary: Address,
    pow: &dyn PowHasher,
    nonce: &mut u64,
) -> Result<()> {
    {
        let work = http(api, "GET", "/mint/work", None)?;
        let chain_id = field(&work, "chain_id")?
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let anchor_height = field(&work, "anchor_height")?.as_u64().unwrap_or(0);
        let next_height = field(&work, "next_height")?.as_u64().unwrap_or(0);
        let anchor_hash: Hash =
            hash_from_hex(field(&work, "anchor_hash")?.as_str().unwrap_or_default())?;
        let seed: Hash = hash_from_hex(field(&work, "seed")?.as_str().unwrap_or_default())?;
        let difficulty = field(&work, "difficulty")?.as_u64().unwrap_or(1);
        // Une salve de calcul (environ 2 s), puis on rafraîchit l'ancrage.
        let started = std::time::Instant::now();
        let mut found = Vec::new();
        while started.elapsed() < Duration::from_secs(2) && found.len() < 20 {
            let share = MintShare {
                chain_id: chain_id.clone(),
                beneficiary,
                anchor_height,
                anchor_hash,
                nonce: *nonce,
            };
            *nonce = nonce.wrapping_add(1);
            if uturoa_state::meets_target(&pow.hash(&seed, &share.pow_input()), difficulty) {
                found.push(share);
            }
        }
        if !found.is_empty() {
            let n = found.len();
            let r = http(
                api,
                "POST",
                "/shares",
                Some(&serde_json::json!({ "hex": to_hex(&encode(&found)) })),
            )?;
            println!(
                "bloc {next_height} · difficulté {difficulty} · {n} part(s) trouvée(s), {} acceptée(s)",
                r.get("accepted").and_then(Value::as_u64).unwrap_or(0)
            );
        }
    }
    Ok(())
}

/// Construit, signe et soumet un envoi ; renvoie l'identifiant de la transaction.
pub fn send(
    api: SocketAddr,
    key: &KeyPair,
    to: Address,
    amount: Amount,
    memo: Option<String>,
) -> Result<String> {
    let sender = Address::from_public_key(&key.public_key());
    let status = http(api, "GET", "/status", None)?;
    let account = http(api, "GET", &format!("/accounts/{sender}"), None)?;
    let chain_id = field(&status, "chain_id")?
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let base_fee: Amount = field(&status, "base_fee_per_byte")?
        .as_str()
        .unwrap_or("0")
        .parse()?;
    let nonce = field(&account, "nonce")?
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    let include_key = !field(&account, "key_known")?.as_bool().unwrap_or(false);
    let mut tx = Transaction {
        chain_id,
        sender,
        nonce,
        fee: 0,
        body: TxBody::Transfer {
            to,
            amount,
            memo: memo.map(String::into_bytes),
        },
    };
    let len = Amount::try_from(encode(&tx.clone().sign(key, include_key)).len()).unwrap_or(0);
    // Marge de 2 : le frais de base peut monter d'ici l'inclusion.
    tx.fee = base_fee.saturating_mul(len).saturating_mul(2);
    let signed = tx.sign(key, include_key);
    let r = http(
        api,
        "POST",
        "/transactions",
        Some(&serde_json::json!({ "hex": to_hex(&encode(&signed)) })),
    )?;
    Ok(field(&r, "id")?.as_str().unwrap_or_default().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coins_are_parsed_with_nine_decimals() {
        assert_eq!(parse_coins("1").unwrap(), UNITS_PER_COIN);
        assert_eq!(parse_coins("1,5").unwrap(), 1_500_000_000);
        assert_eq!(parse_coins("0.000000001").unwrap(), 1);
        assert_eq!(parse_coins(".25").unwrap(), 250_000_000);
        assert!(parse_coins("0.0000000001").is_err());
        assert!(parse_coins("abc").is_err());
    }
}
