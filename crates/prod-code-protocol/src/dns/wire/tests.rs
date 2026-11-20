/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::net::{Ipv4Addr, UdpSocket};

fn exchange(reply: Vec<u8>) -> std::io::Result<Vec<DnsAnswer>> {
    exchange_all(vec![reply], Duration::from_secs(2))
}

/// Answers the query with each of `replies` in turn, the query's ID written into each one that
/// is long enough to carry an ID unless it starts with `0xde 0xad` (a spoofed reply).
fn exchange_all(replies: Vec<Vec<u8>>, timeout: Duration) -> std::io::Result<Vec<DnsAnswer>> {
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let addr = server.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let mut buf = [0; 1024];
        let (n, peer) = server.recv_from(&mut buf).unwrap();
        let (id, asked) = parse_dns_query(&buf[..n]).unwrap();
        assert_eq!(asked, question());
        for mut reply in replies {
            if reply.len() >= 2 && reply[..2] != [0xde, 0xad] {
                reply[..2].copy_from_slice(&id.to_be_bytes());
            }
            server.send_to(&reply, peer).unwrap();
        }
    });
    let result = query_dns(addr, "example.test", DnsQueryType::ANY, timeout);
    worker.join().unwrap();
    result
}

/// A reply with another ID, or a query rather than a response, is not the answer: the real
/// reply after it is, and with nothing else the query times out.
#[test]
fn only_the_reply_to_this_query_is_read() {
    let answer = DnsAnswer {
        name: "example.test".into(),
        ttl: 9,
        data: DnsRecordData::A(Ipv4Addr::new(10, 0, 0, 9)),
    };
    let mut spoofed = format_dns_response(0, &question(), &[answer.clone()], false, 0);
    spoofed[..2].copy_from_slice(&[0xde, 0xad]);
    let mut not_a_response = format_dns_response(0, &question(), &[answer.clone()], false, 0);
    not_a_response[2] &= 0x7f;
    let real = DnsAnswer {
        name: "example.test".into(),
        ttl: 9,
        data: DnsRecordData::A(Ipv4Addr::new(127, 0, 0, 1)),
    };
    let reply = format_dns_response(0, &question(), &[real.clone()], false, 0);
    let got = exchange_all(
        vec![spoofed.clone(), not_a_response, reply],
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(got, vec![real]);
    let err = exchange_all(vec![spoofed], Duration::from_millis(300)).unwrap_err();
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{err:?}"
    );
}

fn question() -> DnsQuestion {
    DnsQuestion {
        name: "example.test".into(),
        qtype: DnsQueryType::ANY,
        qclass: 1,
    }
}

#[test]
fn names_and_query_types() {
    for (name, wire, decoded) in [
        ("", vec![0], ""),
        (".a..test.", b"\x01a\x04test\0".to_vec(), "a.test"),
        ("é.test", b"\x02\xc3\xa9\x04test\0".to_vec(), "é.test"),
    ] {
        assert_eq!(encode_dns_name(name), wire);
        let mut offset = 0;
        assert_eq!(decode_dns_name(&wire, &mut offset).unwrap(), decoded);
        assert_eq!(offset, wire.len());
    }
    assert_eq!(
        encode_dns_name(&"a".repeat(70)),
        [vec![63], vec![b'a'; 63], vec![0]].concat()
    );
    for (number, kind) in [
        (1, DnsQueryType::A),
        (16, DnsQueryType::TXT),
        (33, DnsQueryType::SRV),
        (255, DnsQueryType::ANY),
        (42, DnsQueryType::Other(42)),
    ] {
        assert_eq!(DnsQueryType::from_u16(number), kind);
        assert_eq!(kind.to_u16(), number);
    }
    let mut offset = 3;
    assert_eq!(
        decode_dns_name(b"\x01a\0\xc0\x00", &mut offset).unwrap(),
        "a"
    );
    assert_eq!(offset, 5);
}

#[test]
fn malformed_names_and_questions() {
    for (wire, message) in [
        (vec![], "unexpected EOF reading DNS name"),
        (vec![0xc0], "truncated DNS pointer"),
        (vec![0xc0, 0], "DNS label compression pointer loop"),
        (vec![0xc0, 20], "unexpected EOF reading DNS name"),
        (vec![2, b'a'], "truncated DNS label"),
        (
            vec![1, 255, 0],
            "invalid UTF-8 in DNS label: invalid utf-8 sequence of 1 bytes from index 0",
        ),
    ] {
        assert_eq!(decode_dns_name(&wire, &mut 0).unwrap_err(), message);
    }
    assert_eq!(
        parse_dns_query(&[0; 11]).unwrap_err(),
        "DNS packet too short for header"
    );
    assert_eq!(
        parse_dns_query(&[0; 12]).unwrap_err(),
        "DNS query has 0 questions"
    );
    let mut query = vec![0; 12];
    query[5] = 1;
    query.push(0);
    assert_eq!(
        parse_dns_query(&query).unwrap_err(),
        "truncated DNS question fields"
    );
}

#[test]
fn udp_round_trip_all_record_types() {
    let answers: Vec<_> = [
        DnsRecordData::A(Ipv4Addr::new(127, 0, 0, 2)),
        DnsRecordData::SRV(DnsSrvRecord {
            priority: 2,
            weight: 3,
            port: 8080,
            target: "node.test".into(),
        }),
        DnsRecordData::TXT("hello é".into()),
        DnsRecordData::TXT("".into()),
    ]
    .into_iter()
    .map(|data| DnsAnswer {
        name: "example.test".into(),
        ttl: 123,
        data,
    })
    .collect();
    let response = format_dns_response(0x4242, &question(), &answers, true, 0x23);
    assert_eq!(
        &response[..12],
        &[0x42, 0x42, 0x85, 0x83, 0, 1, 0, 4, 0, 0, 0, 0]
    );
    assert_eq!(exchange(response).unwrap(), answers);
    let answer = DnsAnswer {
        name: "example.test".into(),
        ttl: 1,
        data: DnsRecordData::TXT("x".repeat(300)),
    };
    let response = format_dns_response(0x4242, &question(), &[answer], false, 0);
    assert_eq!(
        exchange(response).unwrap()[0].data,
        DnsRecordData::TXT("x".repeat(255))
    );
}

#[test]
fn udp_truncated_and_unsupported_answers() {
    // A datagram too short to be a reply is not one: the query waits on and times out.
    let err = exchange_all(vec![vec![0; 11]], Duration::from_millis(300)).unwrap_err();
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{err:?}"
    );
    let mut base = format_dns_response(0x4242, &question(), &[], false, 0);
    base[7] = 1;
    for tail in [
        vec![],
        vec![0xc0],
        vec![0],
        vec![0, 0, 1, 0, 1, 0, 0, 0, 1, 0, 4, 127],
    ] {
        assert_eq!(exchange([base.clone(), tail].concat()).unwrap(), vec![]);
    }
    // Well-formed RDATA of unknown type, wrong A length, empty TXT, truncated
    // TXT string and an SRV target whose label extends beyond the packet.
    for (kind, data) in [
        (99u16, vec![1]),
        (1, vec![1]),
        (16, vec![]),
        (16, vec![3, b'x']),
        (33, vec![0, 0, 0, 0, 0, 80, 3, b'x']),
    ] {
        let mut response = base.clone();
        response.push(0);
        response.extend_from_slice(&kind.to_be_bytes());
        response.extend_from_slice(&[0, 1, 0, 0, 0, 1]);
        response.extend_from_slice(&(data.len() as u16).to_be_bytes());
        response.extend_from_slice(&data);
        assert_eq!(exchange(response).unwrap(), vec![]);
    }
}
