/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{DnsAnswer, DnsQueryType, DnsQuestion, DnsRecordData, DnsSrvRecord};
use std::net::SocketAddr;
use std::time::Duration;

/// Encodes a dotted domain name into DNS wire format (`\x04shop\x04code\x08internal\x00`).
pub fn encode_dns_name(name: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(name.len() + 2);
    for label in name.trim_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        let len = label.len().min(63) as u8;
        bytes.push(len);
        bytes.extend_from_slice(&label.as_bytes()[..len as usize]);
    }
    bytes.push(0); // Zero root label
    bytes
}

/// Decodes a DNS wire-format name starting at `offset`. Supports label sequences without compression.
pub fn decode_dns_name(bytes: &[u8], offset: &mut usize) -> Result<String, String> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut current = *offset;
    let mut depth = 0;

    loop {
        if depth > 10 {
            return Err("DNS label compression pointer loop".to_string());
        }
        if current >= bytes.len() {
            return Err("unexpected EOF reading DNS name".to_string());
        }
        let len = bytes[current];
        if len == 0 {
            if !jumped {
                *offset = current + 1;
            }
            break;
        }

        // Pointer (top 2 bits set: 11xxxxxx)
        if len & 0xC0 == 0xC0 {
            if current + 1 >= bytes.len() {
                return Err("truncated DNS pointer".to_string());
            }
            let ptr = (((len & 0x3F) as usize) << 8) | (bytes[current + 1] as usize);
            if !jumped {
                *offset = current + 2;
                jumped = true;
            }
            current = ptr;
            depth += 1;
            continue;
        }

        current += 1;
        let label_len = len as usize;
        if current + label_len > bytes.len() {
            return Err("truncated DNS label".to_string());
        }
        let label_str = std::str::from_utf8(&bytes[current..current + label_len])
            .map_err(|e| format!("invalid UTF-8 in DNS label: {e}"))?;
        labels.push(label_str);
        current += label_len;
        if !jumped {
            *offset = current;
        }
    }

    Ok(labels.join("."))
}

/// Parses an incoming DNS query datagram.
pub fn parse_dns_query(buf: &[u8]) -> Result<(u16, DnsQuestion), String> {
    if buf.len() < 12 {
        return Err("DNS packet too short for header".to_string());
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount == 0 {
        return Err("DNS query has 0 questions".to_string());
    }

    let mut offset = 12;
    let name = decode_dns_name(buf, &mut offset)?;
    if offset + 4 > buf.len() {
        return Err("truncated DNS question fields".to_string());
    }
    let qtype = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
    let qclass = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);

    Ok((
        id,
        DnsQuestion {
            name,
            qtype: DnsQueryType::from_u16(qtype),
            qclass,
        },
    ))
}

/// Formats a DNS response datagram.
pub fn format_dns_response(
    id: u16,
    question: &DnsQuestion,
    answers: &[DnsAnswer],
    authoritative: bool,
    rcode: u8,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);

    // Header (12 bytes)
    out.extend_from_slice(&id.to_be_bytes()); // ID
    let mut flags: u16 = 0x8180; // Standard query response, Recursion available
    if authoritative {
        flags |= 0x0400; // Authoritative Answer bit
    }
    flags |= (rcode as u16) & 0x000F;
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    out.extend_from_slice(&(answers.len() as u16).to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0

    // Question Section
    out.extend_from_slice(&encode_dns_name(&question.name));
    out.extend_from_slice(&question.qtype.to_u16().to_be_bytes());
    out.extend_from_slice(&question.qclass.to_be_bytes());

    // Answer Section
    for ans in answers {
        out.extend_from_slice(&encode_dns_name(&ans.name));
        match &ans.data {
            DnsRecordData::A(ip) => {
                out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A = 1
                out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN = 1
                out.extend_from_slice(&ans.ttl.to_be_bytes());
                out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH = 4
                out.extend_from_slice(&ip.octets());
            }
            DnsRecordData::SRV(srv) => {
                out.extend_from_slice(&33u16.to_be_bytes()); // TYPE SRV = 33
                out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN = 1
                out.extend_from_slice(&ans.ttl.to_be_bytes());
                let target_bytes = encode_dns_name(&srv.target);
                let rdlength = (6 + target_bytes.len()) as u16;
                out.extend_from_slice(&rdlength.to_be_bytes());
                out.extend_from_slice(&srv.priority.to_be_bytes());
                out.extend_from_slice(&srv.weight.to_be_bytes());
                out.extend_from_slice(&srv.port.to_be_bytes());
                out.extend_from_slice(&target_bytes);
            }
            DnsRecordData::TXT(txt) => {
                out.extend_from_slice(&16u16.to_be_bytes()); // TYPE TXT = 16
                out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN = 1
                out.extend_from_slice(&ans.ttl.to_be_bytes());
                let bytes = txt.as_bytes();
                let len = bytes.len().min(255) as u8;
                out.extend_from_slice(&((len as u16) + 1).to_be_bytes());
                out.push(len);
                out.extend_from_slice(&bytes[..len as usize]);
            }
        }
    }

    out
}

/// Queries a DNS server over UDP for a specific name and query type.
pub fn query_dns(
    server: SocketAddr,
    domain: &str,
    qtype: DnsQueryType,
    timeout: Duration,
) -> std::io::Result<Vec<DnsAnswer>> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(timeout))?;

    let id: u16 = 0x4242;

    let mut query = Vec::with_capacity(64);
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes()); // Standard query with recursion
    query.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&encode_dns_name(domain));
    query.extend_from_slice(&qtype.to_u16().to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());

    sock.send_to(&query, server)?;

    let mut buf = [0u8; 1024];
    let (n, _) = sock.recv_from(&mut buf)?;
    let resp = &buf[..n];
    if resp.len() < 12 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "DNS reply too short",
        ));
    }

    let ancount = u16::from_be_bytes([resp[6], resp[7]]) as usize;
    let mut offset = 12;
    // Skip question
    let _ = decode_dns_name(resp, &mut offset);
    offset += 4; // qtype + qclass

    let mut answers = Vec::with_capacity(ancount);
    for _ in 0..ancount {
        if offset >= resp.len() {
            break;
        }
        let ans_name = match decode_dns_name(resp, &mut offset) {
            Ok(n) => n,
            Err(_) => break,
        };
        if offset + 10 > resp.len() {
            break;
        }
        let rtype = u16::from_be_bytes([resp[offset], resp[offset + 1]]);
        let ttl = u32::from_be_bytes([
            resp[offset + 4],
            resp[offset + 5],
            resp[offset + 6],
            resp[offset + 7],
        ]);
        let rdlength = u16::from_be_bytes([resp[offset + 8], resp[offset + 9]]) as usize;
        offset += 10;

        if offset + rdlength > resp.len() {
            break;
        }

        match DnsQueryType::from_u16(rtype) {
            DnsQueryType::A if rdlength == 4 => {
                let ip = std::net::Ipv4Addr::new(
                    resp[offset],
                    resp[offset + 1],
                    resp[offset + 2],
                    resp[offset + 3],
                );
                answers.push(DnsAnswer {
                    name: ans_name,
                    ttl,
                    data: DnsRecordData::A(ip),
                });
            }
            DnsQueryType::SRV if rdlength >= 6 => {
                let prio = u16::from_be_bytes([resp[offset], resp[offset + 1]]);
                let weight = u16::from_be_bytes([resp[offset + 2], resp[offset + 3]]);
                let port = u16::from_be_bytes([resp[offset + 4], resp[offset + 5]]);
                let mut target_off = offset + 6;
                if let Ok(target) = decode_dns_name(resp, &mut target_off) {
                    answers.push(DnsAnswer {
                        name: ans_name,
                        ttl,
                        data: DnsRecordData::SRV(DnsSrvRecord {
                            priority: prio,
                            weight,
                            port,
                            target,
                        }),
                    });
                }
            }
            DnsQueryType::TXT if rdlength > 0 => {
                let txt_len = resp[offset] as usize;
                if offset + 1 + txt_len <= resp.len() {
                    let text = String::from_utf8_lossy(&resp[offset + 1..offset + 1 + txt_len])
                        .to_string();
                    answers.push(DnsAnswer {
                        name: ans_name,
                        ttl,
                        data: DnsRecordData::TXT(text),
                    });
                }
            }
            _ => {}
        }
        offset += rdlength;
    }

    Ok(answers)
}
