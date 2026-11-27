//! MS-OVBA 2.4.1 compression: the spec's examples, chunk boundaries, raw
//! chunks.

use kantor_vba::compression::{compress, decompress};

/// Deterministic bytes that don't compress.
fn noise(n: usize, mut seed: u32) -> Vec<u8> {
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 7) as u8
        })
        .collect()
}

/// Chunk headers of a container: `(compressed, size of the chunk)`.
fn chunks(container: &[u8]) -> Vec<(bool, usize)> {
    let mut out = Vec::new();
    let mut at = 1;
    while at + 2 <= container.len() {
        let h = u16::from_le_bytes([container[at], container[at + 1]]);
        let size = usize::from(h & 0x0FFF) + 3;
        assert_eq!((h >> 12) & 0b111, 0b011, "chunk signature");
        out.push((h & 0x8000 != 0, size));
        at += size;
    }
    assert_eq!(at, container.len(), "chunks fill the container");
    out
}

#[test]
fn spec_example_without_matches() {
    // MS-OVBA 3.2.1: no repeats, so literals only.
    let data = b"abcdefghijklmnopqrstuv.";
    let expected = [
        0x01, 0x19, 0xB0, 0x00, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x00, 0x69, 0x6A,
        0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x00, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x2E,
    ];
    assert_eq!(compress(data), expected);
    assert_eq!(decompress(&expected).unwrap(), data);
}

#[test]
fn copy_tokens_of_every_bit_count() {
    // MS-OVBA 3.2.2's text, with copy tokens at 4-, 5- and 6-bit offsets
    // (including an overlapping copy), the container header worked out
    // from the body.
    let body: &[u8] = &[
        0x00, 0x23, 0x61, 0x61, 0x61, 0x62, 0x63, 0x64, 0x65, 0x82, 0x66, 0x00, 0x70, 0x61, 0x67,
        0x68, 0x69, 0x6A, 0x01, 0x38, 0x08, 0x61, 0x6B, 0x6C, 0x00, 0x30, 0x6D, 0x6E, 0x6F, 0x70,
        0x06, 0x71, 0x02, 0x70, 0x04, 0x10, 0x72, 0x73, 0x74, 0x75, 0x76, 0x10, 0x77, 0x78, 0x79,
        0x7A, 0x00, 0x3C,
    ];
    let header = 0xB000u16 | (body.len() as u16 + 2 - 3);
    let mut container = vec![0x01];
    container.extend_from_slice(&header.to_le_bytes());
    container.extend_from_slice(body);
    let text = b"#aaabcdefaaaaghijaaaaaklaaamnopqaaaaaaaaaaaarstuvwxyzaaa";
    assert_eq!(decompress(&container).unwrap(), text);
    // Ours finds matches too, and reads back the same.
    let ours = compress(text);
    assert!(ours.len() < text.len(), "{} bytes", ours.len());
    assert_eq!(decompress(&ours).unwrap(), text);
}

#[test]
fn round_trips_at_chunk_boundaries() {
    let line = b"    Range(\"A1\").Value = Range(\"B1\").Value * 2 ' copy\r\n";
    for n in [
        0, 1, 2, 3, 4095, 4096, 4097, 8191, 8192, 8193, 12_345, 40_000,
    ] {
        let data: Vec<u8> = line.iter().copied().cycle().take(n).collect();
        let c = compress(&data);
        assert_eq!(decompress(&c).unwrap(), data, "{n} bytes");
        let expected_chunks = n.div_ceil(4096);
        let got = chunks(&c);
        assert_eq!(got.len(), expected_chunks, "{n} bytes");
        assert!(
            got.iter().all(|(compressed, _)| *compressed),
            "{n}: {got:?}"
        );
    }
}

#[test]
fn matches_restart_in_each_chunk() {
    // The second chunk repeats the first one's text: its copy tokens can
    // only point inside itself, so it starts with literals.
    let mut data = noise(4096, 7);
    data.extend_from_slice(&data.clone()[..1000]);
    let c = compress(&data);
    assert_eq!(decompress(&c).unwrap(), data);
    let got = chunks(&c);
    assert_eq!(got.len(), 2);
    // 1000 literals (and their flag bytes) can't shrink.
    assert!(got[1].1 >= 1000 + 2, "{got:?}");
}

#[test]
fn raw_chunks_for_noise() {
    // A full chunk of noise is stored raw (flag bit off, 4096 bytes).
    let data = noise(4096 * 2 + 10, 99);
    let c = compress(&data);
    let got = chunks(&c);
    assert_eq!(got[0], (false, 4098));
    assert_eq!(got[1], (false, 4098));
    assert!(got[2].0);
    assert_eq!(decompress(&c).unwrap(), data);

    // A last chunk that doesn't shrink is raw and padded to 4096 bytes.
    let data = noise(3700, 3);
    let c = compress(&data);
    assert_eq!(chunks(&c), [(false, 4098)]);
    let back = decompress(&c).unwrap();
    assert_eq!(back.len(), 4096);
    assert_eq!(&back[..3700], &data[..]);
    assert!(back[3700..].iter().all(|b| *b == 0));
}

#[test]
fn bad_containers_are_errors() {
    assert!(decompress(&[]).is_err());
    assert!(decompress(&[0x02, 0x00, 0xB0]).is_err());
    // A copy token pointing before the chunk.
    assert!(decompress(&[0x01, 0x02, 0xB0, 0x01, 0x00, 0x70]).is_err());
    // Only the signature: nothing.
    assert_eq!(decompress(&[0x01]).unwrap(), b"");
}
