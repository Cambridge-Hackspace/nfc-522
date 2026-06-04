//! Minimal captive-portal DNS server.
//!
//! Answers *every* A query with the SoftAP gateway IP so that any hostname a
//! client looks up resolves to this device, triggering the OS captive-portal
//! browser. Intended to run forever on its own thread.

use std::net::UdpSocket;

/// Serve DNS on `0.0.0.0:53`, replying to all A queries with `ip`. Never returns.
pub fn serve(ip: [u8; 4]) {
    let socket = match UdpSocket::bind("0.0.0.0:53") {
        Ok(s) => s,
        Err(e) => {
            log::error!("captive DNS: bind :53 failed: {e}");
            return;
        }
    };
    log::info!(
        "captive DNS serving -> {}.{}.{}.{}",
        ip[0],
        ip[1],
        ip[2],
        ip[3]
    );

    let mut buf = [0u8; 512];
    loop {
        let (len, peer) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("captive DNS recv error: {e}");
                continue;
            }
        };

        if let Some(reply) = build_reply(&buf[..len], ip) {
            let _ = socket.send_to(&reply, peer);
        }
    }
}

/// Build an A-record reply pointing the queried name at `ip`.
///
/// Echoes the question section and appends a single answer that uses a name
/// pointer (`0xC00C`) back to the question — valid for any single-question query.
fn build_reply(query: &[u8], ip: [u8; 4]) -> Option<Vec<u8>> {
    // Need at least a header (12 bytes) plus one question.
    if query.len() < 12 {
        return None;
    }

    let mut resp = query.to_vec();
    // Flags: QR=1 (response), Opcode=0, AA=1, RD copied, RA=0; second byte RCODE=0.
    resp[2] = 0x81;
    resp[3] = 0x80;
    // ANCOUNT = 1.
    resp[6] = 0x00;
    resp[7] = 0x01;
    // Zero out NSCOUNT/ARCOUNT to avoid dangling sections.
    resp[8] = 0x00;
    resp[9] = 0x00;
    resp[10] = 0x00;
    resp[11] = 0x00;

    // Answer: pointer to question name, type A, class IN, TTL 60s, RDLEN 4, A.
    resp.extend_from_slice(&[
        0xC0, 0x0C, // name pointer -> offset 12 (the question)
        0x00, 0x01, // TYPE = A
        0x00, 0x01, // CLASS = IN
        0x00, 0x00, 0x00, 0x3C, // TTL = 60
        0x00, 0x04, // RDLENGTH = 4
    ]);
    resp.extend_from_slice(&ip);

    Some(resp)
}
